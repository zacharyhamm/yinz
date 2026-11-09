"""Exercise integration against real temporary Git worktrees and CLI stubs."""

from __future__ import annotations

import fcntl
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
sys.path.insert(0, str(RUNNER.parent))
import git_ops  # noqa: E402


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
        self.vault_workspace = self.root / "vault-workspace"
        self.vault_workspace.write_text("default")
        self.env["YINZ_TEST_VAULT_WORKSPACE"] = str(self.vault_workspace)
        self.stub("swamp", '''import json, os, pathlib, sys
assert sys.argv[1:6] == ["vault", "read-secret", "default", "AGENDA_WORKSPACE", "--force"]
assert not any(name in os.environ for name in ("SWAMP_SERVER_URL", "SWAMP_SERVE_URL", "SWAMP_REPO_DIR", "SWAMP_WORKFLOWS_DIR"))
print(json.dumps({"value": pathlib.Path(os.environ["YINZ_TEST_VAULT_WORKSPACE"]).read_text()}))
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
if os.environ.get("YINZ_TEST_EDIT_WORKSPACE"):
    pathlib.Path(os.environ["YINZ_TEST_VAULT_WORKSPACE"]).write_text("other")
if os.environ.get("YINZ_TEST_RECURRING_ISSUE"):
    agenda = pathlib.Path(os.environ["YINZ_TEST_AGENDA"])
    item = json.loads(agenda.read_text())
    item["item"]["assignments"].append({"category": 2, "item": 7, "source": "explicit", "value": {"date": {"temporal": {"kind": "floating"}, "recurrence": {"frequency": "daily"}}}})
    agenda.write_text(json.dumps(item))
if os.environ.get("YINZ_TEST_CI_OUTPUT"):
    sys.stdout.write(os.environ["YINZ_TEST_CI_OUTPUT"])
    sys.stdout.flush()
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

    def closeout_fixture(self, name="closeout"):
        worktree = self.branch(name)
        request = self.request(worktree)
        merged = self.run_request(request)
        request.update(action="closeout-preview")
        target = self.run_request(request)
        return worktree, {**request, "action": "closeout-cleanup", "cleanupId": str(uuid.uuid4()), "target": target}, merged

    def closeout_fault(self, stage):
        real_git = __import__("shutil").which("git")
        marker = self.root / "closeout-fault"
        self.stub("git", f'''import pathlib, subprocess, sys
args = sys.argv[1:]
marker = pathlib.Path({str(marker)!r})
remove = "worktree" in args and "remove" in args
branch = "update-ref" in args and "-d" in args
if {stage!r} == "before-branch" and branch and not marker.exists():
    marker.touch()
    raise SystemExit(71)
result = subprocess.run([{real_git!r}, *args])
if {stage!r} == "after-worktree" and remove and result.returncode == 0 and not marker.exists():
    marker.touch()
    raise SystemExit(71)
raise SystemExit(result.returncode)
''')

    def test_closeout_removes_worktree_then_exact_branch_and_replays_without_ci(self):
        worktree, request, merged = self.closeout_fixture()
        (worktree / "target").mkdir()
        (worktree / "target" / "ignored-build").write_text("outputs")
        result = self.run_request(request)
        self.assertEqual((result["worktree"], result["branch"]), ("removed", "removed"))
        self.assertFalse(worktree.exists())
        self.assertEqual(self.git(self.repo, "branch", "--list", "closeout"), "")
        self.assertEqual(self.run_request(request)["target"], request["target"])
        self.assertEqual(len(self.event_rows()), 2)
        self.assertEqual(self.receipt(request)["mergedCommit"], merged["mergedCommit"])
        owners = json.loads((self.repo / ".git/yinz-lifecycle/worktree-owners.json").read_text())["owners"]
        self.assertEqual(owners, [])

    def test_closeout_lost_worktree_ack_reserves_claim_and_resumes_without_opening_missing_path(self):
        worktree, request, _ = self.closeout_fixture()
        self.closeout_fault("after-worktree")
        self.run_request(request, success=False)
        self.assertFalse(worktree.exists())
        self.assertNotEqual(self.git(self.repo, "branch", "--list", "closeout"), "")
        blocked = self.run_request({"action": "prepare", "repoPath": str(self.repo), "worktree": str(worktree), "branch": "closeout", "issueKey": request["issueKey"]}, success=False)
        self.assertIn("reserved", blocked["error"])
        result = self.run_request(request)
        self.assertEqual(result["branch"], "removed")
        self.assertEqual(self.git(self.repo, "branch", "--list", "closeout"), "")

    def test_closeout_partial_branch_failure_blocks_moved_or_recreated_resources(self):
        worktree, request, _ = self.closeout_fixture()
        self.closeout_fault("before-branch")
        self.run_request(request, success=False)
        self.assertFalse(worktree.exists())
        worktree.mkdir()
        self.assertIn("reused", self.run_request(request, success=False)["error"])
        worktree.rmdir()
        self.git(self.repo, "update-ref", "refs/heads/closeout", self.git(self.repo, "rev-parse", "HEAD^"))
        self.assertIn("moved", self.run_request(request, success=False)["error"])
        self.git(self.repo, "update-ref", "refs/heads/closeout", request["target"]["head"])
        self.assertIn("identity changed", self.run_request(request, success=False)["error"])

    def test_closeout_same_oid_branch_recreation_during_partial_cleanup_is_stale(self):
        worktree, request, _ = self.closeout_fixture()
        self.closeout_fault("before-branch")
        self.run_request(request, success=False)
        self.assertFalse(worktree.exists())
        oid = request["target"]["head"]
        self.git(self.repo, "branch", "-d", "closeout")
        self.git(self.repo, "branch", "closeout", oid)
        self.assertIn("recreated", self.run_request(request, success=False)["error"])
        self.assertEqual(self.git(self.repo, "rev-parse", "refs/heads/closeout"), oid)

    def test_closeout_killed_helper_keeps_both_locks_and_concurrent_claim_prepare_cannot_recycle(self):
        worktree, request, _ = self.closeout_fixture()
        real_git = __import__("shutil").which("git")
        ready = self.root / "removal-ready"
        release = self.root / "removal-release"
        self.stub("git", f'''import os, pathlib, subprocess, sys, time
args = sys.argv[1:]
if "worktree" in args and "remove" in args:
    pathlib.Path({str(ready)!r}).write_text(str(os.getpid()))
    while not pathlib.Path({str(release)!r}).exists():
        time.sleep(0.01)
raise SystemExit(subprocess.run([{real_git!r}, *args]).returncode)
''')
        runner = self.start_request(request)
        # The fixture gates the child on a file, not elapsed time.
        self.addCleanup(release.touch)
        self.wait_for("worktree-removal child", ready.exists)
        waiting = []
        for action in ("claim", "prepare"):
            child = self.start_request({"action": action, "repoPath": str(self.repo), "worktree": str(worktree), "branch": "closeout", "issueKey": "another"})
            self.assertIn("Waiting for", child.stderr.readline())
            waiting.append(child)
        runner.kill()
        runner.wait(timeout=10)
        # The still-running native-command wrapper inherited both lock FDs.
        for name in ("merge", "worktree-ownership"):
            with (self.repo / ".git/yinz-lifecycle" / f"{name}.lock").open("a+") as lock:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        release.touch()
        runner.communicate(timeout=10)
        for child in waiting:
            stdout, stderr = child.communicate(timeout=10)
            self.assertNotEqual(child.returncode, 0, stderr)
            self.assertIn("reserved", json.loads(stdout)["error"])
        self.assertFalse(worktree.exists())
        self.assertEqual(self.run_request(request)["branch"], "removed")

    def test_closeout_dirty_locked_changed_identity_and_receipt_fail_closed(self):
        worktree, request, _ = self.closeout_fixture()
        (worktree / "untracked").write_text("preserve")
        self.assertIn("not clean", self.run_request(request, success=False)["error"])
        (worktree / "untracked").unlink()
        self.git(self.repo, "worktree", "lock", str(worktree))
        self.assertIn("locked", self.run_request(request, success=False)["error"])
        self.git(self.repo, "worktree", "unlock", str(worktree))
        wrong = {**request, "issueDigest": "wrong"}
        self.assertIn("does not match", self.run_request(wrong, success=False)["error"])
        # A renamed original inode still exists, so this cannot reuse its identity.
        link = worktree / ".git"
        link.rename(worktree / ".git-original")
        link.write_text(request["target"]["gitLink"])
        (worktree / ".git-original").unlink()
        self.assertIn("identity changed", self.run_request(request, success=False)["error"])
        self.assertTrue(worktree.exists())
        self.assertNotEqual(self.git(self.repo, "branch", "--list", "closeout"), "")

    def test_closeout_refuses_control_ancestor_other_checkout_and_submodules(self):
        worktree, request, _ = self.closeout_fixture()
        (worktree / ".gitmodules").write_text("")
        self.git(worktree, "add", ".gitmodules")
        # Even an ignored/empty submodule manifest requires manual inspection.
        self.git(worktree, "reset", "--", ".gitmodules")
        self.assertIn("not clean", self.run_request(request, success=False)["error"])
        (worktree / ".gitmodules").unlink()
        other = self.root / "duplicate"
        self.git(self.repo, "worktree", "add", "--force", str(other), "closeout")
        self.assertIn("another linked checkout", self.run_request(request, success=False)["error"])
        self.git(self.repo, "worktree", "remove", str(other))
        # Selecting the issue worktree as the control repo must never remove it.
        self.assertIn("control/store", self.run_request({**request, "repoPath": str(worktree)}, success=False)["error"])
        self.assertTrue(worktree.exists())

    def test_closeout_protects_a_control_store_symlink_into_ignored_build_outputs(self):
        worktree, request, _ = self.closeout_fixture()
        store = worktree / "target" / "store"
        store.mkdir(parents=True)
        (store / "durable-evidence").write_text("retain")
        (self.repo / ".swamp").symlink_to(store, target_is_directory=True)
        self.assertIn("control/store", self.run_request(request, success=False)["error"])
        self.assertTrue((store / "durable-evidence").is_file())
        self.assertTrue(worktree.exists())

    def test_closeout_symlink_alias_preview_pins_canonical_registered_path(self):
        worktree, request, _ = self.closeout_fixture()
        alias = self.root / "control-alias"
        alias.symlink_to(self.repo, target_is_directory=True)
        target = self.run_request({**request, "action": "closeout-preview", "repoPath": str(alias)})
        self.assertEqual(target, request["target"])
        self.run_request({**request, "repoPath": str(alias)})
        self.assertFalse(worktree.exists())

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
        log = Path(result["ci"].pop("log"))
        self.assertEqual(log, self.repo / ".git" / "yinz-lifecycle" / "merges" / request["attemptId"] / "ci-check.log")
        self.assertTrue(log.is_file())
        self.assertEqual(result["ci"], {
            "command": ["cargo", "x", "ci", "check"], "status": "passed", "exitCode": 0,
            "phases": [], "failedTests": [], "summary": "no phase results; exit 0",
        })
        result["ci"]["log"] = str(log)
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

    def owners(self):
        path = self.repo / ".git" / "yinz-lifecycle" / "worktree-owners.json"
        return json.loads(path.read_text())["owners"] if path.exists() else []

    def claim(self, worktree, issue_key, *, success=True, action="claim"):
        return self.run_request({"action": action, "repoPath": str(self.repo), "worktree": str(worktree),
                                 "issueKey": issue_key}, success=success)

    def confirm(self, worktree, issue_key, *, success=True):
        return self.claim(worktree, issue_key, success=success, action="confirm")

    def write_owners(self, owners):
        directory = self.repo / ".git" / "yinz-lifecycle"
        directory.mkdir(exist_ok=True)
        (directory / "worktree-owners.json").write_text(json.dumps({"owners": owners}))

    def feature_worktree(self, name):
        worktree = self.root / name
        self.git(self.repo, "worktree", "add", "-b", name, str(worktree))
        return worktree

    def test_claim_reserves_canonical_worktree_for_one_issue(self):
        worktree = self.feature_worktree("origin")
        nested = worktree / "nested"
        nested.mkdir()
        canonical = str(worktree.resolve())
        self.assertEqual(self.claim(nested, "default/1"), {"worktree": canonical})
        pending = [{"issueKey": "default/1", "worktree": canonical, "pending": True}]
        self.assertEqual(self.owners(), pending)
        self.claim(worktree, "default/1")
        self.assertEqual(self.owners(), pending)
        self.assertEqual(self.confirm(nested, "default/1"), {"worktree": canonical})
        self.assertEqual(self.owners(), [{"issueKey": "default/1", "worktree": canonical}])
        # Claiming again never downgrades a confirmed claim to a reclaimable one.
        self.claim(worktree, "default/1")
        self.assertEqual(self.owners(), [{"issueKey": "default/1", "worktree": canonical}])
        result = self.claim(worktree, "default/2", success=False)
        self.assertIn("already belongs to issue default/1", result["error"])
        # Another issue's prepare cannot adopt the claimed worktree on any branch.
        result = self.run_request({"action": "prepare", "repoPath": str(self.repo), "worktree": canonical,
                                   "branch": "origin", "issueKey": "default/2"}, success=False)
        self.assertIn("already belongs to issue default/1", result["error"])
        self.assertEqual(self.owners(), [{"issueKey": "default/1", "worktree": canonical}])

    def test_claim_rejects_worktrees_owned_by_prepared_issues(self):
        worktree = self.branch("issue-a")
        before = self.owners()
        result = self.claim(worktree, "default/other", success=False)
        self.assertIn("already belongs to issue default:issue-a", result["error"])
        self.assertEqual(self.owners(), before)
        # The owning legacy issue may claim its own prepared worktree without a new entry.
        self.claim(worktree, "default:issue-a")
        self.assertEqual(self.owners(), before)

    def test_claim_replaces_orphaned_start_claim_of_same_issue(self):
        first = self.feature_worktree("first")
        second = self.feature_worktree("second")
        self.claim(first, "default/1")
        self.claim(second, "default/1")
        self.assertEqual(self.owners(), [{"issueKey": "default/1", "worktree": str(second.resolve()), "pending": True}])
        self.claim(first, "default/2")

    def test_claim_reclaims_pending_claim_of_idle_issue(self):
        worktree = self.feature_worktree("origin")
        canonical = str(worktree.resolve())
        self.claim(worktree, "default/1")
        # Issue 1's start ended before confirming, so it never saved state.
        self.claim(worktree, "default/2")
        self.assertEqual(self.owners(), [{"issueKey": "default/2", "worktree": canonical, "pending": True}])
        # The abandoned issue learns it lost the worktree instead of sharing it.
        result = self.confirm(worktree, "default/1", success=False)
        self.assertIn("was lost; it may have been reclaimed by issue default/2", result["error"])
        self.confirm(worktree, "default/2")
        self.assertEqual(self.owners(), [{"issueKey": "default/2", "worktree": canonical}])

    def test_claim_refuses_pending_claim_of_running_issue(self):
        worktree = self.feature_worktree("origin")
        self.claim(worktree, "default/1")
        before = self.owners()
        issues = self.repo / ".git" / "yinz-lifecycle" / "issues"
        issues.mkdir(exist_ok=True)
        digest = hashlib.sha256("default/1".encode()).hexdigest()
        # Hold issue 1's lifecycle lock as its in-progress start would.
        with (issues / f"{digest}.lock").open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            result = self.claim(worktree, "default/2", success=False)
        self.assertIn("issue default/1's start in progress", result["error"])
        self.assertEqual(self.owners(), before)
        self.claim(worktree, "default/2")
        self.assertEqual([owner["issueKey"] for owner in self.owners()], ["default/2"])

    def test_claim_never_reclaims_confirmed_claim(self):
        worktree = self.feature_worktree("origin")
        self.claim(worktree, "default/1")
        self.confirm(worktree, "default/1")
        before = self.owners()
        result = self.claim(worktree, "default/2", success=False)
        self.assertIn("already belongs to issue default/1", result["error"])
        self.assertEqual(self.owners(), before)

    def test_confirm_requires_this_issues_claim(self):
        worktree = self.feature_worktree("origin")
        result = self.confirm(worktree, "default/1", success=False)
        self.assertIn("was lost; it may have been reclaimed by another issue", result["error"])
        self.assertEqual(self.owners(), [])

    def test_confirm_keeps_prepared_ownership(self):
        worktree = self.branch("issue-a")
        before = self.owners()
        self.confirm(worktree, "default:issue-a")
        self.assertEqual(self.owners(), before)

    def test_owner_registry_rejects_invalid_pending_entries(self):
        worktree = self.feature_worktree("origin")
        canonical = str(worktree.resolve())
        for entry in ({"issueKey": "default/1", "worktree": canonical, "pending": False},
                      {"issueKey": "default/1", "worktree": canonical, "branch": "origin", "pending": True}):
            self.write_owners([entry])
            result = self.claim(worktree, "default/2", success=False)
            self.assertIn("Invalid worktree ownership registry", result["error"])

    def test_claim_rejects_the_main_checkout(self):
        result = self.claim(self.repo, "default/1", success=False)
        self.assertIn("not the main checkout", result["error"])
        self.assertEqual(self.owners(), [])

    def test_claim_rejects_detached_head(self):
        worktree = self.root / "detached"
        self.git(self.repo, "worktree", "add", "--detach", str(worktree))
        result = self.claim(worktree, "default/1", success=False)
        self.assertIn("detached HEAD", result["error"])
        self.assertEqual(self.owners(), [])

    def test_claim_rejects_branch_owned_by_another_issue(self):
        owned = self.branch("issue-a")
        self.git(self.repo, "worktree", "remove", str(owned))
        moved = self.root / "moved"
        self.git(self.repo, "worktree", "add", str(moved), "issue-a")
        before = self.owners()
        result = self.claim(moved, "default/other", success=False)
        self.assertIn("already belongs to issue default:issue-a", result["error"])
        self.assertEqual(self.owners(), before)

    def test_prepare_upgrades_start_claim_to_full_ownership(self):
        worktree = self.feature_worktree("origin")
        canonical = str(worktree.resolve())
        request = {"action": "prepare", "repoPath": str(self.repo), "worktree": canonical,
                   "branch": "origin", "issueKey": "default/1"}
        full = [{"issueKey": "default/1", "worktree": canonical, "branch": "origin"}]
        # A legacy confirmed claim, then a current pending one, both upgrade.
        for confirmed in (True, False):
            self.write_owners([])
            self.claim(worktree, "default/1")
            if confirmed:
                self.confirm(worktree, "default/1")
            self.run_request(request)
            self.run_request(request)
            self.assertEqual(self.owners(), full)
        # Confirming after the upgrade keeps the prepared entry unchanged.
        self.confirm(worktree, "default/1")
        self.assertEqual(self.owners(), full)

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
        output = (
            ">> ci phase format --check: passed\n"
            "        FAIL [   0.019s] (1/4) yinz-server tests::flaky\n"
            ">> ci phase test: failed\n"
            "     Summary [   0.020s] 4 tests run: 3 passed, 1 failed\n"
            "        FAIL [   0.019s] (1/4) yinz-server tests::flaky\n"
            "\n>> Failed: test\n"
        )
        process = subprocess.run(
            [sys.executable, str(RUNNER)], input=json.dumps(request), text=True, capture_output=True,
            env={**self.env, "YINZ_TEST_CI_EXIT": "9", "YINZ_TEST_CI_OUTPUT": output}, timeout=30,
        )
        self.assertEqual(process.returncode, 1)
        self.assertIn("tests::flaky", process.stderr, "gate output must still stream")
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), base)
        receipt = self.receipt(request)
        self.assertEqual(receipt["status"], "failed")
        ci = receipt["ci"]
        self.assertEqual(ci["status"], "failed")
        self.assertEqual(ci["exitCode"], 9)
        self.assertEqual(ci["phases"], [{"name": "format --check", "status": "passed"}, {"name": "test", "status": "failed"}])
        self.assertEqual(ci["failedTests"], ["yinz-server tests::flaky"])
        self.assertEqual(ci["summary"], "phases failed: test; tests: yinz-server tests::flaky; exit 9")
        self.assertEqual(Path(ci["log"]).read_text(), output)
        self.assertEqual(receipt["failure"], f"cargo x ci check failed: {ci['summary']}; log: {ci['log']}")
        self.assertEqual(json.loads(process.stdout)["error"], receipt["failure"])
        self.run_request(request, success=False)
        self.assertEqual(len(self.event_rows()), 2)

    def test_failed_receipt_is_readable_while_the_merge_lock_is_held(self):
        worktree = self.branch("issue-a")
        failed = self.request(worktree)
        self.run_request(failed, env={"YINZ_TEST_CI_EXIT": "1"}, success=False)
        merged = self.run_request(self.request(worktree, skipCiRequest="Skip this test merge."))
        import fcntl
        peek = {"action": "receipt", "repoPath": str(self.repo), "failedOnly": True}
        with (self.repo / ".git" / "yinz-lifecycle" / "merge.lock").open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            receipt = self.run_request({**peek, "attemptId": failed["attemptId"]})
            other = self.run_request({**peek, "attemptId": merged["attemptId"]})
            missing = self.run_request({**peek, "attemptId": str(uuid.uuid4())})
        self.assertEqual(receipt, self.receipt(failed))
        self.assertEqual(other["status"], "not_failed")
        self.assertEqual(missing["status"], "missing")

    def test_new_attempt_receipts_keep_earlier_failed_attempts_of_the_same_issue(self):
        worktree = self.branch("issue-a")
        other = self.branch("issue-b")
        failed = self.request(worktree)
        output = ">> ci phase test: failed\n     TIMEOUT [  60.004s] (2/9) yinz-screen tests::slow\n"
        self.run_request(failed, env={"YINZ_TEST_CI_EXIT": "101", "YINZ_TEST_CI_OUTPUT": output}, success=False)
        self.run_request(self.request(other), env={"YINZ_TEST_CI_EXIT": "1"}, success=False)
        first = self.receipt(failed)
        self.assertNotIn("previousAttempts", first)
        merges = self.repo / ".git" / "yinz-lifecycle" / "merges"
        (merges / f"{uuid.uuid4()}.json").write_text("not json")
        passed = self.run_request(self.request(worktree))
        self.assertEqual(passed["status"], "merged")
        self.assertEqual(passed["previousAttempts"], [{
            "attemptId": failed["attemptId"], "timestamp": first["timestamp"], "failure": first["failure"],
            "ci": {key: first["ci"][key] for key in ("status", "summary", "log", "exitCode", "phases", "failedTests")},
        }])
        self.assertEqual(passed["previousAttempts"][0]["ci"]["failedTests"], ["yinz-screen tests::slow"])
        self.assertTrue(Path(passed["previousAttempts"][0]["ci"]["log"]).is_file())
        self.assertEqual(self.receipt({"attemptId": passed["attemptId"]})["previousAttempts"], passed["previousAttempts"])

    def test_receipts_without_gate_details_remain_valid_and_malformed_details_are_rejected(self):
        worktree = self.branch("issue-a")
        request = self.request(worktree)
        result = self.run_request(request)
        path = self.repo / ".git" / "yinz-lifecycle" / "merges" / f"{request['attemptId']}.json"
        legacy = {**result, "ci": {"status": "passed", "command": ["cargo", "x", "ci", "check"]}}
        path.write_text(json.dumps(legacy))
        self.assertEqual(self.receipt(request)["ci"], legacy["ci"])
        for ci in ({"phases": [{"name": "lint", "status": "skipped"}]}, {"failedTests": [1]}, {"exitCode": "1"}):
            with self.subTest(ci=ci):
                path.write_text(json.dumps({**legacy, "ci": {**legacy["ci"], **ci}}))
                error = self.run_request({"action": "receipt", "repoPath": str(self.repo), "attemptId": request["attemptId"]}, success=False)
                self.assertIn("invalid CI", error["error"])
        path.write_text(json.dumps({**legacy, "previousAttempts": [{"ci": {}}]}))
        error = self.run_request({"action": "receipt", "repoPath": str(self.repo), "attemptId": request["attemptId"]}, success=False)
        self.assertIn("attempt history", error["error"])

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

    def test_vault_workspace_changed_during_ci_cannot_merge(self):
        worktree = self.branch("issue-a")
        base = self.git(self.repo, "rev-parse", "HEAD")
        result = self.run_request(self.request(worktree), env={"YINZ_TEST_EDIT_WORKSPACE": "1"}, success=False)
        self.assertIn("Vault Agenda workspace does not match", result["error"])
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), base)

    def test_vault_workspace_mismatch_blocks_ci(self):
        worktree = self.branch("issue-a")
        self.vault_workspace.write_text("other")
        result = self.run_request(self.request(worktree), success=False)
        self.assertIn("Vault Agenda workspace does not match", result["error"])
        self.assertEqual(self.event_rows(), [])

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
        receipt = self.receipt(request)
        self.assertEqual(receipt["status"], "failed")
        self.assertIsNone(receipt["ci"]["exitCode"])
        self.assertTrue(receipt["ci"]["summary"].endswith("interrupted"))
        self.assertTrue(Path(receipt["ci"]["log"]).is_file(), "cancellation keeps the partial log")
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


    def check_request(self, worktree, argv, **overrides):
        return {
            "action": "check", "repoPath": str(self.repo), "worktree": str(worktree),
            "expectedBranch": self.git(worktree, "branch", "--show-current"),
            "expectedHead": self.git(worktree, "rev-parse", "HEAD"),
            "argv": argv, **overrides,
        }

    def run_check(self, request, *, env=None):
        result = subprocess.run(
            [sys.executable, str(RUNNER)], input=json.dumps(request), text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env={**self.env, **(env or {})}, timeout=30,
        )
        return result.returncode, json.loads(result.stdout), result.stderr

    def test_check_reports_exit_code_duration_and_log_without_raising(self):
        worktree = self.branch("issue-a")
        code, result, log = self.run_check(self.check_request(
            worktree, ["sh", "-c", 'printf "%s|" "$0" "$1"; echo; exit 7', "a b", "$HOME;"]))
        self.assertEqual(code, 0, log)
        self.assertEqual(result["exitCode"], 7)
        self.assertTrue(result["worktreeUnchanged"])
        self.assertGreaterEqual(result["durationMs"], 0)
        # argv is passed verbatim: no shell splitting or expansion of arguments.
        self.assertIn("a b|$HOME;|", log)
        self.assertIn("exit 7 after", log)
        code, result, _ = self.run_check(self.check_request(worktree, ["/usr/bin/true"]))
        self.assertEqual((code, result["exitCode"]), (0, 0))

    def test_check_reports_a_missing_executable_as_a_failed_result(self):
        worktree = self.branch("issue-a")
        code, result, log = self.run_check(self.check_request(worktree, ["yinz-no-such-command"]))
        self.assertEqual(code, 0, log)
        self.assertIsNone(result["exitCode"])
        self.assertIn("Cannot start yinz-no-such-command", log)
        # A file without a shebang fails exec with ENOEXEC, which is also a result.
        script = self.root / "no-shebang"
        script.write_text("echo hi\n")
        script.chmod(0o755)
        code, result, log = self.run_check(self.check_request(worktree, [str(script)]))
        self.assertEqual(code, 0, log)
        self.assertIsNone(result["exitCode"])
        self.assertIn("Cannot start", log)

    def test_check_reaps_descendants_that_would_hold_its_log_open(self):
        worktree = self.branch("issue-a")
        started = time.monotonic()
        code, result, log = self.run_check(self.check_request(worktree, ["sh", "-c", "sleep 60 & exit 0"]))
        self.assertLess(time.monotonic() - started, 20, log)
        self.assertEqual((code, result["exitCode"]), (0, 0), log)
        self.assertNotIn("timedOut", result)

    def test_check_times_out_and_stops_its_process_group(self):
        worktree = self.branch("issue-a")
        for argv in (["sh", "-c", "sleep 60"], ["sh", "-c", "sleep 60 & sleep 60"]):
            started = time.monotonic()
            code, result, log = self.run_check(self.check_request(worktree, argv, timeoutSeconds=1))
            self.assertLess(time.monotonic() - started, 20, log)
            self.assertEqual(code, 0, log)
            self.assertIsNone(result["exitCode"])
            self.assertIs(result["timedOut"], True)
            self.assertTrue(result["worktreeUnchanged"])
            self.assertIn("Timed out after 1 s", log)

    def test_check_rejects_an_invalid_timeout(self):
        worktree = self.branch("issue-a")
        for value in (0, -1, "x", True):
            code, result, _ = self.run_check(self.check_request(worktree, ["/usr/bin/true"], timeoutSeconds=value))
            self.assertNotEqual(code, 0)
            self.assertIn("timeoutSeconds", result["error"])

    def test_check_detects_a_command_that_changes_the_worktree(self):
        worktree = self.branch("issue-a")
        for argv in (
            ["sh", "-c", "echo new > untracked.txt"],
            ["git", "commit", "--allow-empty", "-m", "moved"],
        ):
            request = self.check_request(worktree, argv)
            code, result, log = self.run_check(request)
            self.assertEqual((code, result["exitCode"]), (0, 0), log)
            self.assertFalse(result["worktreeUnchanged"])
            self.assertIn("changed the worktree", log)
            (worktree / "untracked.txt").unlink(missing_ok=True)

    def test_check_requires_the_clean_reviewed_commit_and_isolated_target(self):
        worktree = self.branch("issue-a")
        marker = self.root / "ran"
        argv = ["touch", str(marker)]
        cases = [
            (self.check_request(worktree, argv, expectedHead="0" * 40), {}, "reviewed commit"),
            (self.check_request(worktree, argv, expectedBranch="other"), {}, "reviewed branch"),
            (self.check_request(worktree, []), {}, "argv"),
            (self.check_request(worktree, ["cargo", "a\x00b"]), {}, "argv"),
            (self.check_request(worktree, argv), {"CARGO_TARGET_DIR": str(self.root / "shared")}, "target directory overrides"),
        ]
        for request, env, message in cases:
            code, result, _ = self.run_check(request, env=env)
            self.assertNotEqual(code, 0)
            self.assertIn(message, result["error"])
        (worktree / "dirty.txt").write_text("dirty")
        code, result, _ = self.run_check(self.check_request(worktree, argv))
        self.assertNotEqual(code, 0)
        self.assertIn("not clean", result["error"])
        (worktree / "dirty.txt").unlink()
        (worktree / "target").symlink_to(self.root / "shared-target")
        code, result, _ = self.run_check(self.check_request(worktree, argv))
        self.assertIn("real target", result["error"])
        self.assertFalse(marker.exists())

    def test_check_runs_while_the_merge_lock_is_held(self):
        worktree = self.branch("issue-a")
        directory = self.repo / ".git" / "yinz-lifecycle"
        directory.mkdir(exist_ok=True)
        import fcntl
        with (directory / "merge.lock").open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            code, result, log = self.run_check(self.check_request(worktree, ["/usr/bin/true"]))
        self.assertEqual((code, result["exitCode"]), (0, 0), log)
        self.assertNotIn("lock", log)


class CiLogSummaryTests(unittest.TestCase):
    def summarize(self, text, code=1):
        with tempfile.TemporaryDirectory(prefix="yinz-ci-log-") as directory:
            log = Path(directory) / "ci-check.log"
            log.write_bytes(text.encode())
            return git_ops.summarize_ci_log(log, code)

    def test_phase_markers_and_nextest_failures_are_extracted_once_in_order(self):
        result = self.summarize(
            ">> ci phase dependency boundaries: passed\n"
            "\x1b[1;31m        FAIL\x1b[0m [   0.019s] (1/4) nt-probe tests::bad\n"
            "    test tests::bad ... FAILED\n"
            "     SIGSEGV [   0.184s] (3/4) nt-probe tests::segv\n"
            "        SLOW [>  1.000s] (\u2500\u2500\u2500) nt-probe tests::slow\n"
            "     TIMEOUT [   2.007s] (4/4) nt-probe tests::slow\n"
            "  TRY 2 FAIL [   0.010s] (5/5) nt-probe tests::retried\n"
            "test src/lib.rs - f (line 1) ... FAILED\n"
            ">> ci phase test: failed\r\n"
            "     Summary [   2.008s] 4 tests run: 1 passed, 2 failed, 1 timed out, 0 skipped\n"
            "        FAIL [   0.019s] (1/4) nt-probe tests::bad\n"
            ">> ci phase integration-test: passed\n",
            101,
        )
        self.assertEqual(result["phases"], [
            {"name": "dependency boundaries", "status": "passed"},
            {"name": "test", "status": "failed"},
            {"name": "integration-test", "status": "passed"},
        ])
        self.assertEqual(result["failedTests"], [
            "nt-probe tests::bad", "nt-probe tests::segv", "nt-probe tests::slow",
            "nt-probe tests::retried", "src/lib.rs - f (line 1)",
        ])
        self.assertEqual(result["exitCode"], 101)
        self.assertEqual(
            result["summary"],
            "phases failed: test; tests: nt-probe tests::bad, nt-probe tests::segv, nt-probe tests::slow, "
            "nt-probe tests::retried, src/lib.rs - f (line 1); exit 101",
        )

    def test_passing_gate_counts_its_phases(self):
        result = self.summarize(">> ci phase lint: passed\n>> ci phase test: passed\n\n>> All checks passed\n", 0)
        self.assertEqual(result["summary"], "2 phases passed; exit 0")
        self.assertEqual(result["failedTests"], [])

    def test_missing_markers_fall_back_to_the_last_output_line(self):
        result = self.summarize("Compiling xtask\nerror: could not compile `xtask`\n\n", 101)
        self.assertEqual(result["phases"], [])
        self.assertEqual(result["summary"], "no phase results; last output: error: could not compile `xtask`; exit 101")

    def test_many_failures_are_truncated_with_a_count(self):
        lines = "".join(f"        FAIL [   0.001s] crate tests::t{index}\n" for index in range(git_ops.FAILED_TEST_LIMIT + 7))
        result = self.summarize(lines)
        self.assertEqual(len(result["failedTests"]), git_ops.FAILED_TEST_LIMIT)
        self.assertEqual(result["failedTestsTruncated"], 7)
        self.assertIn(f"(+{git_ops.FAILED_TEST_LIMIT + 2} more)", result["summary"])

    def test_missing_log_is_an_interrupted_gate_without_results(self):
        result = git_ops.summarize_ci_log(Path("/nonexistent/ci-check.log"), None)
        self.assertEqual(result["summary"], "no phase results; interrupted")


if __name__ == "__main__":
    unittest.main()
