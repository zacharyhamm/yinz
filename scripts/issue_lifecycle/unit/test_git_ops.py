"""Exercise integration against real temporary Git worktrees and CLI stubs."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import uuid


RUNNER = Path(__file__).resolve().parents[1] / "git_ops.py"


class GitIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="yinz-lifecycle-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.events = self.root / "events.jsonl"
        self.agenda = self.root / "agenda.json"
        self.item = {
            "id": 7, "kind": "task", "text": "Fix café rendering", "note": "Preserve tabs.\nTest widths.",
            "completed": False, "blocked": False, "discarded_at": None,
            "assignments": [{"category": 30, "item": 7, "source": "explicit", "value": "standard"}],
        }
        self.agenda.write_text(json.dumps({"schema_version": 0, "item": self.item}))
        self.env = {
            **os.environ,
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "YINZ_TEST_AGENDA": str(self.agenda),
            "YINZ_TEST_EVENTS": str(self.events),
            "YINZ_TEST_CONTROL": str(self.root),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "CARGO_HOME": str(self.root / "cargo-home"),
        }
        self.env.pop("CARGO_TARGET_DIR", None)
        self.env.pop("CARGO_BUILD_TARGET_DIR", None)
        self.git(self.repo, "init", "--initial-branch=main")
        self.git(self.repo, "config", "user.email", "lifecycle@example.invalid")
        self.git(self.repo, "config", "user.name", "Lifecycle Test")
        (self.repo / ".gitignore").write_text("target/\n.worktrees/\n")
        (self.repo / "base.txt").write_text("base\n")
        self.git(self.repo, "add", ".")
        self.git(self.repo, "commit", "-m", "initial")
        self.stub("agenda", '''import json, os, pathlib, sys
assert sys.argv[1:6] == ["--workspace", "default", "--json", "item", "show"]
assert sys.argv[6] == "7"
print(pathlib.Path(os.environ["YINZ_TEST_AGENDA"]).read_text())
''')
        self.stub("cargo", '''import json, os, pathlib, subprocess, sys, time
assert sys.argv[1:] == ["x", "ci", "check"]
root = pathlib.Path(os.environ["YINZ_TEST_CONTROL"])
branch = subprocess.check_output(["git", "branch", "--show-current"], text=True).strip()
events = pathlib.Path(os.environ["YINZ_TEST_EVENTS"])
def event(name):
    with events.open("a") as file:
        file.write(json.dumps({"event": name, "branch": branch, "pid": os.getpid()}) + "\\n")
event("start")
if os.environ.get("YINZ_TEST_WAIT"):
    while not (root / (branch + ".release")).exists():
        time.sleep(0.01)
if os.environ.get("YINZ_TEST_MOVE_MAIN"):
    repo = pathlib.Path(os.environ["YINZ_TEST_MOVE_MAIN"])
    (repo / "manual.txt").write_text("manual concurrent change")
    subprocess.run(["git", "-C", str(repo), "add", "manual.txt"], check=True)
    subprocess.run(["git", "-C", str(repo), "commit", "-m", "manual"], check=True)
if os.environ.get("YINZ_TEST_DIRTY"):
    pathlib.Path("unexpected.txt").write_text("changed by test")
if os.environ.get("YINZ_TEST_EDIT_TRACKED"):
    pathlib.Path("base.txt").write_text("changed during CI")
if os.environ.get("YINZ_TEST_MOVE_HEAD"):
    subprocess.run(["git", "commit", "--allow-empty", "-m", "changed during CI"], check=True)
if os.environ.get("YINZ_TEST_SWITCH_BRANCH"):
    subprocess.run(["git", "switch", "-c", "replacement"], check=True)
if os.environ.get("YINZ_TEST_EDIT_ISSUE"):
    agenda = pathlib.Path(os.environ["YINZ_TEST_AGENDA"])
    item = json.loads(agenda.read_text())
    item["item"]["note"] = "New requirements"
    agenda.write_text(json.dumps(item))
if os.environ.get("YINZ_TEST_RECURRING_ISSUE"):
    agenda = pathlib.Path(os.environ["YINZ_TEST_AGENDA"])
    item = json.loads(agenda.read_text())
    item["item"]["assignments"].append({"category": 2, "item": 7, "source": "explicit", "value": {"date": {"temporal": {"kind": "floating"}, "recurrence": {"frequency": "daily"}}}})
    agenda.write_text(json.dumps(item))
event("end")
sys.exit(int(os.environ.get("YINZ_TEST_CI_EXIT", "0")))
''')

    def stub(self, name, source):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n" + source)
        path.chmod(0o755)

    def git(self, path, *args):
        return subprocess.check_output(["git", "-C", str(path), *args], env=self.env, stderr=subprocess.PIPE, text=True).strip()

    def run_request(self, request, *, env=None, success=True):
        result = subprocess.run(
            [sys.executable, str(RUNNER)], input=json.dumps(request), text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env={**self.env, **(env or {})}, timeout=30,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return json.loads(result.stdout)

    def start_request(self, request, env=None):
        process = subprocess.Popen(
            [sys.executable, str(RUNNER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, env={**self.env, **(env or {})},
        )
        process.stdin.write(json.dumps(request))
        process.stdin.close()
        process.stdin = None
        self.addCleanup(self.stop, process)
        return process

    @staticmethod
    def stop(process):
        if process.poll() is None:
            process.terminate()
        process.communicate(timeout=10)

    def wait_for(self, description, predicate):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(0.01)
        self.fail(f"Timed out waiting for {description}")

    def event_rows(self):
        if not self.events.exists():
            return []
        return [json.loads(line) for line in self.events.read_text().splitlines()]

    def issue_digest(self):
        item = {"id": 7, "workspace": "default", "text": self.item["text"], "note": self.item["note"]}
        return hashlib.sha256(json.dumps(item, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()

    def branch(self, name, *, nested=False):
        worktree = (self.repo / ".worktrees" if nested else self.root) / name
        self.run_request({"action": "prepare", "repoPath": str(self.repo), "worktree": str(worktree), "branch": name, "issueKey": "default:" + name})
        (worktree / f"{name}.txt").write_text(name)
        self.git(worktree, "add", ".")
        self.git(worktree, "commit", "-m", name)
        return worktree

    def request(self, worktree, **overrides):
        return {
            "action": "integrate", "repoPath": str(self.repo), "worktree": str(worktree),
            "expectedBranch": self.git(worktree, "branch", "--show-current"),
            "expectedHead": self.git(worktree, "rev-parse", "HEAD"),
            "attemptId": str(uuid.uuid4()), "issueKey": "default:" + worktree.name, "issueDigest": self.issue_digest(),
            "agendaWorkspace": "default", "agendaItemId": 7, **overrides,
        }

    def receipt(self, request):
        return self.run_request({"action": "receipt", "repoPath": str(self.repo), "attemptId": request["attemptId"]})

    def test_rebases_on_current_main_runs_full_ci_and_merges_exact_commit(self):
        worktree = self.branch("issue-a")
        original = self.git(worktree, "rev-parse", "HEAD")
        (self.repo / "main.txt").write_text("main advanced")
        self.git(self.repo, "add", ".")
        self.git(self.repo, "commit", "-m", "advance main")
        base = self.git(self.repo, "rev-parse", "HEAD")
        request = self.request(worktree)
        result = self.run_request(request)
        self.assertEqual(result["status"], "merged")
        self.assertEqual(result["baseCommit"], base)
        self.assertEqual(result["originalCommit"], original)
        self.assertNotEqual(result["rebasedCommit"], original)
        self.assertEqual(self.git(worktree, "rev-parse", "HEAD^"), base)
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), result["mergedCommit"])
        self.assertEqual(result["ci"], {"command": ["cargo", "x", "ci", "check"], "status": "passed"})
        self.assertEqual(self.run_request(request), result)
        self.assertEqual(len(self.event_rows()), 2, "retry must not rerun CI")

    def test_explicit_skip_is_recorded_without_running_ci(self):
        worktree = self.branch("issue-a")
        result = self.run_request(self.request(worktree, skipCiRequest="Skip the full CI check for this merge."))
        self.assertEqual(result["ci"]["status"], "skipped")
        self.assertIn("Skip", result["ci"]["request"])
        self.assertFalse(self.events.exists())

    def test_empty_skip_request_is_rejected(self):
        worktree = self.branch("issue-a")
        result = self.run_request(self.request(worktree, skipCiRequest=""), success=False)
        self.assertIn("explicit user request", result["error"])

    def test_same_attempt_cannot_change_the_skip_request(self):
        worktree = self.branch("issue-a")
        request = self.request(worktree, skipCiRequest="Skip this merge's full check.")
        self.run_request(request)
        result = self.run_request({**request, "skipCiRequest": "A different request"}, success=False)
        self.assertIn("skip request changed", result["error"])

    def test_recurring_or_malformed_live_agenda_item_is_rejected(self):
        worktree = self.branch("issue-a")
        item = {**self.item, "assignments": [{"value": {"date": {"recurrence": "daily"}}}]}
        self.agenda.write_text(json.dumps({"schema_version": 0, "item": item}))
        result = self.run_request(self.request(worktree), success=False)
        self.assertIn("Recurring", result["error"])
        item = {**self.item, "completed": "false"}
        self.agenda.write_text(json.dumps({"schema_version": 0, "item": item}))
        result = self.run_request(self.request(worktree), success=False)
        self.assertIn("invalid task fields", result["error"])
        self.assertFalse(self.events.exists())

    def test_inspect_reports_dirty_and_adopts_without_discarding(self):
        worktree = self.branch("issue-a")
        (worktree / "unfinished.txt").write_text("in progress")
        result = self.run_request({"action": "prepare", "repoPath": str(self.repo), "worktree": str(worktree), "branch": "issue-a", "issueKey": "default:issue-a"})
        self.assertFalse(result["clean"])
        self.assertEqual((worktree / "unfinished.txt").read_text(), "in progress")

    def test_different_issues_cannot_share_a_worktree_or_branch(self):
        worktree = self.branch("issue-a")
        request = {"action": "prepare", "repoPath": str(self.repo), "worktree": str(worktree), "branch": "issue-a", "issueKey": "default:different"}
        result = self.run_request(request, success=False)
        self.assertIn("already belongs to issue", result["error"])
        result = self.run_request({**request, "worktree": str(self.root / "another")}, success=False)
        self.assertIn("already belongs to issue", result["error"])
        result = self.run_request(self.request(worktree, issueKey="default:different"), success=False)
        self.assertIn("already belongs to issue", result["error"])
        self.assertFalse(self.events.exists())

    def test_git_routing_environment_cannot_redirect_operations(self):
        worktree = self.branch("issue-a")
        result = self.run_request(
            self.request(worktree, skipCiRequest="Skip this merge's full check."),
            env={"GIT_DIR": "/missing/repo", "GIT_WORK_TREE": "/missing/worktree", "GIT_INDEX_FILE": "/missing/index"},
        )
        self.assertEqual(result["status"], "merged")

    def test_missing_receipt_is_readable_without_running_work(self):
        attempt = str(uuid.uuid4())
        result = self.run_request({"action": "receipt", "repoPath": str(self.repo), "attemptId": attempt})
        self.assertEqual(result, {"status": "missing", "attemptId": attempt})
        self.assertFalse(self.events.exists())

    def test_corrupt_receipt_cannot_claim_verified_or_merged(self):
        worktree = self.branch("issue-a")
        request = self.request(worktree, skipCiRequest="Skip this test merge.")
        result = self.run_request(request)
        path = self.repo / ".git" / "yinz-lifecycle" / "merges" / f"{request['attemptId']}.json"
        result["ci"] = {"status": "skipped", "command": ["cargo", "x", "ci", "check"]}
        path.write_text(json.dumps(result))
        error = self.run_request({"action": "receipt", "repoPath": str(self.repo), "attemptId": request["attemptId"]}, success=False)
        self.assertIn("without an explicit request", error["error"])

    def test_failed_ci_preserves_failure_and_never_merges(self):
        worktree = self.branch("issue-a")
        base = self.git(self.repo, "rev-parse", "HEAD")
        request = self.request(worktree)
        self.run_request(request, env={"YINZ_TEST_CI_EXIT": "9"}, success=False)
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), base)
        self.assertEqual(self.receipt(request)["status"], "failed")
        self.assertEqual(self.receipt(request)["ci"]["status"], "failed")
        self.run_request(request, success=False)
        self.assertEqual(len(self.event_rows()), 2)

    def test_concurrent_integrations_serialize_rebase_ci_and_merge(self):
        first_tree = self.branch("issue-a")
        second_tree = self.branch("issue-b")
        first_request = self.request(first_tree)
        second_request = self.request(second_tree)
        first = self.start_request(first_request, {"YINZ_TEST_WAIT": "1"})
        self.wait_for("first CI start", lambda: len(self.event_rows()) == 1)
        second = self.start_request(second_request, {"YINZ_TEST_WAIT": "1"})
        # Wait for an observable lock contention message, not a scheduling delay.
        line = second.stderr.readline()
        self.assertIn("Waiting for", line)
        self.assertEqual(len(self.event_rows()), 1)
        (self.root / "issue-a.release").touch()
        first_stdout, first_stderr = first.communicate(timeout=15)
        self.assertEqual(first.returncode, 0, first_stderr)
        first_result = json.loads(first_stdout)
        self.wait_for("second CI start", lambda: len(self.event_rows()) == 3)
        self.assertEqual(self.git(second_tree, "rev-parse", "HEAD^"), first_result["mergedCommit"])
        (self.root / "issue-b.release").touch()
        second_stdout, second_stderr = second.communicate(timeout=15)
        self.assertEqual(second.returncode, 0, second_stderr)
        second_result = json.loads(second_stdout)
        self.assertEqual(second_result["baseCommit"], first_result["mergedCommit"])
        self.assertEqual([row["branch"] for row in self.event_rows()], ["issue-a", "issue-a", "issue-b", "issue-b"])
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), second_result["mergedCommit"])

    def test_manual_main_movement_requires_new_attempt(self):
        worktree = self.branch("issue-a")
        result = self.run_request(self.request(worktree), env={"YINZ_TEST_MOVE_MAIN": str(self.repo)}, success=False)
        self.assertIn("main changed", result["error"])
        self.assertFalse((self.repo / "issue-a.txt").exists())

    def test_worktree_changed_by_ci_cannot_merge(self):
        worktree = self.branch("issue-a")
        result = self.run_request(self.request(worktree), env={"YINZ_TEST_DIRTY": "1"}, success=False)
        self.assertIn("not clean", result["error"])
        self.assertFalse((self.repo / "issue-a.txt").exists())

    def test_ci_changes_cannot_leave_reusable_verification(self):
        worktree = self.branch("issue-a")
        base = self.git(self.repo, "rev-parse", "HEAD")
        for control, message in (
            ("YINZ_TEST_EDIT_TRACKED", "not clean"),
            ("YINZ_TEST_MOVE_HEAD", "HEAD changed"),
            ("YINZ_TEST_SWITCH_BRANCH", "changed branches"),
        ):
            with self.subTest(control=control):
                request = self.request(worktree)
                result = self.run_request(request, env={control: "1"}, success=False)
                self.assertIn(message, result["error"])
                self.assertEqual(self.receipt(request)["status"], "failed")
                self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), base)
                self.git(worktree, "switch", "issue-a")
                self.git(worktree, "reset", "--hard", request["expectedHead"])
                events = len(self.event_rows())
                retry = self.run_request(request, success=False)
                self.assertIn("create a new attempt", retry["error"])
                self.assertEqual(len(self.event_rows()), events)
                self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), base)
        result = self.run_request(self.request(worktree))
        self.assertEqual(result["status"], "merged")
        self.assertEqual(len(self.event_rows()), 8, "a new attempt must rerun CI")

    def test_issue_changed_during_ci_cannot_merge(self):
        worktree = self.branch("issue-a")
        result = self.run_request(self.request(worktree), env={"YINZ_TEST_EDIT_ISSUE": "1"}, success=False)
        self.assertIn("description changed", result["error"])
        self.assertFalse((self.repo / "issue-a.txt").exists())

    def test_recurrence_added_during_ci_cannot_merge(self):
        worktree = self.branch("issue-a")
        result = self.run_request(self.request(worktree), env={"YINZ_TEST_RECURRING_ISSUE": "1"}, success=False)
        self.assertIn("Recurring", result["error"])
        self.assertFalse((self.repo / "issue-a.txt").exists())

    def test_shared_target_and_target_environment_are_rejected(self):
        worktree = self.branch("issue-a")
        (worktree / "target").symlink_to(self.root / "shared-target")
        result = self.run_request(self.request(worktree), success=False)
        self.assertIn("real target", result["error"])
        (worktree / "target").unlink()
        result = self.run_request(self.request(worktree), env={"CARGO_TARGET_DIR": str(self.root / "shared")}, success=False)
        self.assertIn("target directory overrides", result["error"])

    def cargo_config(self, root, contents, filename="config.toml"):
        directory = root / ".cargo"
        directory.mkdir(exist_ok=True)
        (directory / filename).write_text(contents)

    def commit_config(self, root):
        self.git(root, "add", ".cargo")
        self.git(root, "commit", "-m", "configure Cargo")

    def test_nested_worktree_target_shadows_parent_checkout_target(self):
        self.cargo_config(self.repo, '[build]\ntarget-dir = "target"\n')
        self.commit_config(self.repo)
        worktree = self.branch("issue-a", nested=True)
        result = self.run_request(self.request(worktree))
        self.assertEqual(result["status"], "merged")
        self.assertEqual(len(self.event_rows()), 2)

    def test_inherited_shared_target_is_rejected_without_local_target_setting(self):
        self.cargo_config(self.repo, '[build]\ntarget-dir = "target"\n')
        self.commit_config(self.repo)
        worktree = self.branch("issue-a", nested=True)
        self.cargo_config(worktree, "[build]\njobs = 2\n")
        self.commit_config(worktree)
        base = self.git(self.repo, "rev-parse", "HEAD")
        result = self.run_request(self.request(worktree), success=False)
        self.assertIn("break build isolation", result["error"])
        self.assertEqual(self.event_rows(), [])
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), base)

    def test_local_target_shadows_ancestor_and_cargo_home_targets(self):
        self.cargo_config(self.root, '[build]\ntarget-dir = "shared"\n')
        cargo_home = Path(self.env["CARGO_HOME"])
        cargo_home.mkdir()
        (cargo_home / "config.toml").write_text('[build]\ntarget-dir = "shared-home"\n')
        worktree = self.branch("issue-a")
        self.cargo_config(worktree, '[build]\ntarget-dir = "./target"\n')
        self.commit_config(worktree)
        self.assertEqual(self.run_request(self.request(worktree))["status"], "merged")

    def test_legacy_cargo_config_takes_precedence_over_config_toml(self):
        for legacy, modern, success in (("target", "../shared", True), ("../shared", "target", False)):
            with self.subTest(legacy=legacy):
                worktree = self.branch("issue-safe" if success else "issue-shared")
                self.cargo_config(worktree, f'[build]\ntarget-dir = "{legacy}"\n', "config")
                self.cargo_config(worktree, f'[build]\ntarget-dir = "{modern}"\n')
                self.commit_config(worktree)
                events = len(self.event_rows())
                result = self.run_request(self.request(worktree), success=success)
                if success:
                    self.assertEqual(result["status"], "merged")
                else:
                    self.assertIn("break build isolation", result["error"])
                    self.assertEqual(len(self.event_rows()), events)

    def test_malformed_ancestor_config_is_rejected_even_with_local_target(self):
        self.cargo_config(self.root, "[build\n")
        worktree = self.branch("issue-a")
        self.cargo_config(worktree, '[build]\ntarget-dir = "target"\n')
        self.commit_config(worktree)
        result = self.run_request(self.request(worktree), success=False)
        self.assertIn("Cannot inspect Cargo config", result["error"])
        self.assertEqual(self.event_rows(), [])

    def test_wrong_repository_and_reviewed_head_are_rejected(self):
        worktree = self.branch("issue-a")
        result = self.run_request(self.request(worktree, expectedHead="0" * 40), success=False)
        self.assertIn("reviewed commit", result["error"])
        other = self.root / "other"
        other.mkdir()
        self.git(other, "init", "--initial-branch=main")
        result = self.run_request({"action": "inspect", "repoPath": str(other), "worktree": str(worktree)}, success=False)
        self.assertIn("another Git repository", result["error"])

    def test_cancelled_ci_child_is_reaped_before_lock_is_released(self):
        first_tree = self.branch("issue-a")
        second_tree = self.branch("issue-b")
        request = self.request(first_tree)
        process = self.start_request(request, {"YINZ_TEST_WAIT": "1"})
        self.wait_for("CI child to start", lambda: bool(self.event_rows()))
        child_pid = self.event_rows()[0]["pid"]
        process.send_signal(signal.SIGTERM)
        stdout, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 130, stderr)
        self.assertIn("cancelled", json.loads(stdout)["error"])
        with self.assertRaises(ProcessLookupError):
            os.kill(child_pid, 0)
        self.assertEqual(self.receipt(request)["status"], "failed")
        result = self.run_request(self.request(second_tree, skipCiRequest="Skip this test merge."))
        self.assertEqual(result["status"], "merged")

    def test_killed_runner_does_not_unlock_while_ci_child_is_alive(self):
        first_tree = self.branch("issue-a")
        second_tree = self.branch("issue-b")
        request = self.request(first_tree)
        first = self.start_request(request, {"YINZ_TEST_WAIT": "1"})
        self.wait_for("CI child to start", lambda: bool(self.event_rows()))
        first.kill()
        first.wait(timeout=10)
        second = self.start_request(self.request(second_tree, skipCiRequest="Skip this test merge."))
        self.assertIn("Waiting for", second.stderr.readline())
        (self.root / "issue-a.release").touch()
        first.communicate(timeout=10)
        stdout, stderr = second.communicate(timeout=10)
        self.assertEqual(second.returncode, 0, stderr)
        self.assertEqual(json.loads(stdout)["status"], "merged")
        self.assertEqual(self.receipt(request)["status"], "missing")
        self.assertFalse((self.repo / "issue-a.txt").exists())

    def test_receipt_reconciles_merge_after_lost_final_write(self):
        worktree = self.branch("issue-a")
        request = self.request(worktree, skipCiRequest="Skip this test merge.")
        result = self.run_request(request)
        path = self.repo / ".git" / "yinz-lifecycle" / "merges" / f"{request['attemptId']}.json"
        result["status"] = "verified"
        result.pop("mergedCommit")
        path.write_text(json.dumps(result))
        reconciled = self.receipt(request)
        self.assertEqual(reconciled["status"], "merged")
        self.assertEqual(reconciled["mergedCommit"], self.git(self.repo, "rev-parse", "HEAD"))
        self.assertFalse(self.events.exists())

    def test_rebase_conflict_preserves_worktree_for_repair(self):
        worktree = self.branch("issue-a")
        (worktree / "base.txt").write_text("issue edit")
        self.git(worktree, "add", "base.txt")
        self.git(worktree, "commit", "-m", "issue conflict")
        (self.repo / "base.txt").write_text("main edit")
        self.git(self.repo, "add", "base.txt")
        self.git(self.repo, "commit", "-m", "main conflict")
        base = self.git(self.repo, "rev-parse", "HEAD")
        request = self.request(worktree)
        self.run_request(request, success=False)
        self.assertEqual(self.receipt(request)["status"], "failed")
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), base)
        self.assertIn("UU base.txt", self.git(worktree, "status", "--porcelain"))
        self.assertFalse(self.events.exists())


if __name__ == "__main__":
    unittest.main()
