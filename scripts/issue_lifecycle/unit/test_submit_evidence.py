"""Exercise the one-step evidence helper against a fake swamp on PATH."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "submit_evidence.py"
TASK = "a6da10b0-c980-4b82-b920-000000000001"
DIGEST = "d" * 64

# A stateful fake: submit records evidence once (identical replays return the
# original receipt), query prints the slot. Modes come from fake-state.json.
FAKE_SWAMP = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ["FAKE_SWAMP_ROOT"])
state_file = root / "fake-state.json"
state = json.loads(state_file.read_text())
stdin = sys.stdin.read()
with (root / "calls.jsonl").open("a") as log:
    log.write(json.dumps({"argv": sys.argv[1:], "stdin": stdin, "env": {k: v for k, v in os.environ.items() if k.startswith("SWAMP_")}}) + "\n")
args = sys.argv[1:]
if args[:2] == ["behavior", "run"]:
    mode = state.get("submit", "accept")
    if mode == "reject":
        print(json.dumps({"error": {"message": "evidence.summary: Too small: expected string to have >=1 characters"}}))
        sys.exit(1)
    envelope = json.loads(stdin)
    slot = state["slot"]
    if slot["status"] == "superseded":
        print(json.dumps({"error": {"message": "Submission was superseded"}}))
        sys.exit(1)
    if slot["status"] == "accepted":
        if slot["evidence"] != envelope["evidence"]:
            print(json.dumps({"error": {"message": "Changed evidence after acceptance is rejected"}}))
            sys.exit(1)
    else:
        state["slot"] = {"status": "accepted", "evidence": envelope["evidence"], "receipt": {
            "taskId": envelope["taskId"], "assignmentDigest": envelope["assignmentDigest"],
            "payloadDigest": "e" * 64, "acceptedAt": "2026-11-14T00:00:0%dZ" % state.get("submits", 0)}}
    state["submits"] = state.get("submits", 0) + 1
    state_file.write_text(json.dumps(state))
    if mode == "lost_ack":
        state["submit"] = "accept"
        state_file.write_text(json.dumps(state))
        sys.stderr.write("connection reset\n")
        sys.exit(1)
    print(json.dumps({"dataHandles": []}))
    sys.exit(0)
if args[:2] == ["data", "query"]:
    if state.get("query") == "fail":
        sys.stderr.write("lock timeout\n")
        sys.exit(1)
    slot = dict(state["slot"])
    slot.pop("evidence", None)
    slot["assignment"] = {"taskId": state.get("assignmentTask", "")}
    print(json.dumps(slot))
    sys.exit(0)
sys.exit("unexpected swamp call: " + " ".join(args))
'''


class SubmitEvidenceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="yinz-submit-evidence-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        swamp = self.bin / "swamp"
        swamp.write_text(FAKE_SWAMP)
        swamp.chmod(0o755)
        self.control = self.root / "control"
        self.control.mkdir()
        self.task_dir = self.root / "common" / "yinz-lifecycle" / "assignments" / TASK
        self.task_dir.mkdir(parents=True, mode=0o700)
        self.target = {
            "protocol": 1, "taskId": TASK, "role": "review_code", "behaviorName": f"issue-evidence-{TASK}",
            "controlDir": str(self.control), "assignmentDigest": DIGEST,
        }
        self.write_assignment()
        self.set_state(slot={"status": "open"})
        self.evidence_file = self.root / "evidence.json"
        self.evidence = {"role": "review_code", "evidence": {"steps": [{"id": "a", "status": "implemented"}], "findings": []}}
        self.evidence_file.write_text(json.dumps(self.evidence))

    def write_assignment(self, target: dict | None = None):
        files = {
            "target.json": json.dumps(target or self.target, sort_keys=True, separators=(",", ":")).encode(),
            "snapshot.json": b'{"phase":"implementing"}',
            "packet.json": b'{"requirements":{}}',
            "evidence-template.json": b'{"role":"review_code","evidence":{}}\n',
            "evidence-schema.json": b"{}\n",
            "feature.diff": bytes([0xFF, 0x00, 0x0A]),
        }
        keys = {"target.json": "target", "snapshot.json": "snapshot", "packet.json": "packet",
                "evidence-template.json": "template", "evidence-schema.json": "schema", "feature.diff": "featureDiff"}
        manifest = {"version": 1, "directory": str(self.task_dir), "files": {}}
        for name, data in files.items():
            path = self.task_dir / name
            if path.exists():
                path.chmod(0o600)
            path.write_bytes(data)
            path.chmod(0o400)
            manifest["files"][keys[name]] = {"path": str(path), "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
        manifest_path = self.task_dir / "manifest.json"
        if manifest_path.exists():
            manifest_path.chmod(0o600)
        manifest_path.write_text(json.dumps(manifest, sort_keys=True, separators=(",", ":")))

    def set_state(self, **values):
        (self.root / "fake-state.json").write_text(json.dumps(values))

    def state(self):
        return json.loads((self.root / "fake-state.json").read_text())

    def calls(self):
        log = self.root / "calls.jsonl"
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    def helper(self, *args, stdin=None, task_dir=None, digest=DIGEST):
        env = {
            **os.environ,
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "FAKE_SWAMP_ROOT": str(self.root),
            "SWAMP_SERVE_URL": "http://remote.invalid",
            "SWAMP_SERVER_URL": "http://remote.invalid",
            "SWAMP_REPO_DIR": "/elsewhere",
            "SWAMP_WORKFLOWS_DIR": "/elsewhere/workflows",
            "SWAMP_LOCK_TIMEOUT_MS": "3000",
        }
        command = [sys.executable, "-I", str(HELPER), "--task-dir", str(task_dir or self.task_dir), "--assignment-digest", digest, *args]
        result = subprocess.run(command, input=stdin, text=True, capture_output=True, env=env, timeout=60)
        lines = result.stdout.strip().splitlines()
        return result, (json.loads(lines[-1]) if lines and lines[-1].startswith("{") else None)

    def test_builds_envelope_and_sanitizes_environment(self):
        result, output = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual(output, {"status": "accepted", "taskId": TASK, "role": "review_code", "assignmentDigest": DIGEST,
                                  "payloadDigest": "e" * 64, "acceptedAt": "2026-11-14T00:00:00Z"})
        submit, query = self.calls()
        self.assertEqual(submit["argv"], ["behavior", "run", f"issue-evidence-{TASK}", "submit", "--repo-dir", str(self.control),
                                          "--stdin", "--literal-inputs", "--json"])
        self.assertEqual(json.loads(submit["stdin"]), {"taskId": TASK, "assignmentDigest": DIGEST, "role": "review_code",
                                                       "evidence": self.evidence["evidence"]})
        self.assertEqual(query["argv"], ["data", "query", f'behavior("issue-evidence-{TASK}") && name == "submission"',
                                         "--repo-dir", str(self.control), "--select", "content", "--single", "--json"])
        for call in (submit, query):
            self.assertEqual(call["env"], {"SWAMP_LOCK_TIMEOUT_MS": "3000"})
        # Stdin input is equivalent.
        self.set_state(slot={"status": "open"})
        result, output = self.helper("-", stdin=json.dumps(self.evidence))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output["status"], "accepted")

    def test_digest_mismatch_sends_nothing(self):
        result, output = self.helper(str(self.evidence_file), digest="c" * 64)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(output["status"], "refused")
        self.assertIn("--assignment-digest", output["error"])
        result, _ = self.helper(str(self.evidence_file), digest="not-a-digest")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.calls(), [])

    def test_role_or_foreign_task_fields_rejected(self):
        cases = [
            {**self.evidence, "role": "review_ux"},
            {**self.evidence, "taskId": "a6da10b0-c980-4b82-b920-000000000002"},
            {**self.evidence, "assignmentDigest": "c" * 64},
            {**self.evidence, "behaviorName": "issue-evidence-other"},
            {**self.evidence, "controlDir": "/elsewhere"},
            {"role": "review_code", "evidence": []},
            [self.evidence],
        ]
        for evidence in cases:
            with self.subTest(evidence=evidence):
                self.evidence_file.write_text(json.dumps(evidence))
                result, output = self.helper(str(self.evidence_file))
                self.assertEqual(result.returncode, 2, result.stdout)
                self.assertEqual(output["status"], "refused")
        # Matching optional identity fields are accepted.
        self.evidence_file.write_text(json.dumps({**self.evidence, "taskId": TASK, "assignmentDigest": DIGEST}))
        self.assertEqual(self.calls(), [])
        result, _ = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_tampered_assignment_file_sends_nothing(self):
        packet = self.task_dir / "packet.json"
        packet.chmod(0o600)
        packet.write_bytes(b'{"requirements":[]}')
        result, output = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 2)
        self.assertIn("packet.json differs", output["error"])
        packet.unlink()
        result, output = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 2)
        self.assertIn("unreadable", output["error"])
        self.assertEqual(self.calls(), [])

    def test_task_dir_and_behavior_name_must_match_task(self):
        other = self.task_dir.parent / "a6da10b0-c980-4b82-b920-000000000009"
        other.symlink_to(self.task_dir)
        # A symlink resolves to the real task directory, which matches.
        result, _ = self.helper("--status", task_dir=other)
        self.assertEqual(result.returncode, 3, result.stdout)
        copy = self.root / "copy" / "a6da10b0-c980-4b82-b920-000000000009"
        copy.mkdir(parents=True)
        for path in self.task_dir.iterdir():
            (copy / path.name).write_bytes(path.read_bytes())
        result, output = self.helper(str(self.evidence_file), task_dir=copy)
        self.assertEqual(result.returncode, 2)
        self.assertIn("does not belong", output["error"])
        for target in ({**self.target, "behaviorName": "issue-evidence-a6da10b0-c980-4b82-b920-000000000002"},
                       {**self.target, "extra": True}, {**self.target, "protocol": 2}, {**self.target, "controlDir": "relative"}):
            with self.subTest(target=target):
                self.write_assignment(target)
                result, output = self.helper(str(self.evidence_file))
                self.assertEqual(result.returncode, 2, result.stdout)
        result, _ = self.helper(str(self.evidence_file), task_dir=self.root / "missing")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(len(self.calls()), 1)  # only the --status query above

    def test_validation_error_exits_1_with_message(self):
        self.set_state(slot={"status": "open"}, submit="reject")
        result, output = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(output, {"status": "rejected", "submission": "open",
                                  "error": "evidence.summary: Too small: expected string to have >=1 characters"})
        self.assertIn("Too small", result.stderr)

    def test_conflict_is_not_reported_as_accepted(self):
        self.set_state(slot={"status": "accepted", "evidence": {"findings": ["earlier"]}, "receipt": {
            "taskId": TASK, "assignmentDigest": DIGEST, "payloadDigest": "f" * 64, "acceptedAt": "2026-11-13T00:00:00Z"}})
        result, output = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(output["status"], "rejected")
        self.assertEqual(output["submission"], "accepted")
        self.assertIn("Changed evidence after acceptance", output["error"])

    def test_lost_ack_identical_rerun_reports_original_receipt(self):
        self.set_state(slot={"status": "open"}, submit="lost_ack")
        result, output = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(output["status"], "rejected")
        self.assertIn("connection reset", output["error"])
        result, output = self.helper("--status")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(output["acceptedAt"], "2026-11-14T00:00:00Z")
        result, output = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(output["acceptedAt"], "2026-11-14T00:00:00Z")
        self.assertEqual(self.state()["submits"], 2)
        # Acceptance that cannot be confirmed is not reported as accepted.
        self.set_state(**{**self.state(), "query": "fail"})
        result, output = self.helper(str(self.evidence_file))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(output["submission"], "unknown")
        self.assertIn("--status", output["error"])

    def test_status_open_accepted_superseded(self):
        result, output = self.helper("--status")
        self.assertEqual((result.returncode, output["status"]), (3, "open"))
        self.helper(str(self.evidence_file))
        result, output = self.helper("--status")
        self.assertEqual((result.returncode, output["status"]), (0, "accepted"))
        self.set_state(slot={"status": "superseded"})
        result, output = self.helper("--status")
        self.assertEqual((result.returncode, output["status"]), (1, "superseded"))
        # A receipt for a different assignment is never this task's acceptance.
        self.set_state(slot={"status": "accepted", "receipt": {"taskId": TASK, "assignmentDigest": "c" * 64}})
        result, output = self.helper("--status")
        self.assertEqual((result.returncode, output["status"]), (1, "error"))
        self.assertTrue(all(call["argv"][:2] == ["data", "query"] for call in self.calls() if "--stdin" not in call["argv"]))

    def test_print_commands_sends_nothing(self):
        result, _ = self.helper("--print-commands")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls(), [])
        self.assertIn("env -u SWAMP_SERVE_URL -u SWAMP_SERVER_URL -u SWAMP_REPO_DIR -u SWAMP_WORKFLOWS_DIR swamp behavior run "
                      f"issue-evidence-{TASK} submit --repo-dir {self.control}", result.stdout)
        self.assertIn("data query", result.stdout)
        self.assertIn(str(self.task_dir / "evidence-template.json"), result.stdout)


if __name__ == "__main__":
    unittest.main()
