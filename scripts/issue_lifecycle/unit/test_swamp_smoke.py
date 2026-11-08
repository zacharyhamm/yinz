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
        for directory in ("extensions", "models", "workflows", ".agents"):
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
            "YINZ_SESSION_NAME": "smoke",
            "YINZ_PANE_ID": "0",
        }
        for name in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE"):
            self.env.pop(name, None)
        self.stub("cargo", '''import json, os, pathlib, subprocess, sys
assert sys.argv[1:] == ["x", "ci", "check"], sys.argv
head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
with pathlib.Path(os.environ["YINZ_SMOKE_CARGO_LOG"]).open("a") as log:
    log.write(json.dumps({"args":sys.argv[1:],"head":head,"cwd":os.getcwd(),"stub":True}) + "\\n")
print(">> ci phase format --check: passed")
fail_once = os.environ.get("YINZ_SMOKE_CARGO_FAIL_ONCE")
if fail_once and not pathlib.Path(fail_once).exists():
    pathlib.Path(fail_once).touch()
    print("        FAIL [   0.019s] (1/4) smoke-fixture tests::flaky_marker")
    print(">> ci phase test: failed")
    print("\\n>> Failed: test")
    sys.exit(101)
print(">> ci phase test: passed")
''')
        for program in ("claude", "codex", "muse", "pi", "agy"):
            self.stub(program, 'import sys\nraise SystemExit("A coding CLI must be launched through Yinz, never headlessly, in this smoke test")\n')
        self.stub("yinz", '''import json, os, pathlib, subprocess, sys
args = sys.argv[1:]
assert args[:2] == ["--session", "smoke"], args
assert args[-1] == "--json", args
operation = args[3]
stdin = sys.stdin.read() if operation == "prompt" else None
with pathlib.Path(os.environ["YINZ_SMOKE_AGENT_LOG"]).open("a") as log:
    log.write(json.dumps({"args":args,"stdin":stdin,"stub":True}) + "\\n")
state_file = pathlib.Path(os.environ["YINZ_SMOKE_AGENT_STATE"])
if operation == "list-panes":
    assert args == ["--session", "smoke", "action", "list-panes", "--json"], args
    panes = [{"id":0,"is_plugin":False,"tab_id":int(os.environ.get("YINZ_SMOKE_CONTROL_TAB", "0"))}]
    if state_file.exists() and not json.loads(state_file.read_text()).get("closed"):
        panes.append({"id":17,"is_plugin":False,"tab_id":3})
    print(json.dumps(panes))
    sys.exit(0)
assert args[2] == "agent", args
response = {"outcome":"ok","agents":[],"spawned":None,"output":None,"message":None,"acknowledged":False,"turn_id":None}
if operation == "spawn":
    assert args[args.index("--kind")+1] == "claude"
    assert "--allow-outside-worktree" in args
    assert args[args.index("--stack-for")+1] == "terminal_0"
    assert "--tab" not in args
    state = {"cwd":args[args.index("--cwd")+1]}
    state_file.write_text(json.dumps(state))
    response["spawned"] = {"pane_id":"terminal_17","tab_id":3,"branch":None,"cwd":state["cwd"]}
else:
    state = json.loads(state_file.read_text())
    response["agents"] = [{"pane_id":"terminal_17","session":"smoke","host_id":"smoke-host","tab_id":3,"is_suppressed":False,"target":"Local","agent":{"kind":"claude","session_id":"smoke-conversation","state":"Idle","source":"Reported"},"cwd":state["cwd"],"branch":"main","ready":True,"status_available":True,"prompt_supported":True,"not_ready_reason":None,"readiness":None,"input_revision":None,"clear_draft_supported":False,"background":False}]
    assert args[4] == "terminal_17", args
    if operation == "close-completed":
        assert args[args.index("--conversation")+1] == "smoke-conversation"
        assert args[args.index("--submission")+1] == state["submission"]
        state["closed"] = True
        state_file.write_text(json.dumps(state))
    elif operation == "wait":
        if "--until" in args:
            assert args[args.index("--until")+1] == "idle", args
        else:
            assert args[args.index("--submission")+1] == state["submission"], args
            assert args[args.index("--conversation")+1] == "smoke-conversation"
            response.update(outcome="completed", acknowledged=True, submission_id=state["submission"], turn_id="smoke-turn", message=os.environ.get("YINZ_SMOKE_FINAL_TEXT", '{"findings":['))
    elif operation == "prompt":
        assert args[args.index("--file")+1] == "-"
        assert args[args.index("--conversation")+1] == "smoke-conversation"
        assert "--wait" in args
        if "Current lifecycle snapshot (issue requirements and recorded evidence):" in stdin:
            snapshot = json.loads(stdin.split("Current lifecycle snapshot (issue requirements and recorded evidence):\\n\\n", 1)[1])
        if "Finish submission of the findings already established" in stdin:
            evidence = state["evidence"]
        elif "Adversarially review the current plan" in stdin:
            evidence = {"planVersion":snapshot["plan"]["version"], "findings":[]}
            if snapshot["plan"]["version"] <= int(os.environ.get("YINZ_SMOKE_BLOCKED_VERSIONS", "0")):
                evidence["findings"] = [{"id":"coverage", "severity":"high", "description":"Cover the missing caller", "requirement":"Marker covers caller", "mechanism":"Caller can miss the marker", "evidence":["marker.txt"]}]
        elif "Develop or revise a concrete implementation plan" in stdin:
            evidence = {"summary":"Add the disposable fixture marker.","steps":[{"id":"marker","description":"Create marker.txt","files":["marker.txt"]}],"testingStrategy":"Inspect marker contents with focused fixture checks.","checks":[{"id":"marker","command":["grep","-q","smoke marker","marker.txt"],"covers":["marker"]}],"assumptions":["The marker is the complete fixture requirement."]}
            if "This is a targeted revision" in stdin:
                evidence["revision"] = {"summary":"Kept marker scope and checked the caller", "responses":[{"findingId":"coverage", "status":"disputed", "rationale":"The fixture has only one caller", "evidence":["marker.txt"]}], "acceptedDecisions":["One fixture marker is sufficient"]}
        else:
            assert "Investigate the issue" in stdin
            evidence = {"classification":"maintenance","summary":"Add a smoke marker; acceptance requires the marker on local main."}
        target = json.loads(stdin.split("Swamp submission target:\\n", 1)[1].split("\\n\\n", 1)[0])
        envelope = {"taskId":target["taskId"], "assignmentDigest":target["assignmentDigest"], "role":target["role"], "evidence":evidence}
        if not os.environ.get("YINZ_SMOKE_NO_SUBMISSION"):
            submit_env = {**os.environ, "SWAMP_LOCK_TIMEOUT_MS":"3000"}
            submitted = subprocess.run(["swamp", "behavior", "run", target["behaviorName"], "submit", "--repo-dir", target["controlDir"], "--stdin", "--literal-inputs", "--json", "--no-telemetry"], input=json.dumps(envelope), text=True, capture_output=True, env=submit_env, timeout=30)
            assert submitted.returncode == 0, submitted.stderr + submitted.stdout
            accepted = subprocess.run(["swamp", "data", "query", 'behavior("' + target["behaviorName"] + '") && name == "submission"', "--repo-dir", target["controlDir"], "--select", "content", "--single", "--json", "--no-telemetry"], text=True, capture_output=True, env=submit_env, timeout=30)
            assert accepted.returncode == 0, accepted.stderr + accepted.stdout
            receipt = json.loads(accepted.stdout)
            assert receipt["status"] == "accepted" and receipt["receipt"]["taskId"] == target["taskId"], receipt
        state.update(submission=args[args.index("--submission")+1], evidence=evidence, evidence_target=target)
        state_file.write_text(json.dumps(state))
        if os.environ.get("YINZ_SMOKE_DROP_ACK") and "classification" in evidence:
            raise SystemExit("Lost provider response after Swamp accepted the evidence")
        response.update(outcome="completed", acknowledged=True, submission_id=state["submission"], turn_id="smoke-turn", message=os.environ.get("YINZ_SMOKE_FINAL_TEXT", '{"findings":['))
        if os.environ.get("YINZ_SMOKE_TIMEOUT") and "classification" in evidence:
            response.update(outcome="timed_out", message=None)
            response["agents"][0]["agent"]["state"] = "Busy"
            response["agents"][0]["not_ready_reason"] = "unresolved_turn"
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

    def lifecycle(self, method, inputs=None, cwd=None):
        command = [sys.executable, str(self.repo / "scripts" / "issue-lifecycle"), "--control", str(self.repo), "--item", str(self.item_id), method]
        if inputs is not None:
            command.extend(["--input-file", "-"])
        return self.command(command, stdin=json.dumps(inputs) if inputs is not None else None, cwd=cwd)

    def state(self):
        document = json.loads(self.lifecycle("status"))
        records = document["results"]
        states = [record for record in records if "issue" in record and "phase" in record]
        self.assertEqual(len(states), 1, document)
        return states[0]

    def test_installed_foreground_planning_handoff_and_resume(self):
        self.command(["swamp", "workflow", "validate", "yinz-issue-drive", "--json"])
        worktree = self.root / "origin"
        self.git(self.repo, "worktree", "add", "-b", "origin", str(worktree))
        nested = worktree / "nested"
        nested.mkdir()
        self.lifecycle("start", cwd=nested)
        self.lifecycle("start")  # A later caller in control cannot replace the origin.
        self.assertEqual(self.state()["workerWorktree"], str(worktree.resolve()))
        self.env["YINZ_SMOKE_TIMEOUT"] = "1"
        handoff = json.loads(self.lifecycle("drive", {"agent": {"session": "smoke", "kind": "claude"}}))
        self.assertEqual(handoff["decision"], "approve_plan")
        self.assertEqual(handoff["planVersion"], 1)
        self.assertEqual(handoff["state"]["planReview"], {"planVersion": 1, "findings": []})
        for label in ("Proposed behavior", "Implementation steps", "Tests:", "Assumptions:", "Plan review findings", "controlling agent must collect and present"):
            self.assertIn(label, handoff["presentation"])
        self.assertNotIn("approval", self.state())
        self.assertNotIn("implementation", self.state())
        calls = [json.loads(line) for line in self.yinz_log.read_text().splitlines()]
        self.assertEqual([call["args"][3] for call in calls], ["list-panes", "spawn", "wait", "prompt", "wait", "list-panes", "close-completed"] + ["list-panes", "spawn", "wait", "prompt", "list-panes", "close-completed"] * 2)
        spawns = [call["args"] for call in calls if call["args"][3] == "spawn"]
        self.assertTrue(all(args[args.index("--cwd") + 1] == str(worktree.resolve()) for args in spawns))
        prompts = [call for call in calls if call["args"][3] == "prompt"]
        observed = calls[4]["args"]
        submitted = prompts[0]["args"]
        self.assertEqual(observed[observed.index("--submission") + 1], submitted[submitted.index("--submission") + 1])
        resumed = json.loads(self.lifecycle("drive"))
        self.assertEqual(resumed["id"], handoff["id"])
        self.assertEqual(len(self.yinz_log.read_text().splitlines()), len(calls))
        self.lifecycle("approve", {"planVersion": 1, "request": "Approve this disposable fixture plan"})
        prepared = json.loads(self.lifecycle("drive"))
        self.assertEqual(prepared["decision"], "prepare_worktree")
        self.assertEqual(len(self.yinz_log.read_text().splitlines()), len(calls))

    def start_submission_fixture(self):
        worktree = self.root / "typed-worker"
        self.git(self.repo, "worktree", "add", "-b", "typed-worker", str(worktree))
        self.lifecycle("start", cwd=worktree)
        return worktree

    def test_installed_submission_boundary_and_concurrent_replay(self):
        from concurrent.futures import ThreadPoolExecutor

        self.start_submission_fixture()
        self.env["YINZ_SMOKE_NO_SUBMISSION"] = "1"
        handoff = json.loads(self.lifecycle("drive", {"agent": {"session": "smoke", "kind": "claude"}}))
        self.assertEqual(handoff["decision"], "blocked")
        self.assertIn("submission_required", handoff["reason"])
        worker = json.loads(self.yinz_state.read_text())
        target = worker["evidence_target"]
        command = ["swamp", "behavior", "run", target["behaviorName"], "submit", "--repo-dir", str(self.repo), "--stdin", "--literal-inputs", "--json", "--no-telemetry"]
        envelope = {"taskId": target["taskId"], "assignmentDigest": target["assignmentDigest"], "role": target["role"], "evidence": worker["evidence"]}
        def submit(body):
            return subprocess.run(command, input=body, cwd=self.repo, env=self.env, text=True, capture_output=True, timeout=30)
        for body in ('{"evidence":[', json.dumps({**envelope, "evidence": {"summary": "Missing classification"}})):
            rejected = submit(body)
            self.assertNotEqual(rejected.returncode, 0, rejected.stdout)
        query = ["swamp", "data", "query", f'behavior("{target["behaviorName"]}") && name == "submission"', "--select", "content", "--single", "--json"]
        self.assertEqual(json.loads(self.command(query))["status"], "open")
        # Real independent CLI processes contend on the same Swamp behavior lock.
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(submit, [json.dumps(envelope)] * 2))
        for result in results:
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        original = json.loads(self.command(query))
        self.assertEqual(original["status"], "accepted")
        # Losing an acknowledgement does not require another provider prompt.
        replay = submit(json.dumps(envelope))
        self.assertEqual(replay.returncode, 0, replay.stderr + replay.stdout)
        self.assertEqual(json.loads(self.command(query)), original)
        conflict = submit(json.dumps({**envelope, "evidence": {**worker["evidence"], "summary": "Different findings"}}))
        self.assertNotEqual(conflict.returncode, 0)
        self.assertIn("Submission conflict", conflict.stderr + conflict.stdout)
        self.lifecycle("resume-agent")
        self.assertEqual(self.state()["phase"], "classified")
        calls = [json.loads(line) for line in self.yinz_log.read_text().splitlines()]
        self.assertEqual(sum(call["args"][3] == "prompt" for call in calls), 1)
        self.assertEqual(sum(call["args"][3] == "spawn" for call in calls), 1)

    def test_installed_submission_only_continuation_uses_retained_worker(self):
        self.start_submission_fixture()
        self.env["YINZ_SMOKE_NO_SUBMISSION"] = "1"
        handoff = json.loads(self.lifecycle("drive", {"agent": {"session": "smoke", "kind": "claude"}}))
        self.assertEqual(handoff["decision"], "blocked")
        task_id = handoff["assignment"]["id"]
        del self.env["YINZ_SMOKE_NO_SUBMISSION"]
        self.lifecycle("continue-agent", {"taskId": task_id, "request": "Submit the findings already established, without repeating the investigation"})
        self.assertEqual(self.state()["phase"], "classified")
        completed = json.loads(self.lifecycle("drive"))
        self.assertEqual(completed["decision"], "approve_plan")
        calls = [json.loads(line) for line in self.yinz_log.read_text().splitlines()]
        prompts = [call for call in calls if call["args"][3] == "prompt"]
        self.assertEqual(len(prompts), 4)
        self.assertEqual(sum(call["args"][3] == "spawn" for call in calls), 3)
        self.assertIn("Finish submission of the findings already established", prompts[1]["stdin"])
        self.assertNotIn("Investigate the issue against the source", prompts[1]["stdin"])
        self.assertEqual(prompts[0]["args"][4], prompts[1]["args"][4])
        self.assertNotEqual(prompts[0]["args"][prompts[0]["args"].index("--submission") + 1], prompts[1]["args"][prompts[1]["args"].index("--submission") + 1])

    def test_installed_accepted_evidence_survives_lost_provider_response(self):
        self.start_submission_fixture()
        self.env["YINZ_SMOKE_DROP_ACK"] = "1"
        handoff = json.loads(self.lifecycle("drive", {"agent": {"session": "smoke", "kind": "claude"}}))
        self.assertEqual(handoff["decision"], "blocked")
        records = json.loads(self.lifecycle("status"))["results"]
        self.assertTrue(any(record.get("status") == "accepted" and "receipt" in record for record in records))
        recovered = json.loads(self.lifecycle("drive"))
        self.assertEqual(recovered["decision"], "approve_plan")
        calls = [json.loads(line) for line in self.yinz_log.read_text().splitlines()]
        self.assertEqual(sum(call["args"][3] == "prompt" for call in calls), 3)
        self.assertEqual(sum(call["args"][3] == "spawn" for call in calls), 3)

    def test_installed_revision_approval_is_single_use(self):
        worktree = self.root / "revision"
        self.git(self.repo, "worktree", "add", "-b", "revision", str(worktree))
        self.lifecycle("start", cwd=worktree)
        self.env["YINZ_SMOKE_BLOCKED_VERSIONS"] = "3"
        handoff = json.loads(self.lifecycle("drive", {"agent": {"session": "smoke", "kind": "claude"}}))
        self.assertEqual(handoff["decision"], "authorize_plan_revision")
        self.assertEqual(handoff["planVersion"], 2)
        self.assertEqual(handoff["proposedPlanVersion"], 3)
        self.assertIn("Cover the missing caller", handoff["presentation"])
        calls = self.yinz_log.read_text()
        self.assertEqual(json.loads(self.lifecycle("drive"))["id"], handoff["id"])
        self.assertEqual(self.yinz_log.read_text(), calls)
        approval = {"handoffId": handoff["id"], "request": "Revise this disposable plan once"}
        revised = json.loads(self.lifecycle("drive", {"revisionApproval": approval}))
        self.assertEqual(revised["decision"], "authorize_plan_revision")
        self.assertEqual(revised["planVersion"], 3)
        self.assertEqual(revised["proposedPlanVersion"], 4)
        self.assertNotEqual(revised["id"], handoff["id"])
        calls = self.yinz_log.read_text()
        self.assertEqual(json.loads(self.lifecycle("drive"))["id"], revised["id"])
        stale = json.loads(self.lifecycle("drive", {"revisionApproval": approval}))
        self.assertEqual(stale["decision"], "blocked")
        self.assertIn("stale", stale["reason"])
        self.assertEqual(self.yinz_log.read_text(), calls)
        proposal = json.loads(self.lifecycle("drive"))
        self.lifecycle("adjudicate-plan", {
            "planVersion": 3, "reviewFingerprint": proposal["reviewFingerprint"],
            "dispositions": [{"findingId": "coverage", "disposition": "rejected",
                              "rationale": "Inspected fixture source: marker is the entire caller contract",
                              "evidence": ["marker.txt and the smoke fixture acceptance requirement"]}],
        })
        reviewed = json.loads(self.lifecycle("drive"))
        self.assertEqual(reviewed["decision"], "approve_plan")
        self.assertEqual(reviewed["planVersion"], 3)
        self.assertNotIn("approval", self.state())
        self.assertNotIn("implementation", self.state())
        prompts = [json.loads(line) for line in self.yinz_log.read_text().splitlines()
                   if json.loads(line)["args"][3] == "prompt"]
        self.assertEqual(len(prompts), 7)
        self.assertIn(approval["request"], prompts[5]["stdin"])
        self.assertIn("Kept marker scope", reviewed["presentation"])
        self.assertIn("Inspected fixture source", reviewed["presentation"])
        self.assertEqual(reviewed["state"]["planReview"]["findings"][0]["severity"], "high")
        calls = self.yinz_log.read_text()
        self.assertEqual(json.loads(self.lifecycle("drive"))["id"], reviewed["id"])
        self.assertEqual(self.yinz_log.read_text(), calls)
        self.lifecycle("approve", {"planVersion": 3, "request": "Implement the reviewed fixture plan"})
        self.assertEqual(json.loads(self.lifecycle("drive"))["decision"], "prepare_worktree")

    def test_installed_dispatch_agents_rebase_ci_merge_and_agenda_completion(self):
        self.command(["swamp", "workflow", "validate", "yinz-issue-start", "--json"])
        # A direct workflow call cannot bypass the configured vault workspace.
        rejected = subprocess.run(
            ["swamp", "workflow", "run", "yinz-issue-start", "--stdin"],
            input=json.dumps({"workspace": "other", "itemId": self.item_id,
                              "issueName": f"issue-other-{self.item_id}", "workerWorktree": str(self.repo)}),
            cwd=self.repo, env=self.env, text=True, capture_output=True, timeout=30,
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("Vault Agenda workspace does not match", rejected.stdout + rejected.stderr)
        worktree = self.root / "issue-worktree"
        self.git(self.repo, "worktree", "add", "-b", "smoke-issue", str(worktree))
        self.lifecycle("start", cwd=worktree)
        self.assertEqual(self.state()["phase"], "triaging")
        self.lifecycle("agent", {"role": "triage", "session": "smoke", "kind": "claude"})
        state = self.state()
        self.assertEqual(state["phase"], "classified")
        self.assertEqual(state["classification"]["classification"], "maintenance")
        calls = [json.loads(line) for line in self.yinz_log.read_text().splitlines()]
        self.assertEqual([call["args"][3] for call in calls], ["list-panes", "spawn", "wait", "prompt", "list-panes", "close-completed"])
        self.assertIn("Submit evidence through Swamp before ending this task", next(call["stdin"] for call in calls if call["args"][3] == "prompt"))
        records = json.loads(self.lifecycle("status"))["results"]
        task = next(record for record in records if "behaviorName" in record)
        restart = {"taskId": task["id"], "role": "triage", "session": "smoke",
                   "kind": "claude", "request": "Restart triage in the disposable fixture"}
        self.env["YINZ_SMOKE_CONTROL_TAB"] = "8"
        self.lifecycle("restart-agent", restart)
        self.lifecycle("restart-agent", restart)
        self.lifecycle("resume-rollback")
        calls = [json.loads(line) for line in self.yinz_log.read_text().splitlines()]
        self.assertEqual([call["args"][3] for call in calls], ["list-panes", "spawn", "wait", "prompt", "list-panes", "close-completed"] * 2)
        self.lifecycle("plan", {
            "summary": "Add the disposable fixture's marker.",
            "steps": [{"id": "marker", "description": "Create marker.txt", "files": ["marker.txt"]}],
            "testingStrategy": "Exercise the merge protocol using an explicit Cargo stub; this is not Rust verification.",
            "checks": [{"id": "marker", "command": ["grep", "-c", "smoke marker", "marker.txt"], "covers": ["marker"]}],
        })
        self.lifecycle("review-plan", {"planVersion": 1, "findings": []})
        self.lifecycle("approve", {"planVersion": 1, "request": "Approve the disposable smoke fixture plan."})
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
        ready = {"commit": original, "mergeRequest": "Merge the disposable smoke fixture locally."}
        launcher = [sys.executable, str(self.repo / "scripts" / "issue-lifecycle"), "--control", str(self.repo), "--item", str(self.item_id)]
        refused = subprocess.run([*launcher, "ready", "--input-file", "-"], input=json.dumps(ready), cwd=self.repo, env=self.env, text=True, capture_output=True, timeout=120)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("Run verify for the reviewed commit", refused.stdout + refused.stderr)
        verification = json.loads(self.lifecycle("verify"))
        self.assertEqual(verification["status"], "passed")
        self.assertEqual(verification["commit"], original)
        self.assertEqual(self.state()["verification"], verification)
        [check] = verification["checks"]
        self.assertEqual((check["id"], check["exitCode"]), ("marker", 0))
        name = f"issue-lifecycle-test-{self.item_id}"
        log = json.loads(self.command(["swamp", "data", "query", f"modelName == {json.dumps(name)} && name == {json.dumps(check['log'])}", "--select", "content", "--json"]))
        self.assertEqual(len(log["results"]), 1)
        self.assertIn("$ grep -c 'smoke marker' marker.txt\n1\nexit 0 after", log["results"][0])
        self.lifecycle("ready", ready)
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
        receipt = self.state()["receipt"]
        self.lifecycle("rollback", {"step": "summarize", "request": "Repeat close-out in the fixture"})
        self.assertEqual(self.state()["phase"], "merged")
        self.assertEqual(self.state()["receipt"], receipt)
        self.assertFalse(self.agenda("item", "show", str(self.item_id))["item"]["completed"])
        self.lifecycle("merge")
        self.assertEqual(len(self.cargo_log.read_text().splitlines()), 1)
        self.lifecycle("summarize", {"outcomeMet": True, "summary": "Close-out replay verified."})
        self.lifecycle("rollback", {"step": "plan", "request": "Replan the fixture after completion"})
        self.assertEqual(self.state()["phase"], "classified")
        self.assertEqual(self.state()["planVersion"], 1)
        self.assertNotIn("plan", self.state())
        self.assertFalse(self.agenda("item", "show", str(self.item_id))["item"]["completed"])
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), receipt["mergedCommit"])
        self.assertEqual(self.git(self.repo, "status", "--short"), "")

    def reviewed_and_ready(self, worktree, request):
        commit = self.git(worktree, "rev-parse", "HEAD")
        self.lifecycle("review-code", {
            "planVersion": 1, "commit": commit,
            "steps": [{"id": "marker", "status": "implemented"}], "findings": [],
        })
        self.assertEqual(json.loads(self.lifecycle("verify"))["status"], "passed")
        self.lifecycle("ready", {"commit": commit, "mergeRequest": request})
        return commit

    def test_installed_failed_merge_gate_is_recorded_blocked_and_kept_after_a_later_pass(self):
        self.env["YINZ_SMOKE_CARGO_FAIL_ONCE"] = str(self.root / "cargo-failed-once")
        worktree = self.root / "gate-worktree"
        self.git(self.repo, "worktree", "add", "-b", "gate-issue", str(worktree))
        self.lifecycle("start", cwd=worktree)
        self.lifecycle("triage", {"classification": "maintenance", "summary": "Add the smoke marker."})
        self.lifecycle("plan", {
            "summary": "Add the disposable fixture's marker.",
            "steps": [{"id": "marker", "description": "Create marker.txt", "files": ["marker.txt"]}],
            "testingStrategy": "Exercise merge-gate failure recording with an explicit Cargo stub.",
            "checks": [{"id": "marker", "command": ["grep", "-c", "smoke marker", "marker.txt"], "covers": ["marker"]}],
        })
        self.lifecycle("review-plan", {"planVersion": 1, "findings": []})
        self.lifecycle("approve", {"planVersion": 1, "request": "Approve the disposable smoke fixture plan."})
        self.lifecycle("implement", {"worktree": str(worktree), "branch": "gate-issue"})
        (worktree / "marker.txt").write_text("smoke marker\n")
        self.git(worktree, "add", "marker.txt")
        self.git(worktree, "commit", "-m", "add smoke marker")
        self.reviewed_and_ready(worktree, "Merge the disposable smoke fixture locally.")
        launcher = [sys.executable, str(self.repo / "scripts" / "issue-lifecycle"), "--control", str(self.repo),
                    "--item", str(self.item_id)]
        failed_run = subprocess.run([*launcher, "merge"], cwd=self.repo, env=self.env, text=True,
                                    capture_output=True, timeout=120)
        self.assertNotEqual(failed_run.returncode, 0)
        state = self.state()
        [failure] = state["mergeFailures"]
        self.assertEqual(failure["attemptId"], state["attempt"]["id"])
        self.assertEqual(failure["phases"], [{"name": "format --check", "status": "passed"},
                                             {"name": "test", "status": "failed"}])
        self.assertEqual(failure["failedTests"], ["smoke-fixture tests::flaky_marker"])
        self.assertEqual(failure["summary"], "phases failed: test; tests: smoke-fixture tests::flaky_marker; exit 101")
        log = Path(failure["log"])
        self.assertEqual(log, self.repo / ".git" / "yinz-lifecycle" / "merges" / failure["attemptId"] / "ci-check.log")
        self.assertIn("tests::flaky_marker", log.read_text())
        receipt = json.loads(log.parent.with_suffix(".json").read_text())
        self.assertEqual((receipt["status"], receipt["ci"]["status"]), ("failed", "failed"))
        self.assertEqual(receipt["ci"]["phases"], failure["phases"])
        # The launcher points the operator at the evidence.
        for text in (f"Merge attempt {failure['attemptId']} failed: {failure['summary']}", f"Log: {log}",
                     "- [ ] test", "Failed tests: smoke-fixture tests::flaky_marker"):
            self.assertIn(text, failed_run.stderr)
        blocked = json.loads(self.lifecycle("drive"))
        self.assertEqual(blocked["decision"], "blocked")
        self.assertIn(f"log: {log}", blocked["reason"])
        self.assertIn("Merge attempts:", blocked["presentation"])
        self.lifecycle("repair", {"reason": "Fix the flaky marker test before merging again."})
        repaired = self.state()
        self.assertEqual(repaired["phase"], "implementing")
        self.assertIn(str(log), repaired["lastFailure"])
        (worktree / "marker.txt").write_text("smoke marker\nfixed\n")
        self.git(worktree, "commit", "-am", "fix the flaky marker")
        self.reviewed_and_ready(worktree, "Merge the repaired smoke fixture locally.")
        self.lifecycle("merge")
        merged = self.state()
        self.assertEqual(merged["phase"], "merged")
        self.assertEqual(merged["mergeFailures"], [failure])
        self.assertEqual(merged["receipt"]["previousAttempts"],
                         [{"attemptId": failure["attemptId"], "summary": failure["summary"], "log": str(log)}])
        self.assertNotEqual(merged["receipt"]["attemptId"], failure["attemptId"])
        self.assertEqual(len(self.cargo_log.read_text().splitlines()), 2)
        summary = json.loads(self.lifecycle("drive"))
        self.assertEqual(summary["decision"], "summarize")
        self.assertIn(f"- {failure['attemptId']} at ", summary["presentation"])
        self.assertIn(f"- {merged['receipt']['attemptId']}: merged", summary["presentation"])


if __name__ == "__main__":
    unittest.main()
