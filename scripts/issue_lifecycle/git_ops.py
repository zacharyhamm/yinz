#!/usr/bin/env python3
"""Local Git integration for the issue lifecycle; one JSON request per process.

The repository's common Git directory holds the merge lock and receipts, so
every worktree participates in the same exclusion boundary. A merge attempt
holds the OS lock through rebase, CI, and fast-forward. No remote is consulted.
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import threading
import time
from contextlib import ExitStack, contextmanager
from datetime import datetime, timezone
import uuid

from workspace import agenda_workspace, local_environment


class LifecycleError(Exception):
    """An operation failed without discarding the user's work."""


class Cancelled(LifecycleError):
    """The caller cancelled the active operation."""


class LockBusy(LifecycleError):
    """Optional closeout defers instead of queuing behind another Git operation."""

    def __init__(self, stage, label):
        self.stage = stage
        self.label = label
        super().__init__(
            f"Optional closeout deferred during {stage}: {label} lock is busy; "
            "another lifecycle Git operation (possibly merge/CI) may be running. "
            "The completed outcome and recorded cleanup progress are unchanged. "
            "Let that operation finish, then retry closeout or resume-closeout."
        )


def optional_lock_stage(request):
    stages = {"receipt": "merge-receipt validation", "closeout-preview": "cleanup preview",
              "closeout-cleanup": "Git cleanup"}
    if "failIfBusy" not in request:
        return None
    if type(request["failIfBusy"]) is not bool or request.get("action") not in stages:
        raise LifecycleError("failIfBusy must be a boolean for receipt, closeout-preview or closeout-cleanup only")
    return stages[request["action"]] if request["failIfBusy"] else None


_cancelled = False
_lock_fds = []
CI_COMMAND = ["cargo", "x", "ci", "check"]
# A hung plan check would otherwise hold the issue lock indefinitely.
CHECK_TIMEOUT_SECONDS = 3600
CI_LOG_NAME = "ci-check.log"
# Receipts and handoffs list failing tests; the log keeps every one.
FAILED_TEST_LIMIT = 50
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
# `cargo x ci check` prints one marker per finished phase (xtask/src/ci.rs, phase_marker).
PHASE_MARKER = re.compile(r">> ci phase (.+): (passed|failed)")
# nextest final statuses, e.g. `FAIL [   0.019s] (1/4) yinz-server tests::name`.
NEXTEST_FAILURE = re.compile(
    r"\s*(?:TRY \d+ )?(FAIL|TIMEOUT|ABORT|LEAK-FAIL|SIG[A-Z0-9]+)\s+\[[^\]]*\]\s+(?:\([^)]*\)\s+)?(\S+)\s+(\S+)\s*"
)
# Unindented libtest results: doctests, or the fallback when nextest is missing.
LIBTEST_FAILURE = re.compile(r"test (.+?) \.\.\. FAILED")


def now():
    return datetime.now(timezone.utc).isoformat()


def cancel(_signum, _frame):
    global _cancelled
    _cancelled = True


def check_cancelled():
    if _cancelled:
        raise Cancelled("Operation cancelled; inspect the worktree before retrying")


def stop_process(process, *, drained=False):
    """Reap the command group before the caller releases its merge lock.

    A drained process has a pump thread reading its output, so only wait for it.
    """
    for sig, timeout in ((signal.SIGTERM, 3), (signal.SIGKILL, 3)):
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            pass
        try:
            if drained:
                process.wait(timeout=timeout)
            else:
                process.communicate(timeout=timeout)
            # Descendants may outlive a shell which exits on SIGTERM.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            return
        except subprocess.TimeoutExpired:
            continue
    if drained:
        process.wait()
    else:
        process.communicate()


def child_environment():
    environment = {**local_environment(), "GIT_EDITOR": "/usr/bin/true", "GIT_SEQUENCE_EDITOR": "/usr/bin/true"}
    for variable in (
        "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_NAMESPACE",
    ):
        environment.pop(variable, None)
    return environment


def command(args, cwd, *, check=True, capture=True, reap_group=False, timeout=None):
    check_cancelled()
    # Children do not inherit stdin: every protocol request belongs to us.
    process = subprocess.Popen(
        args,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE if capture else sys.stderr,
        stderr=subprocess.PIPE if capture else sys.stderr,
        text=True,
        start_new_session=True,
        pass_fds=tuple(_lock_fds),
        env=child_environment(),
    )
    deadline = None if timeout is None else time.monotonic() + timeout
    try:
        while True:
            check_cancelled()
            if deadline is not None and time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired(args, timeout)
            try:
                stdout, stderr = process.communicate(timeout=0.1)
                break
            except subprocess.TimeoutExpired:
                continue
    except BaseException:
        stop_process(process)
        raise
    if reap_group:
        # A leftover descendant would hold our stderr, and with it the caller's
        # log stream, open after the command itself has exited.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    if check and process.returncode:
        detail = (stderr or stdout or "see command output").strip()
        raise LifecycleError(f"{' '.join(args)} failed ({process.returncode}): {detail}")
    return process.returncode, (stdout or "").strip(), (stderr or "").strip()


def copy_output(source, log):
    """Copy a command's output to the log and to our stderr until end of file.

    The log is the durable record, so a closed or broken stderr stops only the
    stream, never the copy or the drain that keeps the command from blocking.
    """
    stream = getattr(sys.stderr, "buffer", None)
    try:
        while chunk := os.read(source.fileno(), 65536):
            try:
                log.write(chunk)
                log.flush()
            except (OSError, ValueError):
                pass
            if stream is not None:
                try:
                    stream.write(chunk)
                    stream.flush()
                except (OSError, ValueError):
                    stream = None
    finally:
        source.close()


def run_logged(args, cwd, log):
    """Run a command, streaming its combined output to stderr and to log.

    Returns the exit code. Cancellation stops the command's process group and
    keeps the partial log.
    """
    check_cancelled()
    with log.open("wb") as file:
        process = subprocess.Popen(
            args,
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            pass_fds=tuple(_lock_fds),
            env=child_environment(),
        )
        pump = threading.Thread(target=copy_output, args=(process.stdout, file), daemon=True)
        pump.start()
        try:
            while True:
                check_cancelled()
                try:
                    process.wait(timeout=0.1)
                    break
                except subprocess.TimeoutExpired:
                    continue
        except BaseException:
            stop_process(process, drained=True)
            raise
        finally:
            # A leftover descendant would hold the pipe, and with it the pump, open.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            pump.join(timeout=10)
            if pump.is_alive():
                print(f"Output of {' '.join(args)} is still held open; the log may be incomplete",
                      file=sys.stderr, flush=True)
    return process.returncode


