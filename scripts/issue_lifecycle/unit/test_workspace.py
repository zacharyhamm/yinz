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
        self.root = Path(self.temporary.name).resolve()
        self.control = self.root / "control"
        self.control.mkdir()
        (self.control / ".swamp.yaml").write_text("tools: []\n")
        subprocess.run(["git", "init", "--initial-branch=main", str(self.control)],
                       check=True, capture_output=True)
        subprocess.run(["git", "-C", str(self.control), "-c", "user.name=Test",
                        "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
                        "commit", "--allow-empty", "-m", "Fixture"], check=True, capture_output=True)
        self.origin = self.root / "feature"
        subprocess.run(["git", "-C", str(self.control), "worktree", "add", "-b", "feature",
                        str(self.origin)], check=True, capture_output=True)
        self.subdir = self.origin / "nested"
        self.subdir.mkdir()
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
    # TEST_QUEUE holds per-resource records, each answering one query in order.
    queue_file = pathlib.Path(os.environ.get("TEST_QUEUE", "/nonexistent"))
    queue = json.loads(queue_file.read_text()) if queue_file.exists() else {{}}
    resource = next((key for key in queue if f"name == {{json.dumps(key)}}" in args[2] and queue[key]), None)
    log_path = pathlib.Path(os.environ["TEST_VAULT_LOG"])
    started = [entry["input"] for entry in map(json.loads, log_path.read_text().splitlines())
               if entry["args"][:3] == ["workflow", "run", "yinz-issue-start"]]
    if resource:
        print(json.dumps({{"results": [queue[resource].pop(0)]}}))
        queue_file.write_text(json.dumps(queue))
    elif started and 'name == "state"' in args[2] and "TEST_DATA_JSON" not in os.environ:
        # Answer the launcher's post-start query with the origin the start recorded.
        origin = started[-1]
        worktree = origin["workerWorktree"] or os.environ["TEST_CANONICAL_ROOT"] + f"/.worktrees/issue-{{origin['itemId']}}"
        print(json.dumps({{"results": [{{"issue": {{}}, "phase": "triaging", "workerWorktree": worktree}}]}}))
    else:
        print(os.environ.get("TEST_DATA_JSON", json.dumps({{"results": [{{"status": "current", "presentation": "Fixture handoff"}}]}})))
elif args[:3] == ["workflow", "history", "search"]:
    print(os.environ.get("TEST_HISTORY_JSON", '{{"results": []}}'))
else:
    print("{{}}")
    run = {{"behavior": f"behavior:{{args[3]}}", "workflow": f"workflow:{{args[2]}}"}}.get(args[0]) if args[1:2] == ["run"] else None
    sys.exit(json.loads(os.environ.get("TEST_EXIT", "{{}}")).get(run, 0))
