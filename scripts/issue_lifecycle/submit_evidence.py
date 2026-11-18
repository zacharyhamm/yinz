#!/usr/bin/env python3
"""Submit one lifecycle worker's evidence and confirm acceptance in one step.

Run with ``python3 -I``. Standard library only. The helper is bound to the task
whose private assignment directory it is given: the Swamp behavior and control
directory come from that directory's ``target.json``, never from the command
line, and nothing is sent unless every assignment file matches its manifest and
the target's assignment digest equals ``--assignment-digest``.

Exit codes: 0 accepted; 1 rejected, superseded or error; 2 refused before
sending anything; 3 (``--status`` only) still open.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

TARGET_KEYS = {"protocol", "taskId", "role", "behaviorName", "controlDir", "assignmentDigest"}
ROLES = {
    "triage", "plan", "review_plan", "implement", "review_code", "review_ux",
    "plan_review_fixes", "implement_review_fixes",
}
FILE_NAMES = {
    "target": "target.json",
    "snapshot": "snapshot.json",
    "packet": "packet.json",
    "template": "evidence-template.json",
    "schema": "evidence-schema.json",
    "baseline": "baseline.json",
    "featureDiff": "feature.diff",
    "delta": "delta.diff",
}
REQUIRED_FILES = {"target", "snapshot", "packet", "template", "schema"}
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
DIGEST = re.compile(r"^[0-9a-f]{64}$")
# A shell configured for a remote Swamp server must not redirect this task.
SWAMP_ENVIRONMENT = ("SWAMP_SERVE_URL", "SWAMP_SERVER_URL", "SWAMP_REPO_DIR", "SWAMP_WORKFLOWS_DIR")
OUTPUT_LIMIT = 4000


class Refused(Exception):
    """A check failed before anything was sent (exit 2)."""


def emit(value: dict) -> None:
    print(json.dumps(value, sort_keys=True), flush=True)


def read_json(path: Path, label: str):
    try:
        return json.loads(path.read_bytes())
    except (OSError, ValueError) as error:
        raise Refused(f"Cannot read {label} {path}: {error}") from error


def load_target(task_dir: Path, digest: str) -> dict:
    target = read_json(task_dir / FILE_NAMES["target"], "assignment target")
    if not isinstance(target, dict) or set(target) != TARGET_KEYS:
        raise Refused("target.json must contain exactly " + ", ".join(sorted(TARGET_KEYS)))
    task_id = target["taskId"]
    if target["protocol"] != 1:
        raise Refused("Unsupported submission protocol in target.json")
    if not isinstance(task_id, str) or not UUID.match(task_id):
        raise Refused("target.json taskId is not a UUID")
    if target["behaviorName"] != f"issue-evidence-{task_id}":
        raise Refused("target.json behaviorName does not belong to its task")
    if task_dir.name != task_id:
        raise Refused("The task directory does not belong to target.json's task")
    if target["role"] not in ROLES:
        raise Refused("target.json names an unknown role")
    if not isinstance(target["controlDir"], str) or not os.path.isabs(target["controlDir"]):
        raise Refused("target.json controlDir must be an absolute path")
    if not isinstance(target["assignmentDigest"], str) or not DIGEST.match(target["assignmentDigest"]):
        raise Refused("target.json assignmentDigest is malformed")
    if target["assignmentDigest"] != digest:
        raise Refused("--assignment-digest does not match this task's assignment; nothing was sent")
    return target


def verify_manifest(task_dir: Path) -> None:
    manifest = read_json(task_dir / "manifest.json", "assignment manifest")
    if not isinstance(manifest, dict) or set(manifest) != {"version", "directory", "files"} or manifest["version"] != 1:
        raise Refused("manifest.json is not a version 1 assignment manifest")
    if manifest["directory"] != str(task_dir):
        raise Refused("manifest.json describes a different assignment directory")
    files = manifest["files"]
    if not isinstance(files, dict) or not REQUIRED_FILES <= set(files) or not set(files) <= set(FILE_NAMES):
        raise Refused("manifest.json lists unexpected assignment files")
    for key, entry in sorted(files.items()):
        name = FILE_NAMES[key]
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256", "bytes"} or entry["path"] != str(task_dir / name):
            raise Refused(f"manifest.json entry {key} is malformed")
        try:
            data = (task_dir / name).read_bytes()
        except OSError as error:
            raise Refused(f"Assignment file {name} is unreadable: {error}") from error
        if len(data) != entry["bytes"] or hashlib.sha256(data).hexdigest() != entry["sha256"]:
            raise Refused(f"Assignment file {name} differs from its manifest; nothing was sent")


def load_evidence(source: str, target: dict) -> dict:
    try:
        text = sys.stdin.read() if source == "-" else Path(source).read_text()
        value = json.loads(text)
    except (OSError, ValueError) as error:
        raise Refused(f"Cannot read evidence JSON: {error}") from error
    if not isinstance(value, dict):
        raise Refused("Evidence must be a JSON object {role, evidence}")
    unknown = set(value) - {"role", "evidence", "taskId", "assignmentDigest"}
    if unknown:
        raise Refused("Evidence has unexpected keys: " + ", ".join(sorted(unknown)))
    if value.get("role") != target["role"]:
        raise Refused(f"Evidence role must be {target['role']!r} for this task")
    if not isinstance(value.get("evidence"), dict):
        raise Refused("Evidence must contain an evidence object")
    for key in ("taskId", "assignmentDigest"):
        if key in value and value[key] != target[key]:
            raise Refused(f"Evidence {key} does not match this task")
    return {
        "taskId": target["taskId"],
        "assignmentDigest": target["assignmentDigest"],
        "role": target["role"],
        "evidence": value["evidence"],
    }


def commands(target: dict) -> tuple[list[str], list[str]]:
    repo = ["--repo-dir", target["controlDir"]]
    submit = ["swamp", "behavior", "run", target["behaviorName"], "submit", *repo, "--stdin", "--literal-inputs", "--json"]
    query_text = f'behavior("{target["behaviorName"]}") && name == "submission"'
    query = ["swamp", "data", "query", query_text, *repo, "--select", "content", "--single", "--json"]
    return submit, query


def environment() -> dict:
    return {key: value for key, value in os.environ.items() if key not in SWAMP_ENVIRONMENT}


def run(command: list[str], stdin: str | None = None) -> subprocess.CompletedProcess:
    # A command without input never inherits ours: an open caller pipe would
    # otherwise block a child that reads stdin until the timeout.
    source = {"input": stdin} if stdin is not None else {"stdin": subprocess.DEVNULL}
    try:
        return subprocess.run(command, **source, text=True, capture_output=True, env=environment(), timeout=600)
    except (OSError, subprocess.TimeoutExpired) as error:
        return subprocess.CompletedProcess(command, 1, "", str(error))


def error_message(result: subprocess.CompletedProcess) -> str:
    raw = (result.stdout + ("\n" if result.stdout and result.stderr else "") + result.stderr).strip()
    for text in (result.stdout, result.stderr, raw):
        try:
            value = json.loads(text)
        except ValueError:
            continue
        if isinstance(value, dict):
            error = value.get("error")
            if isinstance(error, dict) and isinstance(error.get("message"), str):
                return error["message"]
            if isinstance(error, str):
                return error
            if isinstance(value.get("message"), str):
                return value["message"]
    return raw[-OUTPUT_LIMIT:] or f"exit {result.returncode}"


def query_submission(target: dict):
    """Returns (status, submission or None, error or None)."""
    result = run(commands(target)[1])
    if result.returncode != 0:
        return "unknown", None, error_message(result)
    try:
        submission = json.loads(result.stdout)
    except ValueError:
        return "unknown", None, "Swamp returned no submission JSON: " + result.stdout.strip()[-OUTPUT_LIMIT:]
    if not isinstance(submission, dict) or submission.get("status") not in ("open", "accepted", "superseded"):
        return "unknown", None, "The submission slot has an unknown shape"
    return submission["status"], submission, None


def accepted_line(target: dict, submission: dict):
    receipt = submission.get("receipt")
    if (
        submission.get("status") != "accepted" or not isinstance(receipt, dict)
        or receipt.get("taskId") != target["taskId"]
        or receipt.get("assignmentDigest") != target["assignmentDigest"]
    ):
        return None
    return {
        "status": "accepted",
        "taskId": target["taskId"],
        "role": target["role"],
        "assignmentDigest": target["assignmentDigest"],
        "payloadDigest": receipt.get("payloadDigest"),
        "acceptedAt": receipt.get("acceptedAt"),
    }


def status(target: dict) -> int:
    state, submission, error = query_submission(target)
    line = accepted_line(target, submission) if submission else None
    if line:
        emit(line)
        return 0
    if state == "open":
        emit({"status": "open", "taskId": target["taskId"], "role": target["role"]})
        return 3
    emit({"status": state if state == "superseded" else "error", "error": error or f"Submission is {state} without this task's receipt", "submission": state})
    return 1


def submit(target: dict, envelope: dict) -> int:
    result = run(commands(target)[0], json.dumps(envelope))
    if result.returncode != 0 and (result.stdout or result.stderr):
        sys.stderr.write(result.stdout + result.stderr)
    state, submission, query_error = query_submission(target)
    line = accepted_line(target, submission) if submission else None
    # Success needs both: an accepted receipt seen after a failed submit is a
    # conflict with earlier evidence (or a lost acknowledgement), never this one.
    if result.returncode == 0 and line:
        emit(line)
        return 0
    if result.returncode != 0:
        error = error_message(result)
    elif query_error:
        error = "Submitted, but acceptance could not be confirmed: " + query_error + ". Rerun with --status."
    else:
        error = f"Submit returned without this task's accepted receipt (submission is {state})"
    emit({"status": "rejected", "error": error, "submission": state})
    return 1


def print_commands(target: dict, task_dir: Path) -> int:
    submit_command, query_command = commands(target)
    prefix = ["env"] + [part for name in SWAMP_ENVIRONMENT for part in ("-u", name)]
    envelope = {"taskId": target["taskId"], "assignmentDigest": target["assignmentDigest"], "role": target["role"], "evidence": "<the evidence object>"}
    print("# Write this envelope, with your evidence object, to a file outside the source tree:")
    print("# " + json.dumps(envelope))
    print(f"# Start from {task_dir / FILE_NAMES['template']} (its evidence value) and {task_dir / FILE_NAMES['schema']}.")
    print(shlex.join(prefix + submit_command) + " < ENVELOPE.json")
    print("# Acceptance, including after a lost acknowledgement:")
    print(shlex.join(prefix + query_command))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Submit this task's lifecycle evidence and confirm acceptance.")
    parser.add_argument("--task-dir", required=True, help="this task's private assignment directory")
    parser.add_argument("--assignment-digest", required=True, help="the assignment digest from the task prompt")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("evidence", nargs="?", help="evidence JSON {role, evidence}, or - for stdin")
    mode.add_argument("--status", action="store_true", help="report acceptance without submitting")
    mode.add_argument("--print-commands", action="store_true", help="print the equivalent swamp commands; send nothing")
    args = parser.parse_args(argv)
    try:
        if not DIGEST.match(args.assignment_digest):
            raise Refused("--assignment-digest must be 64 lowercase hex characters")
        try:
            task_dir = Path(args.task_dir).resolve(strict=True)
        except OSError as error:
            raise Refused(f"Task directory does not exist: {error}") from error
        if not task_dir.is_dir():
            raise Refused("Task directory is not a directory")
        target = load_target(task_dir, args.assignment_digest)
        verify_manifest(task_dir)
        if args.print_commands:
            return print_commands(target, task_dir)
        if args.status:
            return status(target)
        envelope = load_evidence(args.evidence, target)
    except Refused as error:
        sys.stderr.write(f"{error}\n")
        emit({"status": "refused", "error": str(error)})
        return 2
    return submit(target, envelope)


if __name__ == "__main__":
    sys.exit(main())