def summarize_ci_log(log, returncode):
    """Extract per-phase results and failing tests from a merge-gate log."""
    phases = []
    tests = []
    seen = set()
    last = ""
    try:
        with log.open(encoding="utf-8", errors="replace") as file:
            for raw in file:
                line = ANSI_ESCAPE.sub("", raw).rstrip("\r\n")
                if line.strip():
                    last = line.strip()
                if marker := PHASE_MARKER.fullmatch(line):
                    phases.append({"name": marker[1], "status": marker[2]})
                    continue
                if failure := NEXTEST_FAILURE.fullmatch(line):
                    name = f"{failure[2]} {failure[3]}"
                elif failure := LIBTEST_FAILURE.fullmatch(line):
                    name = failure[1]
                else:
                    continue
                # nextest repeats each failure in its final summary.
                if name not in seen:
                    seen.add(name)
                    tests.append(name)
    except FileNotFoundError:
        pass
    failed = [phase["name"] for phase in phases if phase["status"] == "failed"]
    parts = []
    if failed:
        parts.append("phases failed: " + ", ".join(failed))
    elif phases:
        parts.append(f"{len(phases)} phases passed")
    if tests:
        shown = ", ".join(tests[:5])
        parts.append(f"tests: {shown}" + (f" (+{len(tests) - 5} more)" if len(tests) > 5 else ""))
    if not phases:
        parts.append("no phase results" + (f"; last output: {last[:200]}" if last else ""))
    parts.append("interrupted" if returncode is None else f"exit {returncode}")
    result = {
        "exitCode": returncode,
        "phases": phases,
        "failedTests": tests[:FAILED_TEST_LIMIT],
        "summary": "; ".join(parts),
    }
    if len(tests) > FAILED_TEST_LIMIT:
        result["failedTestsTruncated"] = len(tests) - FAILED_TEST_LIMIT
    return result


def previous_attempts(receipts, issue_key, current):
    """Summaries of this issue's earlier failed attempts, oldest first."""
    found = []
    for candidate in sorted(receipts.glob("*.json")):
        if candidate.stem == current:
            continue
        try:
            value = json.loads(candidate.read_text())
        except (OSError, ValueError) as error:
            print(f"Skipping unreadable merge receipt {candidate.name}: {error}", file=sys.stderr, flush=True)
            continue
        if not isinstance(value, dict) or value.get("issueKey") != issue_key or value.get("status") != "failed":
            continue
        ci = value.get("ci") if isinstance(value.get("ci"), dict) else {}
        found.append({
            "attemptId": value.get("attemptId", candidate.stem),
            "timestamp": value.get("timestamp"),
            "failure": value.get("failure"),
            "ci": {key: ci[key] for key in ("status", "summary", "log", "exitCode", "phases", "failedTests") if key in ci},
        })
    return sorted(found, key=lambda entry: str(entry.get("timestamp") or ""))


def git(path, *args, check=True):
    return command(["git", "-C", str(path), *args], path, check=check)


def text_field(request, name):
    value = request.get(name)
    if not isinstance(value, str) or not value.strip() or "\x00" in value:
        raise LifecycleError(f"{name} must be a nonempty string")
    return value


def canonical_repo(path):
    path = Path(path).resolve(strict=True)
    top = Path(git(path, "rev-parse", "--show-toplevel")[1]).resolve(strict=True)
    common = git(top, "rev-parse", "--path-format=absolute", "--git-common-dir")[1]
    return top, Path(common).resolve(strict=True)


def inspect(request):
    repo, common = canonical_repo(text_field(request, "repoPath"))
    worktree, other_common = canonical_repo(request.get("worktree", str(repo)))
    if other_common != common:
        raise LifecycleError("The worktree belongs to another Git repository")
    branch = git(worktree, "symbolic-ref", "--quiet", "--short", "HEAD", check=False)[1]
    return {
        "commonDir": str(common),
        "worktree": str(worktree),
        "branch": branch,
        "head": git(worktree, "rev-parse", "HEAD")[1],
        "main": git(repo, "rev-parse", "refs/heads/main")[1],
        "clean": not git(worktree, "status", "--porcelain", "--untracked-files=all")[1]
        and not unfinished_operation(worktree),
    }


def branch_name(repo, branch):
    if branch == "main" or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/-]*", branch):
        raise LifecycleError("The issue branch must be a safe local branch other than main")
    git(repo, "check-ref-format", "--branch", branch)


def unfinished_operation(path):
    for operation in ("rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"):
        marker = Path(git(path, "rev-parse", "--git-path", operation)[1])
        if not marker.is_absolute():
            marker = path / marker
        if marker.exists():
            return operation
    return None


def clean(path):
    if git(path, "status", "--porcelain", "--untracked-files=all")[1]:
        raise LifecycleError(f"Worktree is not clean: {path}")
    operation = unfinished_operation(path)
    if operation:
        raise LifecycleError(f"Unfinished Git operation in {path}: {operation}")


def prepare(request):
    repo, common = canonical_repo(text_field(request, "repoPath"))
    desired = Path(text_field(request, "worktree"))
    if not desired.is_absolute():
        raise LifecycleError("worktree must be an absolute path")
    desired = desired.resolve()
    branch = text_field(request, "branch")
    issue_key = text_field(request, "issueKey")
    branch_name(repo, branch)
    with merge_lock(common, 60, name="worktree-ownership", label="worktree ownership") as directory:
        owners = read_owners(directory)
        assert_not_closing(owners, issue_key, str(desired), branch)
        assert_owner(owners, issue_key, str(desired), branch)
        if desired.exists():
            state = inspect({**request, "worktree": str(desired)})
            if Path(state["worktree"]) != desired or state["branch"] != branch:
                raise LifecycleError("Existing worktree does not match the requested branch and path")
        else:
            if git(repo, "show-ref", "--verify", "--quiet", f"refs/heads/{branch}", check=False)[0] == 0:
                raise LifecycleError("Branch already exists; supply its existing worktree to adopt it")
            desired.parent.mkdir(parents=True, exist_ok=True)
            git(repo, "worktree", "add", "-b", branch, str(desired), "refs/heads/main")
            state = inspect(request)
        owner = {"issueKey": issue_key, "worktree": str(desired), "branch": branch}

        def superseded(entry):
            # The full entry supersedes this issue's start claim on the same worktree.
            return entry == owner or (
                "branch" not in entry and entry["issueKey"] == issue_key and entry["worktree"] == str(desired)
            )

        if owner not in owners or any(superseded(entry) and entry != owner for entry in owners):
            owners = [entry for entry in owners if not superseded(entry)] + [owner]
            save_receipt(directory / "worktree-owners.json", {"owners": owners})
        return state