''')
        stub.chmod(0o755)
        self.env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
                    "TEST_VAULT_LOG": str(self.log), "TEST_VAULT_VALUE": str(self.value),
                    "TEST_CANONICAL_ROOT": str(self.control),
                    "SWAMP_SERVER_URL": "http://must-not-use.invalid", "SWAMP_REPO_DIR": "/wrong"}

    def launch(self, command="status", *args, cwd=None, stdin=None):
        return subprocess.run([sys.executable, str(LAUNCHER), "--control", str(self.control),
                               "--item", "7", command, *args], cwd=cwd or self.subdir, env=self.env,
                              input=stdin, capture_output=True, text=True, timeout=10)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def runs(self):
        """(kind, name, input) for each behavior or workflow run, in order."""
        return [(call["args"][0], call["args"][3] if call["args"][0] == "behavior" else call["args"][2], call["input"])
                for call in self.calls() if call["args"][1:2] == ["run"]]

    def test_status_uses_control_vault_without_workspace_argument(self):
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        self.assertEqual(len(calls), 3)
        self.assertIn("issue-work-7", calls[1]["args"][2])
        self.assertTrue(all(call["cwd"] == str(self.control) for call in calls))
        self.assertTrue(all(call["args"][call["args"].index("--repo-dir") + 1] == str(self.control) for call in calls))

    def test_status_reports_exact_inconsistent_run_without_mutation(self):
        self.env["TEST_DATA_JSON"] = json.dumps({"results": [{"issue": {}, "phase": "done"}]})
        self.env["TEST_HISTORY_JSON"] = json.dumps({"results": [
            {"runId": "original-run", "status": "running", "inputs": {"issueName": "issue-work-7"}},
            {"runId": "other-run", "status": "running", "inputs": {"issueName": "other"}},
        ]})
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        document = json.loads(result.stdout)
        diagnostics = document["diagnostics"]
        self.assertEqual([entry["runId"] for entry in diagnostics], ["original-run"])
        self.assertEqual(document["taskRecovery"]["kind"], "none")
        self.assertEqual([entry["args"][:2] for entry in self.calls()],
                         [["vault", "read-secret"], ["data", "query"], ["workflow", "history"]])

    def test_status_task_recovery_finishes_pending_rollback_first(self):
        state = {"issue": {}, "phase": "classified"}
        self.env["TEST_DATA_JSON"] = json.dumps({"results": [
            state, {"id": "recovery-1", "step": "plan", "request": "Replan", "status": "pending",
                    "before": state, "next": state}]})
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        recovery = json.loads(result.stdout)["taskRecovery"]
        self.assertEqual((recovery["kind"], recovery["command"]), ("finish_rollback", {"method": "drive", "input": {}}))
        self.assertFalse(recovery["requiresUserRequest"])

    def test_closeout_dispatch_is_thin_and_recovery_does_not_replace_target(self):
        self.env.update(YINZ_SESSION_NAME="recovery", YINZ_PANE_ID="99")
        for method in ("closeout", "resume-closeout"):
            with self.subTest(method=method):
                result = self.launch(method)
                self.assertEqual(result.returncode, 0, result.stderr)
                call = self.calls()[-1]
                self.assertEqual(call["args"][:4], ["behavior", "run", "issue-work-7", method.replace("-", "_")])
                self.assertEqual(call["input"], {})

    def start_input(self):
        return next(call["input"] for call in reversed(self.calls())
                    if call["args"][:3] == ["workflow", "run", "yinz-issue-start"])

    def test_start_passes_vault_workspace_as_issue_identity(self):
        result = self.launch("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.start_input(), {"workspace": "work", "itemId": 7, "issueName": "issue-work-7",
                                              "workerWorktree": str(self.origin.resolve()), "newWorktree": False})
        self.assertTrue(all(call["cwd"] == str(self.control.resolve()) for call in self.calls()))
        self.assertTrue(all(call["args"][call["args"].index("--repo-dir") + 1] == str(self.control.resolve())
                            for call in self.calls()))
        # The workflow's own output goes to stderr; stdout is the recorded origin, read back from state.
        self.assertEqual(json.loads(result.stdout), {"issueName": "issue-work-7", "phase": "triaging",
                                                     "workerWorktree": str(self.origin.resolve())})
        self.assertEqual(self.calls()[-1]["args"][:2], ["data", "query"])
        self.assertIn('name == "state"', self.calls()[-1]["args"][2])
        self.assertIn(f"Issue issue-work-7: workers use {self.origin.resolve()}", result.stderr)

    def test_start_on_main_implies_new_worktree_and_flag_overrides_feature_origin(self):
        canonical = f"{self.control}/.worktrees/issue-7"
        result = self.launch("start", cwd=self.control)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("invoked on main; start creates or adopts the canonical issue worktree", result.stderr)
        self.assertEqual(self.start_input(), {"workspace": "work", "itemId": 7, "issueName": "issue-work-7",
                                              "workerWorktree": "", "newWorktree": True})
        self.assertEqual(json.loads(result.stdout)["workerWorktree"], canonical)
        result = self.launch("start", "--new-worktree")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("invoked on main", result.stderr)
        self.assertEqual(self.start_input()["workerWorktree"], "")
        self.assertTrue(self.start_input()["newWorktree"])
        # The flag needs no Git checkout at all.
        self.env["GIT_CEILING_DIRECTORIES"] = str(self.root.parent)
        result = self.launch("start", "--new-worktree", cwd=self.root)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.start_input()["newWorktree"])

    def test_start_from_detached_head_keeps_explicit_origin(self):
        detached = self.root / "detached"
        subprocess.run(["git", "-C", str(self.control), "worktree", "add", "--detach", str(detached)],
                       check=True, capture_output=True)
        result = self.launch("start", cwd=detached)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.start_input()["workerWorktree"], self.start_input()["newWorktree"]),
                         (str(detached.resolve()), False))

    def test_new_worktree_flag_is_start_only(self):
        for command in ("drive", "status", "approve"):
            with self.subTest(command=command):
                result = self.launch(command, "--new-worktree")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("--new-worktree applies only to start", result.stderr)
                self.assertFalse(self.log.exists())

    def test_failed_or_stateless_start_prints_no_origin(self):
        self.env["TEST_EXIT"] = json.dumps({"workflow:yinz-issue-start": 4})
        result = self.launch("start")
        self.assertEqual(result.returncode, 4)
        self.assertEqual(result.stdout, "")
        self.assertFalse(any(call["args"][:2] == ["data", "query"] for call in self.calls()))
        self.env.pop("TEST_EXIT")
        self.env["TEST_DATA_JSON"] = json.dumps({"results": []})
        self.log.unlink()
        result = self.launch("start")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertIn("start recorded no lifecycle state", result.stderr)

    def write_agent_config(self, value, root=None):
        path = (root or self.control) / ".yinz" / "issue-lifecycle.json"
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps(value))
        return path

    def test_start_resolves_only_control_config_and_field_overrides(self):
        self.write_agent_config({"agent": {"kind": "claude", "model": "wrong"}}, self.origin)
        self.write_agent_config({"agent": {"kind": "codex", "model": "custom", "effort": "deep"}})
        for override, expected in (
            (None, {"kind": "codex", "model": "custom", "effort": "deep"}),
            ({"effort": "brief"}, {"kind": "codex", "model": "custom", "effort": "brief"}),
            ({"kind": "codex", "model": "other"}, {"kind": "codex", "model": "other", "effort": "deep"}),
            ({"kind": "claude"}, {"kind": "claude"}),
            ({"kind": "pi", "model": "pi-model"}, {"kind": "pi", "model": "pi-model"}),
            ({"model": None, "effort": None}, {"kind": "codex"}),
            ({"effort": None}, {"kind": "codex", "model": "custom"}),
        ):
            with self.subTest(override=override):
                args = []
                if override is not None:
                    inputs = self.root / "start.json"
                    inputs.write_text(json.dumps({"agent": override}))
                    args = ["--input-file", str(inputs)]
                result = self.launch("start", *args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.start_input()["agent"], expected)

    def test_start_without_control_default_keeps_explicit_flow_or_uses_input(self):
        self.write_agent_config({"agent": {"kind": "claude"}}, self.origin)
        result = self.launch("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("agent", self.start_input())
        inputs = self.root / "start.json"
        inputs.write_text(json.dumps({"agent": {"kind": "agy", "model": None}}))
        result = self.launch("start", "--input-file", str(inputs))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.start_input()["agent"], {"kind": "agy"})

    def test_start_rejects_invalid_config_and_inputs_before_dispatch(self):
        for config in ([], None, {"agents": {}}, {"agent": None}, {"agent": {}},
                       {"agent": {"kind": "unknown"}}, {"agent": {"kind": ["codex"]}},
                       {"agent": {"kind": "codex", "session": "dev"}},
                       {"agent": {"kind": "codex", "model": 1}},
                       {"agent": {"kind": "codex", "effort": " "}},
                       {"verification": {"baselineChecks": [{"id": "Bad"}]}}, {"verification": 1}):
            with self.subTest(config=config):
                path = self.write_agent_config(config)
                result = self.launch("start")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(str(path), result.stderr)
                self.assertFalse(self.log.exists())
        path.write_text("{")
        result = self.launch("start")
        self.assertIn("Invalid JSON", result.stderr)
        path.unlink()
        for value in ({"agent": {"model": "missing-kind"}}, {"agent": None},
                      {"agent": {"kind": "codex", "effort": False}}, {"other": 1}):
            with self.subTest(value=value):
                inputs = self.root / "start.json"
                inputs.write_text(json.dumps(value))
                result = self.launch("start", "--input-file", str(inputs))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("start", result.stderr)
                self.assertFalse(self.log.exists())

    def test_start_accepts_verification_section_and_rejects_invalid_one(self):
        verification = {"baselineChecks": [{"id": "fixture", "command": ["/usr/bin/true"]}], "rebaseProbe": False}
        self.write_agent_config({"agent": {"kind": "codex"}, "verification": verification})
        result = self.launch("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.start_input()["agent"], {"kind": "codex"})
        self.write_agent_config({"verification": {"baselineChecks": []}})
        result = self.launch("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("agent", self.start_input())
        dispatched = len(self.calls())
        for config in ({"agent": {"kind": "codex"}, "verification": {"rebaseProbe": "yes"}},
                       {"verification": {"baselineChecks": [{"id": "a", "command": ["x"]}] * 2}},
                       {"verification": {"timeout": 1}}):
            with self.subTest(config=config):
                path = self.write_agent_config(config)
                result = self.launch("start")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"{path}: verification", result.stderr)
                self.assertEqual(len(self.calls()), dispatched)
        self.write_agent_config({"agents": {}})
        result = self.launch("start")
        self.assertIn("only optional 'agent' and 'verification'", result.stderr)
        self.assertEqual(len(self.calls()), dispatched)

    def test_drive_does_not_reread_new_issue_configuration(self):
        path = self.write_agent_config({})
        path.write_text("invalid")
        result = self.launch("drive")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_later_invocations_do_not_supply_an_origin(self):
        self.launch("start")
        for command in ("drive", "agent", "restart-agent", "resume-agent", "resume-rollback"):
            with self.subTest(command=command):
                result = self.launch(command, cwd=self.control)
                self.assertEqual(result.returncode, 0, result.stderr)
                dispatched = next(call["input"] for call in reversed(self.calls()) if "input" in call)
                self.assertNotIn("workerWorktree", dispatched)
                self.assertNotIn("workerWorktree", dispatched.get("drive", {}))

    def test_start_outside_git_does_not_dispatch(self):
        # A temporary directory beneath an unrelated checkout must not be discovered.
        self.env["GIT_CEILING_DIRECTORIES"] = str(self.root.parent)
        result = self.launch("start", cwd=self.root)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("start must be invoked from a Git worktree, or pass --new-worktree", result.stderr)
        self.assertFalse(self.log.exists())

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

    def test_drive_forwards_one_revision_approval_and_rejects_unknown_inputs(self):
        approval = {"handoffId": "a6da10b0-c980-4b82-b920-000000000001", "request": "Revise this plan once"}
        inputs = self.root / "drive.json"
        inputs.write_text(json.dumps({"revisionApproval": approval}))
        result = self.launch("drive", "--input-file", str(inputs))
        self.assertEqual(result.returncode, 0, result.stderr)
        dispatched = next(call["input"] for call in self.calls() if "input" in call)
        self.assertEqual(dispatched["drive"]["revisionApproval"], approval)
        self.log.unlink()
        inputs.write_text(json.dumps({"approveAllRevisions": True}))
        result = self.launch("drive", "--input-file", str(inputs))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("revisionApproval", result.stderr)
        self.assertFalse(any("input" in call for call in self.calls()))

    HANDOFF = "a6da10b0-c980-4b82-b920-000000000002"

    def queue(self, **records):
        path = self.root / "queue.json"
        path.write_text(json.dumps(records))
        self.env["TEST_QUEUE"] = str(path)

    @staticmethod
    def handoff(decision, phase, **state):
        return {"id": decision, "status": "current", "decision": decision, "presentation": f"Presented {decision}",
                "state": {"phase": phase, **state}}

    def integrate(self):
        return self.handoff("integrate", "ready", mergeAuthorization={"commit": "c" * 40, "request": "Merge"})

    def test_chained_decision_records_then_drives_and_prints_one_handoff(self):
        self.env.update(YINZ_SESSION_NAME="dev", YINZ_PANE_ID="4")
        self.queue(handoff=[self.handoff("authorize_merge", "implementing")])
        result = self.launch("approve", "--handoff", self.HANDOFF, "--request", "Approve plan 1")
        self.assertEqual(result.returncode, 0, result.stderr)
        control = {"session": "dev", "paneId": "terminal_4"}
        self.assertEqual(self.runs(), [
            ("behavior", "approve", {"handoffId": self.HANDOFF, "request": "Approve plan 1"}),
            ("workflow", "yinz-issue-drive", {"issueName": "issue-work-7", "drive": {"control": control}}),
        ])
        self.assertEqual(json.loads(result.stdout)["decision"], "authorize_merge")
        self.assertIn("recorded approve; continuing with drive", result.stderr)
        self.assertIn("Presented authorize_merge", result.stderr)
        # The behavior's and workflow's own output never reaches stdout.
        self.assertEqual(result.stdout.count("\n"), 1)

    def test_no_drive_or_failed_decision_never_drives(self):
        result = self.launch("decide-review", "--handoff", self.HANDOFF, "--request", "Fix it",
                             "--repair", "review_code/a", "--ignore-remaining", "--no-drive")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.runs(), [("behavior", "decide_review", {
            "handoffId": self.HANDOFF, "request": "Fix it",
            "repair": [{"role": "review_code", "findingId": "a"}], "ignoreRemaining": True})])
        self.log.unlink()
        self.env["TEST_EXIT"] = json.dumps({"behavior:approve": 3})
        result = self.launch("approve", "--handoff", self.HANDOFF, "--request", "Approve")
        self.assertEqual(result.returncode, 3)
        self.assertEqual([run[:2] for run in self.runs()], [("behavior", "approve")])
        self.assertEqual(result.stdout, "")

    def test_drive_merges_an_authorized_integrate_once_then_drives_again(self):
        self.env.update(YINZ_SESSION_NAME="dev", YINZ_PANE_ID="4")
        summarize = self.handoff("summarize", "merged")
        self.queue(handoff=[self.integrate(), summarize, self.integrate()])
        agent = self.root / "drive.json"
        agent.write_text(json.dumps({"agent": {"kind": "claude", "session": "dev"}}))
        result = self.launch("drive", "--input-file", str(agent), "--skip-ci-request", "The user waived CI")
        self.assertEqual(result.returncode, 0, result.stderr)
        control = {"session": "dev", "paneId": "terminal_4"}
        self.assertEqual(self.runs(), [
            ("workflow", "yinz-issue-drive", {"issueName": "issue-work-7",
                                              "drive": {"agent": {"kind": "claude", "session": "dev"}, "control": control}}),
            ("workflow", "yinz-issue-merge", {"issueName": "issue-work-7", "skipCiRequest": "The user waived CI"}),
            ("workflow", "yinz-issue-drive", {"issueName": "issue-work-7", "drive": {"control": control}}),
        ])
        validations = [call["args"][2] for call in self.calls() if call["args"][:2] == ["workflow", "validate"]]
        self.assertEqual(validations, ["yinz-issue-drive", "yinz-issue-merge"])
        self.assertEqual(json.loads(result.stdout), summarize)
        self.assertIn("running merge (rebase, full CI gate, fast-forward)", result.stderr)
        self.assertNotIn("CI waiver was not used", result.stderr)

    def test_no_merge_and_unused_waiver_are_reported(self):
        self.queue(handoff=[self.integrate()])
        result = self.launch("ready", "--handoff", self.HANDOFF, "--request", "Merge it", "--no-merge",
                             "--skip-ci-request", "The user waived CI")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([run[:2] for run in self.runs()],
                         [("behavior", "ready"), ("workflow", "yinz-issue-drive")])
        self.assertEqual(self.runs()[0][2], {"handoffId": self.HANDOFF, "mergeRequest": "Merge it"})
        self.assertEqual(json.loads(result.stdout)["decision"], "integrate")
        self.assertIn("the CI waiver was not used; no merge ran", result.stderr)

    def test_non_merging_chained_commands_stop_at_an_authorized_integrate(self):
        for command in (["decide-review", "--handoff", self.HANDOFF, "--request", "Ignore the rest",
                         "--ignore-remaining"], ["resume-rollback"]):
            with self.subTest(command=command[0]):
                self.log.unlink(missing_ok=True)
                self.queue(handoff=[self.integrate()])
                result = self.launch(*command)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual([run[:2] for run in self.runs()],
                                 [("behavior", command[0].replace("-", "_")), ("workflow", "yinz-issue-drive")])
                self.assertEqual(json.loads(result.stdout)["decision"], "integrate")
                self.assertNotIn("running merge", result.stderr)

    def test_existing_attempt_is_never_merged_automatically(self):
        attempted = self.integrate()
        attempted["state"]["attempt"] = {"id": "earlier"}
        self.queue(handoff=[attempted])
        result = self.launch("drive")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([run[1] for run in self.runs()], ["yinz-issue-drive"])

    def test_failed_auto_merge_exits_with_its_code_and_prints_the_blocked_handoff(self):
        blocked = self.handoff("blocked", "ready", attempt={"id": "attempt-1"})
        failure = {"attemptId": "attempt-1", "summary": "phases failed: test", "phases": [], "logResource": "merge-log"}
        self.queue(handoff=[self.integrate(), blocked],
                   state=[{"issue": {}, "phase": "ready", "attempt": {"id": "attempt-1"}, "mergeFailures": [failure]}])
        self.env["TEST_EXIT"] = json.dumps({"workflow:yinz-issue-merge": 7})
        result = self.launch("drive")
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertEqual([run[1] for run in self.runs()], ["yinz-issue-drive", "yinz-issue-merge", "yinz-issue-drive"])
        self.assertIn("Merge attempt attempt-1 failed: phases failed: test", result.stderr)
        self.assertEqual(json.loads(result.stdout), blocked)

    def test_merge_command_drives_afterwards_without_a_second_merge(self):
        self.env.pop("YINZ_SESSION_NAME", None)
        self.env.pop("YINZ_PANE_ID", None)
        self.queue(handoff=[self.integrate()])
        result = self.launch("merge", "--skip-ci-request", "The user waived CI")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.runs(), [
            ("workflow", "yinz-issue-merge", {"issueName": "issue-work-7", "skipCiRequest": "The user waived CI"}),
            ("workflow", "yinz-issue-drive", {"issueName": "issue-work-7", "drive": {}}),
        ])
        self.assertEqual(json.loads(result.stdout)["decision"], "integrate")
        self.log.unlink()
        result = self.launch("merge", "--no-drive")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.runs(), [("workflow", "yinz-issue-merge", {"issueName": "issue-work-7"})])
        self.assertEqual(result.stdout, "{}\n")

    def test_request_file_reads_the_user_words(self):
        result = self.launch("approve", "--handoff", self.HANDOFF, "--request-file", "-", "--no-drive",
                             stdin="Approve plan 1, it's \"fine\"\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.runs()[0][2], {"handoffId": self.HANDOFF, "request": "Approve plan 1, it's \"fine\""})

    def test_invalid_decision_flags_never_dispatch(self):
        cases = (
            ("approve", "--handoff", self.HANDOFF, "--request", "x", "--skip-ci-request", "waive"),
            ("approve", "--handoff", self.HANDOFF, "--request", "x", "--repair", "review_code/a"),
            ("approve", "--handoff", "not-a-uuid", "--request", "x"),
            ("approve", "--request", "x", "--request-file", "-"),
            ("approve", "--input-file", "-", "--request-file", "-"),
            ("drive", "--handoff", self.HANDOFF),
            ("drive", "--no-drive"),
            ("closeout", "--no-drive"),
            ("implement", "--handoff", self.HANDOFF),
            ("decide-review", "--handoff", self.HANDOFF, "--request", "x", "--ignore", "ux/a"),
        )
        for args in cases:
            with self.subTest(args=args):
                result = self.launch(*args, stdin="{}")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertFalse(self.log.exists(), result.stderr)
