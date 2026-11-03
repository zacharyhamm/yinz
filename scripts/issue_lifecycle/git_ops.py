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
import signal
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from datetime import datetime, timezone
import uuid


class LifecycleError(Exception):
    """An operation failed without discarding the user's work."""


class Cancelled(LifecycleError):
    """The caller cancelled the active operation."""


_cancelled = False
_lock_fd = None
CI_COMMAND = ["cargo", "x", "ci", "check"]


def now():
    return datetime.now(timezone.utc).isoformat()


def cancel(_signum, _frame):
    global _cancelled
    _cancelled = True


def check_cancelled():
    if _cancelled:
        raise Cancelled("Operation cancelled; inspect the worktree before retrying")


def stop_process(process):
    """Reap the command group before the caller releases its merge lock."""
    for sig, timeout in ((signal.SIGTERM, 3), (signal.SIGKILL, 3)):
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            pass
        try:
            process.communicate(timeout=timeout)
            # Descendants may outlive a shell which exits on SIGTERM.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            return
        except subprocess.TimeoutExpired:
            continue
    process.communicate()


def command(args, cwd, *, check=True, capture=True):
    check_cancelled()
    environment = {**os.environ, "GIT_EDITOR": "/usr/bin/true", "GIT_SEQUENCE_EDITOR": "/usr/bin/true"}
    for variable in (
        "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_NAMESPACE",
    ):
        environment.pop(variable, None)
    # Children do not inherit stdin: every protocol request belongs to us.
    process = subprocess.Popen(
        args,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE if capture else sys.stderr,
        stderr=subprocess.PIPE if capture else sys.stderr,
        text=True,
        start_new_session=True,
        pass_fds=() if _lock_fd is None else (_lock_fd,),
        env=environment,
    )
    try:
        while True:
            check_cancelled()
            try:
                stdout, stderr = process.communicate(timeout=0.1)
                break
            except subprocess.TimeoutExpired:
                continue
    except BaseException:
        stop_process(process)
        raise
    if check and process.returncode:
        detail = (stderr or stdout or "see command output").strip()
        raise LifecycleError(f"{' '.join(args)} failed ({process.returncode}): {detail}")
    return process.returncode, (stdout or "").strip(), (stderr or "").strip()


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
        if owner not in owners:
            owners.append(owner)
            save_receipt(directory / "worktree-owners.json", {"owners": owners})
        return state


def read_owners(directory):
    path = directory / "worktree-owners.json"
    if not path.exists():
        return []
    value = json.loads(path.read_text())
    owners = value.get("owners") if isinstance(value, dict) else None
    if not isinstance(owners, list) or any(
        not isinstance(owner, dict) or any(
            not isinstance(owner.get(key), str) or not owner[key]
            for key in ("issueKey", "worktree", "branch")
        ) for owner in owners
    ):
        raise LifecycleError("Invalid worktree ownership registry")
    return owners


def assert_owner(owners, issue_key, worktree, branch):
    for owner in owners:
        if owner["worktree"] == worktree or owner["branch"] == branch:
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
def merge_lock(common, wait_seconds=14400, *, name="merge", label="repository merge"):
    global _lock_fd
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
                if time.monotonic() >= deadline:
                    raise LifecycleError(f"Timed out waiting for the {label} lock; retry later")
                if not waiting:
                    print(f"Waiting for the {label} lock", file=sys.stderr, flush=True)
                    waiting = True
                time.sleep(0.05)
        try:
            _lock_fd = lock.fileno()
            print(f"Acquired the {label} lock", file=sys.stderr, flush=True)
            yield directory
        finally:
            _lock_fd = None
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


def validate_receipt(receipt):
    if not isinstance(receipt, dict) or receipt.get("status") not in ("verified", "merged", "failed"):
        raise LifecycleError("Invalid merge receipt status")
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
    repo, common = canonical_repo(text_field(request, "repoPath"))
    with merge_lock(common) as directory:
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
                result["ci"]["status"] = "running"
                command(CI_COMMAND, worktree, capture=False)
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
                result.update(status="failed", failure=str(error))
                save_receipt(path, result)
            raise


def main():
    signal.signal(signal.SIGINT, cancel)
    signal.signal(signal.SIGTERM, cancel)
    try:
        request = json.load(sys.stdin)
        if not isinstance(request, dict):
            raise LifecycleError("Request must be a JSON object")
        actions = {"inspect": inspect, "prepare": prepare, "integrate": integrate, "receipt": receipt_action}
        action = request.get("action")
        if not isinstance(action, str) or action not in actions:
            raise LifecycleError(f"Unknown action: {action}")
        result = actions[action](request)
        print(json.dumps(result, ensure_ascii=False, sort_keys=True))
        return 0
    except (LifecycleError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        print(json.dumps({"error": str(error)}, ensure_ascii=False))
        return 130 if isinstance(error, Cancelled) else 1


if __name__ == "__main__":
    sys.exit(main())