def claim_target(request):
    _, common = canonical_repo(text_field(request, "repoPath"))
    worktree, other_common = canonical_repo(text_field(request, "worktree"))
    if other_common != common:
        raise LifecycleError("The worktree belongs to another Git repository")
    return common, worktree, text_field(request, "issueKey")


@contextmanager
def issue_idle(directory, issue_key):
    """Yield whether another issue's lifecycle lock is free, holding it while True."""
    issues = directory / "issues"
    issues.mkdir(exist_ok=True)
    # The same lock withIssue holds for the whole of each lifecycle method.
    digest = hashlib.sha256(issue_key.encode()).hexdigest()
    with (issues / f"{digest}.lock").open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            yield False
            return
        try:
            yield True
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def claim(request):
    """Reserve the worker worktree for an issue before its lifecycle state exists.

    The claim stays pending until start saves state and confirms it. Another
    issue's start reclaims a pending claim whose issue is not running, because
    that issue's start ended without saving state.
    """
    common, worktree, issue_key = claim_target(request)
    branch = git(worktree, "symbolic-ref", "--quiet", "--short", "HEAD", check=False)[1]
    if branch == "main":
        raise LifecycleError("Run start from a feature worktree, not the main checkout")
    if not branch:
        # Implementation adopts this worktree's branch; a detached HEAD has none.
        raise LifecycleError("Run start from a worktree on a feature branch, not a detached HEAD")
    with merge_lock(common, 60, name="worktree-ownership", label="worktree ownership") as directory:
        owners = read_owners(directory)
        assert_not_closing(owners, issue_key, str(worktree), branch)
        conflicts = [
            owner for owner in owners
            if owner["issueKey"] != issue_key
            and (owner["worktree"] == str(worktree) or owner.get("branch") == branch)
        ]
        for owner in conflicts:
            if not owner.get("pending"):
                raise LifecycleError(
                    f"Worktree or branch already belongs to issue {owner['issueKey']} at {owner['worktree']}"
                )
        with ExitStack() as held:
            # flock is per open file, so lock each owning issue once.
            for owner_key in dict.fromkeys(owner["issueKey"] for owner in conflicts):
                if not held.enter_context(issue_idle(directory, owner_key)):
                    raise LifecycleError(
                        f"Worktree is being claimed by issue {owner_key}'s start in progress; retry later"
                    )
            # Start claims only when no state exists, under the issue lock, so this
            # issue's start claims elsewhere are orphans from a failed state save.
            kept = [
                owner for owner in owners
                if owner not in conflicts
                and not (owner["issueKey"] == issue_key and "branch" not in owner
                         and owner["worktree"] != str(worktree))
            ]
            if not any(owner["issueKey"] == issue_key and owner["worktree"] == str(worktree) for owner in kept):
                kept.append({"issueKey": issue_key, "worktree": str(worktree), "pending": True})
            if kept != owners:
                save_receipt(directory / "worktree-owners.json", {"owners": kept})
            for owner in conflicts:
                print(f"Reclaimed worktree {worktree} from issue {owner['issueKey']}, whose start never saved state",
                      file=sys.stderr, flush=True)
    return {"worktree": str(worktree)}


def confirm(request):
    """Make an issue's start claim permanent once its lifecycle state is saved."""
    common, worktree, issue_key = claim_target(request)
    with merge_lock(common, 60, name="worktree-ownership", label="worktree ownership") as directory:
        owners = read_owners(directory)
        assert_not_closing(owners, issue_key, str(worktree))
        mine = [owner for owner in owners if owner["issueKey"] == issue_key and owner["worktree"] == str(worktree)]
        if not mine:
            holder = next((owner["issueKey"] for owner in owners if owner["worktree"] == str(worktree)), None)
            raise LifecycleError(
                f"Worktree claim for issue {issue_key} at {worktree} was lost; it may have been reclaimed"
                + (f" by issue {holder}" if holder else " by another issue")
            )
        pending = [owner for owner in mine if owner.get("pending")]
        if pending:
            kept = [owner for owner in owners if owner not in pending]
            # A prepared or already confirmed entry keeps the claim permanent by itself.
            if len(pending) == len(mine):
                kept.append({"issueKey": issue_key, "worktree": str(worktree)})
            save_receipt(directory / "worktree-owners.json", {"owners": kept})
    return {"worktree": str(worktree)}


def read_owners(directory):
    path = directory / "worktree-owners.json"
    if not path.exists():
        return []
    value = json.loads(path.read_text())
    owners = value.get("owners") if isinstance(value, dict) else None
    # A start claim omits branch; prepare replaces it with a full entry. Only a
    # start claim may be pending, until start saves state and confirms it.
    if not isinstance(owners, list) or any(
        not isinstance(owner, dict) or any(
            not isinstance(owner.get(key), str) or not owner[key]
            for key in ("issueKey", "worktree", *(("branch",) if "branch" in owner else ()))
        ) or ("pending" in owner and (owner["pending"] is not True or "branch" in owner))
        or ("cleanupId" in owner and (not isinstance(owner["cleanupId"], str) or "branch" not in owner))
        for owner in owners
    ):
        raise LifecycleError("Invalid worktree ownership registry")
    return owners


def assert_owner(owners, issue_key, worktree, branch):
    for owner in owners:
        if "branch" not in owner:
            if owner["worktree"] == worktree and owner["issueKey"] != issue_key:
                raise LifecycleError(
                    f"Worktree already belongs to issue {owner['issueKey']} at {owner['worktree']}"
                )
        elif owner["worktree"] == worktree or owner["branch"] == branch:
            if owner != {"issueKey": issue_key, "worktree": worktree, "branch": branch}:
                raise LifecycleError(
                    f"Worktree or branch already belongs to issue {owner['issueKey']} at {owner['worktree']}"
                )


