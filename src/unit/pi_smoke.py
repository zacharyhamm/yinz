#!/usr/bin/env python3
"""Run installed Pi startup/reload with isolated settings and no model request."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time


def main():
    binary = shutil.which("pi")
    assert binary, "Install Pi 1.0.0 to run this optional smoke check"
    with tempfile.TemporaryDirectory(prefix="yinz-pi-smoke-") as folder:
        root = Path(folder)
        log = root / "reports.jsonl"
        reporter = root / "yinz"
        reporter.write_text("#!/usr/bin/env python3\nimport sys\nwith open(" + repr(str(log))
                            + ", 'a') as out: out.write(sys.stdin.read() + '\\n')\n")
        reporter.chmod(0o700)
        template = Path(__file__).resolve().parents[2] / "yinz-runtime/src/agent/pi-extension.ts"
        extension = root / "reporter.ts"
        extension.write_text(template.read_text().replace("__VERSION__", "smoke")
                             .replace("__YINZ_BIN__", json.dumps(str(reporter)))
                             .replace("__TOKEN_ENV__", '"YINZ_REMOTE_PANE_TOKEN"'))
        env = {**os.environ, "PI_CODING_AGENT_DIR": str(root / "agent"),
               "YINZ_PANE_ID": "1", "TERM": "xterm-256color"}
        env.pop("YINZ_REMOTE_PANE_TOKEN", None)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
        process = subprocess.Popen([binary, "--offline", "--no-approve", "--no-extensions",
                                    "--no-skills", "--no-prompt-templates", "--no-context-files",
                                    "--no-session", "--extension", str(extension)],
                                   stdin=slave, stdout=slave, stderr=slave, cwd=root, env=env)
        os.close(slave)
        output = bytearray()

        def wait_for(predicate):
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    try:
                        data = os.read(master, 65536)
                    except OSError:
                        data = b""
                    output.extend(data)
                    if b"\x1b[6n" in data:
                        os.write(master, b"\x1b[1;1R")
                reports = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
                if predicate(reports):
                    return reports
                assert process.poll() is None, output.decode(errors="replace")[-4000:]
            raise AssertionError(output.decode(errors="replace")[-4000:])

        try:
            first = wait_for(lambda reports: len(reports) >= 1)
            assert first[0]["hook_event_name"] == "SessionStart"
            # Wait for Pi's editor to enter bracketed-paste mode before sending a command.
            wait_for(lambda _: b"\x1b[?2004h" in output)
            os.write(master, b"/reload\r")
            reports = wait_for(lambda reports: len(reports) >= 2)
            assert [report["hook_event_name"] for report in reports] == ["SessionStart", "SessionStart"]
            assert reports[0]["session_id"] == reports[1]["session_id"]
            os.write(master, b"/quit\r")
            reports = wait_for(lambda reports: any(report["hook_event_name"] == "SessionEnd" for report in reports))
            assert len(reports) == 3, reports
            process.wait(timeout=10)
            print("Pi startup, reload, and shutdown passed without a model request")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)


if __name__ == "__main__":
    main()
