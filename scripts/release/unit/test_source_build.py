"""Exercise visible source-build worker outcomes without compiling Yinz."""
import importlib.util
import json
import os
import select
import subprocess
import sys
import time
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[3] / "src/live_update/source_build.py"
spec = importlib.util.spec_from_file_location("source_build_worker", SCRIPT)
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class SourceBuildWorkerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.checkout = self.root / "checkout"
        self.checkout.mkdir()
        (self.checkout / "Cargo.lock").write_text("pinned dependency lock\n")
        self.record = self.root / "build.json"
        self.record.write_text(json.dumps({"commit": "0123456789abcdef", "snapshot": str(self.checkout),
                                          "targets": ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"],
                                          "log": str(self.root / "build.log")}))
        for tool in ("cargo-zigbuild", "zig", "protoc", "python3", "file", "readelf"):
            (self.tools / tool).symlink_to("/usr/bin/true")
        self.script("rustup", "printf '%s\\n' x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu")
        self.script("cargo", 'printf "%s\\n" "$*"; touch "built-$3"')
        self.environment = patch.dict(os.environ, {"PATH": str(self.tools) + ":/usr/bin"})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def script(self, name, body):
        path = self.tools / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def run_worker(self):
        # Avoid altering the unittest process's installed handlers.
        with patch.object(worker.signal, "signal"):
            return worker.build(self.record)

    def test_success_builds_each_target_and_keeps_log_and_result(self):
        self.assertEqual(self.run_worker(), 0)
        self.assertTrue(json.loads((self.root / "result.json").read_text())["success"])
        for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
            self.assertTrue((self.checkout / f"built-{target}").exists())
            self.assertIn(target, (self.root / "build.log").read_text())

    def test_compiler_failure_stops_before_second_target(self):
        self.script("cargo", 'echo compiler-failure; exit 7')
        self.assertEqual(self.run_worker(), 1)
        result = json.loads((self.root / "result.json").read_text())
        self.assertFalse(result["success"])
        self.assertIn("exit 7", result["error"])
        self.assertIn("compiler-failure", (self.root / "build.log").read_text())
        self.assertFalse((self.checkout / "built-aarch64-unknown-linux-gnu").exists())

    def test_cancel_before_worker_start_never_launches_compilation(self):
        (self.root / "cancel").touch()
        self.assertEqual(self.run_worker(), 1)
        self.assertIn("cancelled", json.loads((self.root / "result.json").read_text())["error"])
        self.assertEqual([path.name for path in self.checkout.iterdir()], ["Cargo.lock"])

    def test_missing_prerequisite_is_diagnostic_failure(self):
        with patch.object(worker.shutil, "which", return_value=None):
            self.assertEqual(self.run_worker(), 1)
        self.assertIn("requires cargo", (self.root / "build.log").read_text())
        self.assertFalse(json.loads((self.root / "result.json").read_text())["success"])

    def test_cancellation_stops_active_compiler_and_its_child(self):
        self.script("cargo", 'sleep 60 & echo "child=$!"; wait')
        with subprocess.Popen([sys.executable, str(SCRIPT), str(self.record)], stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT) as process:
            output = b""
            deadline = time.monotonic() + 10
            while b"child=" not in output:
                remaining = deadline - time.monotonic()
                self.assertGreater(remaining, 0, output.decode())
                self.assertTrue(select.select([process.stdout], [], [], remaining)[0])
                output += os.read(process.stdout.fileno(), 65536)
            (self.root / "cancel").touch()
            remaining_output, _ = process.communicate(timeout=10)
            output += remaining_output
            self.assertEqual(process.returncode, 1, output.decode())
        result = json.loads((self.root / "result.json").read_text())
        self.assertFalse(result["success"])
        child_pid = int(output.split(b"child=", 1)[1].splitlines()[0])
        child_stat = Path(f"/proc/{child_pid}/stat")
        if child_stat.exists():
            # An exited orphan may await init's reap; it must no longer execute.
            self.assertEqual(child_stat.read_text().rsplit(")", 1)[1].split()[0], "Z")
        self.assertIn("cancelled", result["error"])

    def test_changed_committed_lock_refuses_successful_compiler_output(self):
        self.script("cargo", 'echo changed-dependencies > Cargo.lock; echo compiler-succeeded')
        self.assertEqual(self.run_worker(), 1)
        result = json.loads((self.root / "result.json").read_text())
        self.assertFalse(result["success"])
        self.assertIn("pinned Cargo.lock changed", result["error"])
        self.assertNotIn("Compiling aarch64", (self.root / "build.log").read_text())

    def test_stale_process_receipt_never_signals_reused_pid(self):
        process = self.root / "process.json"
        process.write_text(json.dumps({"pid": os.getpid(), "start": "not-this-process"}))
        with patch.object(worker.os, "killpg") as kill:
            worker.stop(process)
        kill.assert_not_called()