def main_worktree(repo):
    entries = git(repo, "worktree", "list", "--porcelain", "-z")[1].split("\x00\x00")
    matches = []
    for entry in entries:
        fields = entry.split("\x00")
        if "branch refs/heads/main" in fields:
            paths = [field[9:] for field in fields if field.startswith("worktree ")]
            matches.extend(Path(path).resolve(strict=True) for path in paths)
    if len(matches) != 1:
        raise LifecycleError("Local main must be checked out in exactly one worktree")
    clean(matches[0])
    return matches[0]


def digest_issue(workspace, item):
    content = {"workspace": workspace, "id": item["id"], "text": item["text"], "note": item["note"]}
    encoded = json.dumps(content, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(encoded.encode()).hexdigest()


def check_issue(request, repo):
    workspace = text_field(request, "agendaWorkspace")
    agenda_workspace(repo, workspace, run=lambda args: command(args, repo)[1])
    item_id = request.get("agendaItemId")
    if isinstance(item_id, bool) or not isinstance(item_id, int) or item_id <= 0:
        raise LifecycleError("agendaItemId must be a positive integer")
    _, output, _ = command(
        ["agenda", "--workspace", workspace, "--json", "item", "show", str(item_id)], repo
    )
    try:
        document = json.loads(output)
        item = document["item"]
        if type(document["schema_version"]) is not int or document["schema_version"] != 0 or type(item["id"]) is not int or item["id"] != item_id:
            raise LifecycleError("Agenda returned a different item or unsupported schema")
        if type(item["completed"]) is not bool or type(item["blocked"]) is not bool or not isinstance(item["text"], str) or not isinstance(item["note"], str) or not isinstance(item["assignments"], list):
            raise LifecycleError("Agenda returned invalid task fields")
        if item["kind"] != "task" or item["completed"] or item["discarded_at"] is not None or item["blocked"]:
            raise LifecycleError("Agenda issue must be a pending, unblocked task outside Trash")
        for assignment in item["assignments"]:
            if not isinstance(assignment, dict):
                raise LifecycleError("Agenda returned an invalid assignment")
            value = assignment.get("value")
            if value == "standard":
                continue
            if not isinstance(value, dict):
                raise LifecycleError("Agenda returned an invalid assignment value")
            date = value.get("date")
            if isinstance(date, dict) and date.get("recurrence") is not None:
                raise LifecycleError("Recurring Agenda tasks cannot be development issues")
            if set(value) == {"text"} and isinstance(value["text"], str):
                continue
            if set(value) == {"number"} and type(value["number"]) in (int, float) and math.isfinite(value["number"]):
                continue
            if set(value) == {"date"} and isinstance(date, dict) and "recurrence" in date:
                temporal = date.get("temporal")
                if isinstance(temporal, dict) and temporal.get("kind") in ("floating", "zoned", "instant"):
                    continue
            raise LifecycleError("Agenda returned an invalid typed assignment value")
        if digest_issue(workspace, item) != text_field(request, "issueDigest"):
            raise LifecycleError("Agenda issue description changed; reassess the plan before merging")
    except (KeyError, TypeError, ValueError) as error:
        raise LifecycleError(f"Invalid Agenda response: {error}") from error


def configured_workspace(request):
    repo = text_field(request, "repoPath")
    return {"workspace": agenda_workspace(
        repo, request.get("expectedWorkspace"), run=lambda args: command(args, repo)[1])}


def isolated_target(worktree):
    if os.environ.get("CARGO_TARGET_DIR") or os.environ.get("CARGO_BUILD_TARGET_DIR"):
        raise LifecycleError("Unset Cargo target directory overrides before integration")
    target = worktree / "target"
    if target.is_symlink() or (target.exists() and not target.is_dir()):
        raise LifecycleError("The worktree needs its own real target directory")
    # Cargo merges scalar settings from the nearest configuration first, then
    # ancestors and Cargo home. Shadowed target settings do not affect isolation.
    import tomllib
    locations = [path / ".cargo" for path in (worktree, *worktree.parents)]
    locations.append(worktree / Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))))
    effective = None
    for location in dict.fromkeys(locations):
        # Cargo prefers the legacy filename when both files exist.
        for filename in ("config", "config.toml"):
            config = location / filename
            if not config.is_file():
                continue
            try:
                build = tomllib.loads(config.read_text()).get("build", {})
                if not isinstance(build, dict):
                    raise ValueError("build must be a table")
                value = build.get("target-dir")
                if value is not None and not isinstance(value, str):
                    raise ValueError("build.target-dir must be a string")
            except (OSError, ValueError) as error:
                raise LifecycleError(f"Cannot inspect Cargo config {config}: {error}") from error
            if value is not None and effective is None:
                effective = (config, location.parent / value)
            break
    if effective is not None:
        config, configured_target = effective
        if configured_target.resolve() != target.resolve():
            raise LifecycleError(f"Cargo target-dir override in {config} would break build isolation")


@contextmanager
def merge_lock(common, wait_seconds=14400, *, name="merge", label="repository merge", busy_stage=None):
    directory = common / "yinz-lifecycle"
    directory.mkdir(exist_ok=True)
    # Keep this inode permanently: unlinking a lock path creates two lock domains.
    with (directory / f"{name}.lock").open("a+") as lock:
        waiting = False
        deadline = time.monotonic() + wait_seconds
        while True:
            check_cancelled()
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if busy_stage is not None:
                    raise LockBusy(busy_stage, label)
                if time.monotonic() >= deadline:
                    raise LifecycleError(f"Timed out waiting for the {label} lock; retry later")
                if not waiting:
                    print(f"Waiting for the {label} lock", file=sys.stderr, flush=True)
                    waiting = True
                time.sleep(0.05)
        try:
            _lock_fds.append(lock.fileno())
            print(f"Acquired the {label} lock", file=sys.stderr, flush=True)
            yield directory
        finally:
            _lock_fds.pop()
            fcntl.flock(lock, fcntl.LOCK_UN)


def attempt_path(directory, request):
    attempt = text_field(request, "attemptId")
    try:
        if str(uuid.UUID(attempt)) != attempt:
            raise ValueError("must use canonical lowercase UUID spelling")
    except ValueError as error:
        raise LifecycleError(f"Invalid attemptId: {error}") from error
    receipts = directory / "merges"
    receipts.mkdir(exist_ok=True)
    return receipts / f"{attempt}.json"


