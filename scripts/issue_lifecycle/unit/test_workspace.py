"""Exercise vault configuration and launcher dispatch without touching a real vault."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


LAUNCHER = Path(__file__).resolve().parents[2] / "issue-lifecycle"


class WorkspaceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="yinz-lifecycle-vault-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.control = self.root / "control"
        self.control.mkdir()
        (self.control / ".swamp.yaml").write_text("tools: []\n")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "calls.jsonl"
        self.value = self.root / "vault.json"
        self.value.write_text(json.dumps({"value": "work"}))
        stub = self.bin / "swamp"
        stub.write_text(f'''#!{sys.executable}
import json, os, pathlib, sys
assert not any(key in os.environ for key in ("SWAMP_SERVE_URL", "SWAMP_SERVER_URL", "SWAMP_REPO_DIR", "SWAMP_WORKFLOWS_DIR"))
args = sys.argv[1:]
entry = {{"args": args, "cwd": os.getcwd()}}
if "--stdin" in args:
    entry["input"] = json.load(sys.stdin)
with pathlib.Path(os.environ["TEST_VAULT_LOG"]).open("a") as log:
    log.write(json.dumps(entry) + "\\n")
if args[:2] == ["vault", "read-secret"]:
    assert args[2:4] == ["default", "AGENDA_WORKSPACE"]
    print(pathlib.Path(os.environ["TEST_VAULT_VALUE"]).read_text())
elif args[:2] == ["data", "query"]:
    print(json.dumps({{"results": [{{"status": "current", "presentation": "Fixture handoff"}}]}}))
else:
    print("{{}}")
''')
        stub.chmod(0o755)
        self.env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
                    "TEST_VAULT_LOG": str(self.log), "TEST_VAULT_VALUE": str(self.value),
                    "SWAMP_SERVER_URL": "http://must-not-use.invalid", "SWAMP_REPO_DIR": "/wrong"}

    def launch(self, command="status", *args):
        return subprocess.run([sys.executable, str(LAUNCHER), "--control", str(self.control),
                               "--item", "7", command, *args], cwd=self.root, env=self.env,
                              capture_output=True, text=True, timeout=10)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_status_uses_control_vault_without_workspace_argument(self):
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        self.assertEqual(len(calls), 2)
        self.assertIn("issue-work-7", calls[1]["args"][2])
        self.assertTrue(all(call["cwd"] == str(self.control) for call in calls))
        self.assertTrue(all(call["args"][call["args"].index("--repo-dir") + 1] == str(self.control) for call in calls))

    def test_start_passes_vault_workspace_as_issue_identity(self):
        result = self.launch("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls()[-1]["input"], {"workspace": "work", "itemId": 7, "issueName": "issue-work-7"})

    def test_explicit_workspace_is_an_assertion(self):
        result = self.launch("status", "--workspace", "other")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match", result.stderr)
        self.assertEqual(len(self.calls()), 1)

    def test_invalid_or_missing_workspace_never_dispatches(self):
        for value in ({}, {"value": ""}, {"value": "bad/name"}, {"value": None}, {"value": "work\n"}):
            with self.subTest(value=value):
                self.value.write_text(json.dumps(value))
                self.log.unlink(missing_ok=True)
                result = self.launch()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Cannot read a valid Agenda workspace", result.stderr)
                self.assertEqual(len(self.calls()), 1)

    def test_missing_vault_fails_with_setup_instructions(self):
        self.value.unlink()
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("swamp vault put default AGENDA_WORKSPACE", result.stderr)
        self.assertEqual(len(self.calls()), 1)

    def test_worker_commands_capture_invoking_control_identity(self):
        self.env.update(YINZ_SESSION_NAME="dev", YINZ_PANE_ID="0")
        for command in ("drive", "agent", "restart-agent"):
            with self.subTest(command=command):
                result = self.launch(command)
                self.assertEqual(result.returncode, 0, result.stderr)
                dispatched = next(call["input"] for call in reversed(self.calls()) if "input" in call)
                if command == "drive":
                    dispatched = dispatched["drive"]
                self.assertEqual(dispatched["control"], {"session": "dev", "paneId": "terminal_0"})

    def test_drive_without_pane_environment_preserves_saved_settings(self):
        self.env.pop("YINZ_SESSION_NAME", None)
        self.env.pop("YINZ_PANE_ID", None)
        result = self.launch("drive")
        self.assertEqual(result.returncode, 0, result.stderr)
        dispatched = next(call["input"] for call in self.calls() if "input" in call)
        self.assertEqual(dispatched["drive"], {})
