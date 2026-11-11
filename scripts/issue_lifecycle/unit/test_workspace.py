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
    print(os.environ.get("TEST_DATA_JSON", json.dumps({{"results": [{{"status": "current", "presentation": "Fixture handoff"}}]}})))
elif args[:3] == ["workflow", "history", "search"]:
    print(os.environ.get("TEST_HISTORY_JSON", '{{"results": []}}'))
else:
    print("{{}}")
''')
        stub.chmod(0o755)
        self.env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
                    "TEST_VAULT_LOG": str(self.log), "TEST_VAULT_VALUE": str(self.value),
                    "SWAMP_SERVER_URL": "http://must-not-use.invalid", "SWAMP_REPO_DIR": "/wrong"}

    def launch(self, command="status", *args, cwd=None):
        return subprocess.run([sys.executable, str(LAUNCHER), "--control", str(self.control),
                               "--item", "7", command, *args], cwd=cwd or self.subdir, env=self.env,
                              capture_output=True, text=True, timeout=10)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

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
        diagnostics = json.loads(result.stdout)["diagnostics"]
        self.assertEqual([entry["runId"] for entry in diagnostics], ["original-run"])
        self.assertEqual([entry["args"][:2] for entry in self.calls()],
                         [["vault", "read-secret"], ["data", "query"], ["workflow", "history"]])

    def test_closeout_dispatch_is_thin_and_recovery_does_not_replace_target(self):
        self.env.update(YINZ_SESSION_NAME="recovery", YINZ_PANE_ID="99")
        for method in ("closeout", "resume-closeout"):
            with self.subTest(method=method):
                result = self.launch(method)
                self.assertEqual(result.returncode, 0, result.stderr)
                call = self.calls()[-1]
                self.assertEqual(call["args"][:4], ["behavior", "run", "issue-work-7", method.replace("-", "_")])
                self.assertEqual(call["input"], {})

    def test_start_passes_vault_workspace_as_issue_identity(self):
        result = self.launch("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls()[-1]["input"], {"workspace": "work", "itemId": 7, "issueName": "issue-work-7",
                                                   "workerWorktree": str(self.origin.resolve())})
        self.assertTrue(all(call["cwd"] == str(self.control.resolve()) for call in self.calls()))
        self.assertTrue(all(call["args"][call["args"].index("--repo-dir") + 1] == str(self.control.resolve())
                            for call in self.calls()))

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
                self.assertEqual(self.calls()[-1]["input"]["agent"], expected)

    def test_start_without_control_default_keeps_explicit_flow_or_uses_input(self):
        self.write_agent_config({"agent": {"kind": "claude"}}, self.origin)
        result = self.launch("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("agent", self.calls()[-1]["input"])
        inputs = self.root / "start.json"
        inputs.write_text(json.dumps({"agent": {"kind": "agy", "model": None}}))
        result = self.launch("start", "--input-file", str(inputs))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls()[-1]["input"]["agent"], {"kind": "agy"})

    def test_start_rejects_invalid_config_and_inputs_before_dispatch(self):
        for config in ([], None, {"agents": {}}, {"agent": None}, {"agent": {}},
                       {"agent": {"kind": "unknown"}}, {"agent": {"kind": ["codex"]}},
                       {"agent": {"kind": "codex", "session": "dev"}},
                       {"agent": {"kind": "codex", "model": 1}},
                       {"agent": {"kind": "codex", "effort": " "}}):
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
        self.assertIn("start must be invoked from a Git worktree", result.stderr)
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