def attempt_log(path):
    """The merge-gate log beside an attempt's receipt."""
    directory = path.with_suffix("")
    directory.mkdir(exist_ok=True)
    return directory / CI_LOG_NAME


def save_receipt(path, receipt):
    receipt["timestamp"] = now()
    fd, temporary = tempfile.mkstemp(prefix=".receipt-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as file:
            json.dump(receipt, file, ensure_ascii=False, sort_keys=True)
            file.write("\n")
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def reconcile(repo, path):
    receipt = json.loads(path.read_text())
    validate_receipt(receipt)
    candidate = receipt.get("rebasedCommit")
    if receipt["status"] in ("verified", "merged") and candidate:
        contained = git(repo, "merge-base", "--is-ancestor", candidate, "refs/heads/main", check=False)[0] == 0
        if receipt["status"] == "merged" and not contained:
            raise LifecycleError("Recorded merge is no longer contained in local main")
        if contained and receipt["status"] == "verified":
            receipt.update(status="merged", mergedCommit=candidate, mergedAt=now())
            save_receipt(path, receipt)
    return receipt


def is_text_list(value):
    return isinstance(value, list) and all(isinstance(entry, str) for entry in value)


def validate_ci_details(ci):
    """Type-check the optional merge-gate details; older receipts omit them."""
    if not isinstance(ci, dict):
        raise LifecycleError("Merge receipt has an invalid CI result")
    for key, check in (
        ("log", lambda value: isinstance(value, str) and bool(value)),
        ("summary", lambda value: isinstance(value, str)),
        ("exitCode", lambda value: value is None or type(value) is int),
        ("failedTests", is_text_list),
        ("failedTestsTruncated", lambda value: type(value) is int and value > 0),
        ("phases", lambda value: isinstance(value, list) and all(
            isinstance(phase, dict) and set(phase) == {"name", "status"} and isinstance(phase["name"], str)
            and phase["status"] in ("passed", "failed") for phase in value)),
    ):
        if key in ci and not check(ci[key]):
            raise LifecycleError(f"Merge receipt has an invalid CI {key}")


def validate_receipt(receipt):
    if not isinstance(receipt, dict) or receipt.get("status") not in ("verified", "merged", "failed"):
        raise LifecycleError("Invalid merge receipt status")
    if "ci" in receipt:
        validate_ci_details(receipt["ci"])
    history = receipt.get("previousAttempts", [])
    if not isinstance(history, list) or any(
        not isinstance(entry, dict) or not isinstance(entry.get("attemptId"), str) for entry in history
    ):
        raise LifecycleError("Merge receipt has an invalid attempt history")
    for entry in history:
        if "ci" in entry:
            validate_ci_details(entry["ci"])
    if receipt["status"] in ("verified", "merged"):
        ci = receipt.get("ci", {})
        if not isinstance(ci, dict) or ci.get("command") != CI_COMMAND or ci.get("status") not in ("passed", "skipped"):
            raise LifecycleError("Merge receipt has no valid CI result")
        if ci["status"] == "skipped" and (not isinstance(ci.get("request"), str) or not ci["request"].strip()):
            raise LifecycleError("Merge receipt skips CI without an explicit request")
        for field in ("baseCommit", "originalCommit", "rebasedCommit"):
            if not isinstance(receipt.get(field), str) or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", receipt[field]):
                raise LifecycleError(f"Invalid commit in merge receipt: {field}")
        if receipt["rebasedCommit"] == receipt["baseCommit"]:
            raise LifecycleError("Merge receipt contains no changes beyond main")


def receipt_action(request):
    busy_stage = optional_lock_stage(request)
    repo, common = canonical_repo(text_field(request, "repoPath"))
    if request.get("failedOnly") is True:
        # A failed receipt is terminal and replaced atomically, so it needs no
        # lock. Waiting for one would block on the next queued merge's gate.
        directory = common / "yinz-lifecycle"
        directory.mkdir(exist_ok=True)
        path = attempt_path(directory, request)
        if not path.is_file():
            return {"status": "missing", "attemptId": request["attemptId"]}
        receipt = json.loads(path.read_text())
        validate_receipt(receipt)
        if receipt["status"] != "failed":
            return {"status": "not_failed", "attemptId": request["attemptId"]}
        return receipt
    with merge_lock(common, busy_stage=busy_stage) as directory:
        path = attempt_path(directory, request)
        if not path.is_file():
            return {"status": "missing", "attemptId": request["attemptId"]}
        return reconcile(repo, path)


def check_verified_worktree(worktree, receipt):
    clean(worktree)
    if git(worktree, "symbolic-ref", "--quiet", "--short", "HEAD")[1] != receipt["branch"]:
        raise LifecycleError("Issue worktree changed branches during integration")
    if git(worktree, "rev-parse", "HEAD")[1] != receipt["rebasedCommit"]:
        raise LifecycleError("Issue HEAD changed during integration")


def finish_merge(request, repo, worktree, path, receipt):
    validate_receipt(receipt)
    check_issue(request, repo)
    check_verified_worktree(worktree, receipt)
    target = main_worktree(repo)
    if git(repo, "rev-parse", "refs/heads/main")[1] != receipt["baseCommit"]:
        raise LifecycleError("Local main changed during integration; rebase and verify a new attempt")
    # Git updates the ref with its own compare-and-swap lock. ff-only additionally
    # refuses a conflicting manual main update outside the lifecycle's lock.
    git(target, "merge", "--ff-only", "--no-edit", receipt["rebasedCommit"])
    receipt.update(status="merged", mergedCommit=receipt["rebasedCommit"], mergedAt=now())
    save_receipt(path, receipt)
    return receipt


def integrate(request):
    repo, common = canonical_repo(text_field(request, "repoPath"))
    branch = text_field(request, "expectedBranch")
    branch_name(repo, branch)
    expected = text_field(request, "expectedHead")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", expected):
        raise LifecycleError("expectedHead must be a full commit hash")
    skip = request.get("skipCiRequest")
    if skip is not None and (not isinstance(skip, str) or not skip.strip()):
        raise LifecycleError("skipCiRequest must contain the explicit user request")
    with merge_lock(common) as directory:
        path = attempt_path(directory, request)
        state = inspect(request)
        worktree = Path(state["worktree"])
        assert_owner(read_owners(directory), text_field(request, "issueKey"), str(worktree), branch)
        if path.exists():
            previous = reconcile(repo, path)
            identity = {
                "issueKey": text_field(request, "issueKey"),
                "issueDigest": text_field(request, "issueDigest"),
                "worktree": str(worktree),
                "branch": branch,
                "originalCommit": expected,
            }
            if any(previous.get(key) != value for key, value in identity.items()):
                raise LifecycleError("Merge attempt belongs to different issue, worktree, or commit")
            if previous.get("ci", {}).get("request") != skip:
                raise LifecycleError("The explicit CI skip request changed for this merge attempt")
            if previous["status"] == "merged":
                return previous
            if previous["status"] == "failed":
                raise LifecycleError("This attempt failed; repair the issue and create a new attempt")
            return finish_merge(request, repo, worktree, path, previous)
        result = {
            "attemptId": request["attemptId"],
            "issueKey": text_field(request, "issueKey"),
            "issueDigest": text_field(request, "issueDigest"),
            "agendaWorkspace": text_field(request, "agendaWorkspace"),
            "agendaItemId": request.get("agendaItemId"),
            "worktree": str(worktree),
            "branch": branch,
            "baseCommit": state["main"],
            "originalCommit": expected,
            "rebasedCommit": None,
            "ci": {"status": "skipped" if skip else "not_run", "command": CI_COMMAND},
        }
        if skip:
            result["ci"]["request"] = skip
        history = previous_attempts(path.parent, result["issueKey"], request["attemptId"])
        if history:
            result["previousAttempts"] = history
        try:
            if state["branch"] != branch or state["head"] != expected:
                raise LifecycleError("Issue branch or HEAD differs from the reviewed commit")
            isolated_target(worktree)
            clean(worktree)
            main_worktree(repo)
            check_issue(request, repo)
            git(worktree, "rebase", "--no-autostash", result["baseCommit"])
            result["rebasedCommit"] = git(worktree, "rev-parse", "HEAD")[1]
            if result["rebasedCommit"] == result["baseCommit"]:
                raise LifecycleError("The issue branch has no changes beyond local main")
            clean(worktree)
            isolated_target(worktree)
            if not skip:
                log = attempt_log(path)
                result["ci"].update(status="running", log=str(log))
                code = None
                try:
                    code = run_logged(CI_COMMAND, worktree, log)
                finally:
                    # Record what the log shows even when the gate was interrupted.
                    result["ci"].update(summarize_ci_log(log, code))
                if code:
                    raise LifecycleError(f"{' '.join(CI_COMMAND)} failed: {result['ci']['summary']}; log: {log}")
                result["ci"]["status"] = "passed"
            # Only preserve reusable verification for the commit and clean
            # worktree that were checked, including changes made by CI itself.
            check_verified_worktree(worktree, result)
            result["status"] = "verified"
            save_receipt(path, result)
            return finish_merge(request, repo, worktree, path, result)
        except BaseException as error:
            # A verified receipt is the write-ahead record for an interrupted
            # merge. Keep it so receipt reconciliation can determine the result.
            if result.get("status") not in ("verified", "merged"):
                if result["ci"]["status"] == "running":
                    result["ci"]["status"] = "failed"
                # The lifecycle rejects an empty failure, which would drop this evidence.
                result.update(status="failed", failure=str(error) or type(error).__name__)
                save_receipt(path, result)
            raise


def worktree_matches(worktree, branch, head):
    """True when the worktree is still clean at the expected branch and commit."""
    try:
        clean(worktree)
        current = git(worktree, "symbolic-ref", "--quiet", "--short", "HEAD", check=False)[1]
        return current == branch and git(worktree, "rev-parse", "HEAD")[1] == head
    except Cancelled:
        raise
    except (LifecycleError, OSError):
        # An unborn HEAD, a removed worktree or a broken Git link is a change.
        return False


def run_check(request):
    """Run one approved plan check at the reviewed commit, outside the merge lock.

    A nonzero exit is a result, not a protocol failure. Output goes to stderr,
    which the caller stores as the check's log.
    """
    repo, common = canonical_repo(text_field(request, "repoPath"))
    worktree, other_common = canonical_repo(text_field(request, "worktree"))
    if other_common != common:
        raise LifecycleError("The worktree belongs to another Git repository")
    branch = text_field(request, "expectedBranch")
    branch_name(repo, branch)
    expected = text_field(request, "expectedHead")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", expected):
        raise LifecycleError("expectedHead must be a full commit hash")
    argv = request.get("argv")
    if (
        not isinstance(argv, list) or not argv
        or any(not isinstance(value, str) or "\x00" in value for value in argv)
        or not argv[0].strip()
    ):
        raise LifecycleError("argv must be a nonempty list of strings without NUL")
    timeout = request.get("timeoutSeconds", CHECK_TIMEOUT_SECONDS)
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not timeout > 0:
        raise LifecycleError("timeoutSeconds must be a positive number")
    if git(worktree, "symbolic-ref", "--quiet", "--short", "HEAD", check=False)[1] != branch:
        raise LifecycleError("Issue worktree is not on the reviewed branch")
    if git(worktree, "rev-parse", "HEAD")[1] != expected:
        raise LifecycleError("Issue HEAD differs from the reviewed commit")
    isolated_target(worktree)
    clean(worktree)
    print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
    started = time.monotonic()
    timed_out = False
    try:
        code = command(argv, worktree, check=False, capture=False, reap_group=True, timeout=timeout)[0]
    except subprocess.TimeoutExpired:
        # command() has already stopped the check's whole process group.
        print(f"Timed out after {timeout:g} s; stopped the check's process group", file=sys.stderr, flush=True)
        code = None
        timed_out = True
    except OSError as error:
        # Includes ENOEXEC and E2BIG, not only a missing or non-executable file.
        print(f"Cannot start {argv[0]}: {error}", file=sys.stderr, flush=True)
        code = None
    duration = int((time.monotonic() - started) * 1000)
    unchanged = worktree_matches(worktree, branch, expected)
    print(f"exit {code} after {duration} ms", file=sys.stderr, flush=True)
    if not unchanged:
        print("The check changed the worktree, branch or HEAD", file=sys.stderr, flush=True)
    result = {"exitCode": code, "durationMs": duration, "worktreeUnchanged": unchanged}
    if timed_out:
        result["timedOut"] = True
    return result



def assert_not_closing(owners, issue_key, worktree, branch=None):
    for owner in owners:
        if owner.get("cleanupId") and (owner["worktree"] == worktree or owner.get("branch") == branch):
            raise LifecycleError("Worktree ownership is reserved by closeout; resume that cleanup first")


def registered_worktrees(repo):
    entries = []
    for record in git(repo, "worktree", "list", "--porcelain", "-z")[1].split("\x00\x00"):
        fields = dict(field.split(" ", 1) if " " in field else (field, "")
                      for field in record.split("\x00") if field)
        if "worktree" in fields:
            entries.append(fields)
    return entries


def filesystem_identity(path):
    stat = path.stat()
    return {"device": str(stat.st_dev), "inode": str(stat.st_ino)}


def branch_identity(common, branch):
    """Pin ref storage and reflog, including same-OID deletion/recreation on resume.

    Ref changes outside lifecycle locks are not excluded. Any storage change is
    conservatively stale; compare-and-delete still protects the final Git update.
    """
    entries = []
    loose = common / "refs" / "heads" / branch
    for path in [loose, common / "logs" / "refs" / "heads" / branch,
                 *([] if loose.exists() else [common / "packed-refs"])]:
        if path.is_symlink():
            raise LifecycleError("Closeout refuses symlinked ref storage")
        if path.exists():
            stat = path.stat()
            entries.append([str(path), stat.st_dev, stat.st_ino, stat.st_mtime_ns,
                            stat.st_ctime_ns, hashlib.sha256(path.read_bytes()).hexdigest()])
        else:
            entries.append([str(path), None])
    return hashlib.sha256(json.dumps(entries, sort_keys=True).encode()).hexdigest()


def closeout_merge(request, repo, directory):
    path = attempt_path(directory, request)
    if not path.is_file():
        raise LifecycleError("Closeout requires a durable merged receipt")
    receipt = reconcile(repo, path)
    if receipt["status"] != "merged" or any(receipt.get(key) != text_field(request, key)
                                              for key in ("issueKey", "issueDigest")):
        raise LifecycleError("Closeout merge receipt does not match this issue and attempt")
    if receipt.get("mergedCommit") != receipt["rebasedCommit"]:
        raise LifecycleError("Closeout merge receipt has a different merged commit")
    branch_name(repo, receipt["branch"])
    return receipt


def closeout_target(request, repo, common, directory, receipt):
    desired = Path(receipt["worktree"])
    if not desired.is_absolute() or desired.resolve() != desired:
        raise LifecycleError("Closeout worktree must have its recorded canonical path")
    entries = registered_worktrees(repo)
    matches = [entry for entry in entries if Path(entry["worktree"]).resolve() == desired]
    if len(matches) != 1 or not desired.is_dir():
        raise LifecycleError("Closeout worktree is absent or not registered; cannot offer deletion")
    entry = matches[0]
    if any(key in entry for key in ("locked", "prunable", "bare")):
        raise LifecycleError("Closeout worktree is locked, prunable or bare")
    # The issue lock is already held by Swamp. Lock order: issue -> merge -> ownership.
    # These locks do not exclude arbitrary manual Git or external processes.
    protected = [repo, common, (repo / ".swamp").resolve(), (repo / ".swamp.yaml").resolve(),
                 (repo / ".swamp" / "data").resolve(),
                 *[Path(e["worktree"]).resolve() for e in entries if e is not entry]]
    if any(target == desired or desired in target.parents for target in protected):
        raise LifecycleError("Closeout would remove the control/store, main or another worktree")
    worktree, other_common = canonical_repo(desired)
    gitdir = Path(git(worktree, "rev-parse", "--absolute-git-dir")[1]).resolve(strict=True)
    if worktree != desired or other_common != common or gitdir == common or gitdir.parent != common / "worktrees":
        raise LifecycleError("Closeout requires an exact registered linked worktree")
    branch = receipt["branch"]
    oid = receipt["mergedCommit"]
    if entry.get("branch") != f"refs/heads/{branch}" or entry.get("HEAD") != oid:
        raise LifecycleError("Closeout worktree branch or HEAD changed")
    if any(e is not entry and e.get("branch") == f"refs/heads/{branch}" for e in entries):
        raise LifecycleError("Closeout branch has another linked checkout")
    if (desired / ".git").is_symlink():
        raise LifecycleError("Closeout refuses a symlinked Git registration link")
    clean(desired)
    if (desired / ".gitmodules").exists():
        raise LifecycleError("Closeout refuses worktrees with submodules; retain for manual inspection")
    if git(repo, "rev-parse", f"refs/heads/{branch}")[1] != oid:
        raise LifecycleError("Closeout branch moved")
    owners = read_owners(directory)
    mine = {"issueKey": receipt["issueKey"], "worktree": str(desired), "branch": branch}
    matching = [owner for owner in owners if owner["worktree"] == str(desired) or owner.get("branch") == branch]
    if matching != [mine] and matching != [{**mine, "cleanupId": request.get("cleanupId")}]:
        raise LifecycleError("Closeout requires the exact prepared ownership claim")
    return {"worktree": str(desired), "branch": branch, "head": oid,
            "branchIdentity": branch_identity(common, branch),
            "gitDir": str(gitdir), "directoryIdentity": filesystem_identity(desired),
            "registrationIdentity": filesystem_identity(gitdir),
            "linkIdentity": filesystem_identity(desired / ".git"),
            "gitLink": (desired / ".git").read_text(), "owner": mine}


def closeout_preview(request):
    busy_stage = optional_lock_stage(request)
    repo, common = canonical_repo(text_field(request, "repoPath"))
    with merge_lock(common, busy_stage=busy_stage) as directory:
        with merge_lock(common, 60, name="worktree-ownership", label="worktree ownership", busy_stage=busy_stage):
            receipt = closeout_merge(request, repo, directory)
            return closeout_target(request, repo, common, directory, receipt)


def closeout_cleanup(request):
    busy_stage = optional_lock_stage(request)
    repo, common = canonical_repo(text_field(request, "repoPath"))
    cleanup_id = text_field(request, "cleanupId")
    if str(uuid.UUID(cleanup_id)) != cleanup_id:
        raise LifecycleError("cleanupId must be a canonical UUID")
    with merge_lock(common, busy_stage=busy_stage) as directory:
        with merge_lock(common, 60, name="worktree-ownership", label="worktree ownership", busy_stage=busy_stage):
            merge = closeout_merge(request, repo, directory)
            receipts = directory / "closeouts"
            receipts.mkdir(exist_ok=True)
            path = receipts / f"{cleanup_id}.json"
            target = request.get("target")
            binding = {key: request.get(key) for key in ("issueKey", "issueDigest", "attemptId", "cleanupId")}
            if path.exists():
                saved = json.loads(path.read_text())
                if any(saved.get(key) != value for key, value in binding.items()) or saved["target"] != target:
                    raise LifecycleError("Closeout receipt belongs to another authorization or target")
            else:
                current = closeout_target(request, repo, common, directory, merge)
                if current != target:
                    raise LifecycleError("Closeout offer is stale; worktree identity changed")
                saved = {**binding, "target": current, "worktree": "pending", "branch": "pending"}
                save_receipt(path, saved)
            if target["worktree"] != merge["worktree"] or target["branch"] != merge["branch"] or target["head"] != merge["mergedCommit"]:
                raise LifecycleError("Closeout targets differ from the merged receipt")
            desired = Path(target["worktree"])
            branch = target["branch"]
            oid = target["head"]
            owners = read_owners(directory)
            mine = target["owner"]
            reserved = {**mine, "cleanupId": cleanup_id}
            if saved["branch"] != "removed":
                matches = [o for o in owners if o["worktree"] == str(desired) or o.get("branch") == branch]
                if matches not in ([mine], [reserved]):
                    raise LifecycleError("Closeout ownership changed")
                if matches == [mine]:
                    owners = [reserved if o == mine else o for o in owners]
                    save_receipt(directory / "worktree-owners.json", {"owners": owners})
            # Never reconcile absence from the parent repository's HEAD.
            if desired.exists() or desired.is_symlink():
                if saved["worktree"] == "removed":
                    raise LifecycleError("Closeout worktree path was reused")
                current = closeout_target(request, repo, common, directory, merge)
                if current != target:
                    raise LifecycleError("Closeout worktree identity changed")
                saved["worktree"] = "removing"
                save_receipt(path, saved)
                git(repo, "worktree", "remove", "--", str(desired))
            elif saved["worktree"] not in ("removing", "removed"):
                raise LifecycleError("Worktree disappeared before closeout removal intent")
            if any(Path(e["worktree"]).resolve() == desired for e in registered_worktrees(repo)) or Path(target["gitDir"]).exists():
                raise LifecycleError("Closeout registration survived or was recreated")
            saved["worktree"] = "removed"
            save_receipt(path, saved)
            if desired.exists() or desired.is_symlink():
                raise LifecycleError("Closeout worktree path was reused before branch deletion")
            if any(e.get("branch") == f"refs/heads/{branch}" for e in registered_worktrees(repo)):
                raise LifecycleError("Closeout branch is checked out elsewhere")
            code, current, _ = git(repo, "rev-parse", "--verify", f"refs/heads/{branch}", check=False)
            if code == 0:
                if saved["branch"] == "removed" or current != oid:
                    raise LifecycleError("Closeout branch moved or was recreated")
                if branch_identity(common, branch) != target["branchIdentity"]:
                    raise LifecycleError("Closeout branch storage identity changed or was recreated")
                if git(repo, "merge-base", "--is-ancestor", oid, "refs/heads/main", check=False)[0] != 0:
                    raise LifecycleError("Closeout branch is no longer merged in main")
                saved["branch"] = "deleting"
                save_receipt(path, saved)
                git(repo, "update-ref", "-d", f"refs/heads/{branch}", oid)
            elif saved["branch"] not in ("deleting", "removed"):
                raise LifecycleError("Branch disappeared before closeout deletion intent")
            saved["branch"] = "removed"
            save_receipt(path, saved)
            owners = read_owners(directory)
            if any(o != reserved and (o["worktree"] == str(desired) or o.get("branch") == branch) for o in owners):
                raise LifecycleError("Closeout ownership was reused")
            if reserved in owners:
                save_receipt(directory / "worktree-owners.json", {"owners": [o for o in owners if o != reserved]})
            return saved


def closeout_failure_receipt(request):
    """Return only this exact authorization's last atomic helper progress."""
    _, common = canonical_repo(text_field(request, "repoPath"))
    cleanup_id = text_field(request, "cleanupId")
    if str(uuid.UUID(cleanup_id)) != cleanup_id:
        return None
    path = common / "yinz-lifecycle" / "closeouts" / f"{cleanup_id}.json"
    if not path.is_file():
        return None
    saved = json.loads(path.read_text())
    if any(saved.get(key) != request.get(key) for key in ("issueKey", "issueDigest", "attemptId", "cleanupId", "target")):
        return None
    return saved


def main():
    signal.signal(signal.SIGINT, cancel)
    signal.signal(signal.SIGTERM, cancel)
    request = {}
    try:
        request = json.load(sys.stdin)
        if not isinstance(request, dict):
            raise LifecycleError("Request must be a JSON object")
        actions = {"inspect": inspect, "claim": claim, "confirm": confirm, "prepare": prepare, "integrate": integrate,
                   "receipt": receipt_action, "agenda-workspace": configured_workspace,
                   "check": run_check, "closeout-preview": closeout_preview, "closeout-cleanup": closeout_cleanup}
        action = request.get("action")
        if not isinstance(action, str) or action not in actions:
            raise LifecycleError(f"Unknown action: {action}")
        optional_lock_stage(request)
        result = actions[action](request)
        print(json.dumps(result, ensure_ascii=False, sort_keys=True))
        return 0
    except (LifecycleError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        result = {"error": str(error)}
        if isinstance(error, LockBusy):
            result.update(code="lock_busy", stage=error.stage, lock=error.label)
        if isinstance(request, dict) and request.get("action") == "closeout-cleanup":
            try:
                progress = closeout_failure_receipt(request)
                if progress is not None:
                    result["closeout"] = progress
            except (LifecycleError, OSError, ValueError, subprocess.SubprocessError):
                pass  # Failure to observe progress never guesses a completed step.
        print(json.dumps(result, ensure_ascii=False))
        return 130 if isinstance(error, Cancelled) else 1


if __name__ == "__main__":
    sys.exit(main())
