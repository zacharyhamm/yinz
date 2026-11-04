"""Exercise installed Swamp and Agenda in a disposable repository/workspace.

Cargo and Yinz are explicit protocol stubs: this checks workflow dispatch and
merge/agent wiring, not Rust CI or a live coding agent. Run this file explicitly
when the installed Swamp binary and its local authentication are available.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parents[3]


class InstalledSwampSmokeTest(unittest.TestCase):
    def setUp(self):
        if not shutil.which("swamp") or not shutil.which("agenda"):
            self.skipTest("installed swamp and agenda are required for the dispatch smoke test")
        self.temporary = tempfile.TemporaryDirectory(prefix="yinz-swamp-smoke-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        for directory in ("extensions", "models", "workflows"):
            if (SOURCE / directory).exists():
                shutil.copytree(SOURCE / directory, self.repo / directory, ignore=shutil.ignore_patterns("unit", "__pycache__"))
        for name in (".swamp.yaml", ".gitignore"):
            shutil.copy2(SOURCE / name, self.repo / name)
        (self.repo / "scripts" / "issue_lifecycle").mkdir(parents=True)
        for name in ("issue-lifecycle", "issue_lifecycle/git_ops.py", "issue_lifecycle/workspace.py"):
            shutil.copy2(SOURCE / "scripts" / name, self.repo / "scripts" / name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.xdg_data = self.root / "data"
        self.xdg_state = self.root / "state"
        self.xdg_data.mkdir()
        self.xdg_state.mkdir()
        self.cargo_log = self.root / "cargo.jsonl"
        self.yinz_log = self.root / "yinz.jsonl"
        self.yinz_state = self.root / "yinz-state.json"
        self.env = {
            **os.environ,
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "XDG_DATA_HOME": str(self.xdg_data),
            "XDG_STATE_HOME": str(self.xdg_state),
            "SWAMP_NO_INBOX_NOTICE": "1",
            "SWAMP_TELEMETRY_DISABLED": "1",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "CARGO_HOME": str(self.root / "cargo-home"),
            "YINZ_SMOKE_CARGO_LOG": str(self.cargo_log),
            "YINZ_SMOKE_AGENT_LOG": str(self.yinz_log),
            "YINZ_SMOKE_AGENT_STATE": str(self.yinz_state),
        }
        for name in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE"):
            self.env.pop(name, None)
        self.stub("cargo", '''import json, os, pathlib, subprocess, sys
assert sys.argv[1:] == ["x", "ci", "check"], sys.argv
head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
with pathlib.Path(os.environ["YINZ_SMOKE_CARGO_LOG"]).open("a") as log:
    log.write(json.dumps({"args":sys.argv[1:],"head":head,"cwd":os.getcwd(),"stub":True}) + "\\n")
''')
        for program in ("claude", "codex", "muse", "pi", "agy"):
            self.stub(program, 'import sys\nraise SystemExit("A coding CLI must be launched through Yinz, never headlessly, in this smoke test")\n')
        self.stub("yinz", '''import json, os, pathlib, sys
args = sys.argv[1:]
assert args[:3] == ["--session", "smoke", "agent"], args
assert args[-1] == "--json", args
operation = args[3]
stdin = sys.stdin.read() if operation == "prompt" else None
with pathlib.Path(os.environ["YINZ_SMOKE_AGENT_LOG"]).open("a") as log:
    log.write(json.dumps({"args":args,"stdin":stdin,"stub":True}) + "\\n")
state_file = pathlib.Path(os.environ["YINZ_SMOKE_AGENT_STATE"])
response = {"outcome":"ok","agents":[],"spawned":None,"output":None,"message":None,"acknowledged":False,"turn_id":None}
if operation == "spawn":
    assert args[args.index("--kind")+1] == "claude"
    assert "--allow-outside-worktree" in args
    state = {"cwd":args[args.index("--cwd")+1]}
    state_file.write_text(json.dumps(state))
    response["spawned"] = {"pane_id":"terminal_17","tab_id":3,"branch":None,"cwd":state["cwd"]}
else:
    state = json.loads(state_file.read_text())
    response["agents"] = [{"pane_id":"terminal_17","session":"smoke","host_id":"smoke-host","tab_id":3,"is_suppressed":False,"target":"Local","agent":{"kind":"claude","session_id":"smoke-conversation","state":"Idle","source":"Reported"},"cwd":state["cwd"],"branch":"main","ready":True,"status_available":True,"prompt_supported":True,"not_ready_reason":None,"readiness":None,"input_revision":None,"clear_draft_supported":False,"background_tasks":[]}]
    assert args[4] == "terminal_17", args
    if operation == "wait":
        assert args[args.index("--until")+1] == "idle", args
    elif operation == "prompt":
        assert args[args.index("--file")+1] == "-"
        assert args[args.index("--conversation")+1] == "smoke-conversation"
        assert "--wait" in args
        assert "triage" in stdin.lower() or "Investigate the issue" in stdin
        response.update(outcome="completed", acknowledged=True, submission_id=args[args.index("--submission")+1], turn_id="smoke-turn", message=json.dumps({"classification":"maintenance","summary":"Add a smoke marker; acceptance requires the marker on local main."}))
    else:
        raise AssertionError("Unexpected Yinz operation: " + operation)
print(json.dumps(response))
''')
        self.git(self.repo, "init", "--initial-branch=main")
        self.git(self.repo, "config", "user.name", "Lifecycle Smoke")
        self.git(self.repo, "config", "user.email", "lifecycle@example.invalid")
        self.git(self.repo, "add", ".")
        self.git(self.repo, "commit", "-m", "smoke fixture")
        self.agenda("workspace", "create", "lifecycle-test", workspace=False)
        self.command(["swamp", "vault", "create", "local_encryption", "default", "--json"])
        self.command(["swamp", "vault", "put", "default", "AGENDA_WORKSPACE", "--json"], stdin="lifecycle-test\n")
        self.git(self.repo, "add", "vaults")
        self.git(self.repo, "commit", "-m", "configure fixture vault")
        category = self.agenda("category", "add", "yinz", "--matching", "false")
        self.category_id = category["category"]["id"]
        captured = self.agenda("item", "add", "Add lifecycle smoke marker", "--note", "Acceptance: marker.txt exists on local main. No production issue is involved.")
        self.item_id = captured["item"]["id"]
        self.agenda("category", "assign", str(self.category_id), str(self.item_id))

    def stub(self, name, source):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n" + source)
        path.chmod(0o755)

    def command(self, args, *, cwd=None, stdin=None, timeout=120):
        process = subprocess.Popen(args, cwd=cwd or self.repo, env=self.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            stdout, stderr = process.communicate(stdin, timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                stdout, stderr = process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                stdout, stderr = process.communicate()
            self.fail(f"Command timed out: {args}\n{stdout}\n{stderr}")
        diagnostics = ""
        if process.returncode:
            status = subprocess.run(["git", "-C", str(self.repo), "status", "--short", "--untracked-files=all"], env=self.env, text=True, capture_output=True, timeout=10)
            diff = subprocess.run(["git", "-C", str(self.repo), "diff", "--stat"], env=self.env, text=True, capture_output=True, timeout=10)
            diagnostics = f"\nFixture status:\n{status.stdout}{diff.stdout}"
        self.assertEqual(process.returncode, 0, f"Command failed: {args}\nstdout:\n{stdout}\nstderr:\n{stderr}{diagnostics}")
        return stdout

    def git(self, path, *args):
        return self.command(["git", "-C", str(path), *args]).strip()

    def agenda(self, *args, workspace=True):
        selection = ["--workspace", "lifecycle-test"] if workspace else []
        return json.loads(self.command(["agenda", "--json", *selection, *args]))

    def lifecycle(self, method, inputs=None):
        command = [sys.executable, str(self.repo / "scripts" / "issue-lifecycle"), "--control", str(self.repo), "--item", str(self.item_id), method]
        if inputs is not None:
            command.extend(["--input-file", "-"])
        return self.command(command, stdin=json.dumps(inputs) if inputs is not None else None)

    def state(self):
        document = json.loads(self.lifecycle("status"))
        records = document["results"]
        states = [record for record in records if "issue" in record and "phase" in record]
        self.assertEqual(len(states), 1, document)
        return states[0]

    def test_installed_dispatch_agents_rebase_ci_merge_and_agenda_completion(self):
        self.command(["swamp", "workflow", "validate", "yinz-issue-start", "--json"])
        # A direct workflow call cannot bypass the configured vault workspace.
        rejected = subprocess.run(
            ["swamp", "workflow", "run", "yinz-issue-start", "--stdin"],
            input=json.dumps({"workspace": "other", "itemId": self.item_id,
                              "issueName": f"issue-other-{self.item_id}"}),
            cwd=self.repo, env=self.env, text=True, capture_output=True, timeout=30,
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("Vault Agenda workspace does not match", rejected.stdout + rejected.stderr)
        self.lifecycle("start")
        self.assertEqual(self.state()["phase"], "triaging")
        self.lifecycle("agent", {"role": "triage", "session": "smoke", "kind": "claude"})
        state = self.state()
        self.assertEqual(state["phase"], "classified")
        self.assertEqual(state["classification"]["classification"], "maintenance")
        calls = [json.loads(line) for line in self.yinz_log.read_text().splitlines()]
        self.assertEqual([call["args"][3] for call in calls], ["spawn", "wait", "prompt"])
        self.assertIn("Return exactly one JSON object", calls[-1]["stdin"])
        self.lifecycle("plan", {
            "summary": "Add the disposable fixture's marker.",
            "steps": [{"id": "marker", "description": "Create marker.txt", "files": ["marker.txt"]}],
            "testingStrategy": "Exercise the merge protocol using an explicit Cargo stub; this is not Rust verification.",
        })
        self.lifecycle("review-plan", {"planVersion": 1, "findings": []})
        self.lifecycle("approve", {"planVersion": 1, "request": "Approve the disposable smoke fixture plan."})
        worktree = self.root / "issue-worktree"
        self.lifecycle("implement", {"worktree": str(worktree), "branch": "smoke-issue"})
        (worktree / "marker.txt").write_text("smoke marker\n")
        self.git(worktree, "add", "marker.txt")
        self.git(worktree, "commit", "-m", "add smoke marker")
        original = self.git(worktree, "rev-parse", "HEAD")
        # Move local main after development so the merge workflow must rebase.
        (self.repo / "base-change.txt").write_text("main advanced\n")
        self.git(self.repo, "add", "base-change.txt")
        self.git(self.repo, "commit", "-m", "advance main before merge")
        base = self.git(self.repo, "rev-parse", "HEAD")
        self.lifecycle("review-code", {
            "planVersion": 1, "commit": original,
            "steps": [{"id": "marker", "status": "implemented"}], "findings": [],
            "preliminaryChecks": [{"command": "inspect fixture marker", "result": "passed", "summary": "marker.txt contains the expected fixture text."}],
        })
        self.lifecycle("ready", {"commit": original, "mergeRequest": "Merge the disposable smoke fixture locally."})
        self.lifecycle("merge")
        merged = self.state()
        self.assertEqual(merged["phase"], "merged")
        self.assertEqual(merged["receipt"]["ciStatus"], "passed")
        self.assertEqual(merged["receipt"]["baseCommit"], base)
        self.assertNotEqual(merged["receipt"]["rebasedCommit"], original)
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), merged["receipt"]["mergedCommit"])
        cargo = [json.loads(line) for line in self.cargo_log.read_text().splitlines()]
        self.assertEqual(len(cargo), 1)
        self.assertEqual(cargo[0]["args"], ["x", "ci", "check"])
        self.assertEqual(cargo[0]["head"], merged["receipt"]["mergedCommit"])
        self.assertEqual((self.repo / "marker.txt").read_text(), "smoke marker\n")
        self.assertFalse(self.agenda("item", "show", str(self.item_id))["item"]["completed"])
        self.lifecycle("summarize", {"outcomeMet": True, "summary": "The disposable marker reached local main; dispatch and Cargo/Yinz stub protocols passed."})
        self.assertEqual(self.state()["phase"], "done")
        self.assertTrue(self.agenda("item", "show", str(self.item_id))["item"]["completed"])
        self.assertEqual(self.git(self.repo, "status", "--short"), "")


if __name__ == "__main__":
    unittest.main()
