"""Consistency diagnostics must not infer termination from time or evidence."""
import copy
import json
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from issue_lifecycle.observations import (
    PENDING_ROLLBACK_METHOD, USER_REQUEST, compact_submission, completed_worker_conditions, diagnose,
    rejected_launch_conditions, state_digest, task_recovery, terminal_failure_conditions,
)

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "discard_cases.json"
# Shared with extensions/behaviors/unit/worker_recovery_test.ts: each child's drive
# classification (classifyWorkerFailure) and status advice (task_recovery) must agree.
RECOVERY_FIXTURE = Path(__file__).resolve().parent / "fixtures" / "recovery_cases.json"


class ObservationTests(unittest.TestCase):
    def test_completion_background_and_identity(self):
        task = {"id": "task", "behaviorName": "worker", "status": "running",
                "config": {"session": "original"}, "promptDigest": "digest",
                "submission": {"assignmentDigest": "assignment"}}
        child = {"config": task["config"], "promptDigest": "digest", "phase": "waiting",
                 "submissionId": "submission", "conversationId": "conversation"}
        receipt = {"status": "accepted", "assignment": {"taskId": "task", "digest": "assignment"}}
        for phase, outcome, background, expected in (
            ("waiting", "timed_out", [], "evidence_waiting_for_completion"),
            ("completed", "completed", [], "completion_unreconciled"),
            ("completed", "completed", ["job"], "background_work"),
        ):
            child.update(phase=phase, outcome=outcome, backgroundTasks=background)
            before = copy.deepcopy((task, child, receipt))
            for _ in range(2):
                result = diagnose("issue", {}, task, child, receipt, [])
                self.assertEqual(result[0]["code"], expected)
                self.assertEqual(result[0]["identity"]["submissionId"], "submission")
            self.assertEqual((task, child, receipt), before)
        self.assertIn("accept-submission with taskId task", diagnose("issue", {}, task, child, receipt, [])[0]["action"])
        self.assertNotIn("accept-submission", diagnose("issue", {}, task, child, None, [])[0]["action"])
        # A timed-out wait reports background work only on the agent row.
        response = {"outcome": "timed_out", "agents": [
            {"pane_id": "terminal_1", "background": True},
            {"pane_id": "terminal_2", "background_tasks": ["other"]}]}
        waiting = dict(child, phase="waiting", outcome="timed_out", backgroundTasks=[],
                       paneId="terminal_1", rawResponse=json.dumps(response))
        self.assertEqual(diagnose("issue", {}, task, waiting, receipt, [])[0]["code"], "background_work")
        self.assertIn("accept-submission", diagnose("issue", {}, task, waiting, receipt, [])[0]["action"])
        waiting["paneId"] = "terminal_3"
        self.assertEqual(diagnose("issue", {}, task, waiting, receipt, [])[0]["code"],
                         "evidence_waiting_for_completion")
        waiting["rawResponse"] = "not json"
        self.assertEqual(diagnose("issue", {}, task, waiting, receipt, [])[0]["code"],
                         "evidence_waiting_for_completion")
        receipt["assignment"]["digest"] = "stale"
        self.assertEqual(diagnose("issue", {}, task, child, receipt, [])[0]["code"], "evidence_identity_mismatch")
        child["promptDigest"] = "replacement"
        self.assertEqual(diagnose("issue", {}, task, child, receipt, [])[0]["code"], "worker_identity_mismatch")
        self.assertEqual(diagnose("issue", {}, task, None, receipt, [])[0]["code"], "child_state_missing")

    def test_only_correlated_running_workflow_after_done_is_inconsistent(self):
        run = {"runId": "exact-run", "status": "running", "startedAt": "2000-01-01",
               "inputs": {"issueName": "issue"}}
        self.assertEqual(diagnose("issue", {"phase": "implementing"}, None, None, None, [run]), [])
        result = diagnose("issue", {"phase": "done"}, None, None, None, [run])
        self.assertEqual(result[0]["runId"], "exact-run")
        self.assertIn("liveness is unknown", result[0]["message"])
        run["inputs"]["issueName"] = "other"
        self.assertEqual(diagnose("issue", {"phase": "done"}, None, None, None, [run]), [])


def fixture_cases():
    return {case["name"]: case for case in json.loads(FIXTURE.read_text())}


def fixture_child(name, **changes):
    child = copy.deepcopy(fixture_cases()[name]["child"])
    child.update(changes)
    return child


def response(child, **changes):
    value = json.loads(child["rawResponse"])
    value.update(changes)
    return json.dumps(value)


class TaskRecoveryTests(unittest.TestCase):
    STATE = {"issue": {"itemId": 85}, "phase": "triaging"}

    def task(self, child, **changes):
        task = {"id": "6f1e0c8a-2b1f-4d43-9a8e-1c3b5d7f9a0b", "behaviorName": "issue-85-triage-worker",
                "role": "triage", "status": "running", "config": copy.deepcopy(child["config"]),
                "promptDigest": child["promptDigest"], "before": copy.deepcopy(self.STATE),
                "submission": {"behaviorName": "issue-85-evidence", "assignmentDigest": "assignment"}}
        task.update(changes)
        return task

    def recover(self, child, task=None, submission=None, rollback=None, diagnostics=(), state=None):
        task = task or self.task(child)
        return task_recovery(state or copy.deepcopy(self.STATE), task, child, submission, rollback, list(diagnostics))

    def assert_request(self, entry):
        self.assertTrue(entry["requiresUserRequest"])
        self.assertEqual(entry["input"]["request"], USER_REQUEST)

    def test_state_digest_matches_the_controller_canonical_digest(self):
        # Computed by extensions/behaviors/_lib/lifecycle.ts stateDigest for this state.
        state = {"issue": {"workspace": "default", "itemId": 7, "text": 'Café ✓ "quoted" \\ slash',
                           "note": "line\nbreak\ttab\u0001 </tag> \U0001F600", "digest": "d" * 64},
                 "phase": "triaging", "planVersion": 0}
        self.assertEqual(state_digest(state), "f24aaafd048940447e6cce8af2d7934b809f533c87ca1513540436515c03dc23")
        # JSON.stringify escapes lone surrogates, including in keys.
        lone = {"b": "x\ud800y\udfffz", "a": "\U0001F600 é\u0001", "\ud83dk": 1}
        self.assertEqual(state_digest(lone), "42c1f660d0ffd966500992d320ec9d455b8ebb99431c4038dfd627bbb1435470")

    def test_status_reports_submission_states_by_digest(self):
        submission = {"status": "accepted",
                      "assignment": {"taskId": "task", "digest": "assignment", "before": self.STATE},
                      "receipt": {"taskId": "task", "payload": {"role": "triage"}, "next": self.STATE}}
        original = copy.deepcopy(submission)
        compact = compact_submission(submission)
        self.assertEqual(submission, original)
        self.assertEqual(compact["assignment"], {"taskId": "task", "digest": "assignment",
                                                 "beforeDigest": state_digest(self.STATE)})
        self.assertEqual(compact["receipt"], {"taskId": "task", "payload": {"role": "triage"},
                                              "nextDigest": state_digest(self.STATE)})
        self.assertIsNone(compact_submission(None))

    def test_compact_task_compares_lifecycle_by_digest(self):
        child = fixture_child("v2 rejected")
        task = self.task(child)
        del task["before"]
        task["beforeDigest"] = state_digest(self.STATE)
        result = self.recover(child, task)
        self.assertTrue(result["discard"]["lifecycleUnchanged"])
        changed = {**self.STATE, "phase": "classified"}
        self.assertFalse(self.recover(child, task, state=changed)["discard"]["lifecycleUnchanged"])

    def test_no_or_resolved_task_needs_no_recovery(self):
        self.assertEqual(task_recovery(self.STATE, None, None, None, None, [])["kind"], "none")
        child = fixture_child("completed")
        for changes in ({"status": "discarded"}, {"status": "superseded"}, {"status": "completed"},
                        {"status": "completed", "cleanup": "done"}):
            result = self.recover(child, self.task(child, **changes))
            self.assertEqual(result["kind"], "none", changes)
            self.assertIsNone(result["command"])
            self.assertIn("follow the current handoff", result["summary"])
        self.assertEqual(result["advisory"],
                         "Computed from durable records; the named method re-verifies every condition.")

    def test_pending_rollback_routes_to_rollback_completion(self):
        self.assertEqual(PENDING_ROLLBACK_METHOD, "drive")
        child = fixture_child("completed")
        rollback = {"id": "recovery-1", "step": "plan", "request": "Replan", "status": "pending",
                    "before": self.STATE, "next": self.STATE}
        result = self.recover(child, rollback=rollback)
        self.assertEqual(result["kind"], "finish_rollback")
        self.assertEqual(result["command"], {"method": "drive", "input": {}})
        self.assertFalse(result["requiresUserRequest"])
        self.assertIn("plan", result["summary"])
        self.assertEqual(result["alternatives"][0]["method"], "resume-rollback")
        self.assert_request(result["alternatives"][1])
        # Even without a task, the saved intent comes first; a completed rollback does not.
        self.assertEqual(task_recovery(self.STATE, None, None, None, rollback, [])["kind"], "finish_rollback")
        rollback["status"] = "completed"
        self.assertNotEqual(self.recover(child, rollback=rollback)["kind"], "finish_rollback")

    def test_pending_discard_and_cleanup_route_to_drive(self):
        child = fixture_child("v2 rejected")
        task = self.task(child, discard={"request": "Discard it", "basis": "rejected_launch"},
                         lastError="lost response")
        result = self.recover(child, task)
        self.assertEqual((result["kind"], result["command"]["method"]), ("finish_discard", "drive"))
        self.assertEqual(result["lastError"], "lost response")
        # Cleanup refused by a transient guard on the same worker: drive retries within a bound.
        thrown = "Worker cleanup blocked: rejected: worker is Busy, not idle"
        child = fixture_child("completed", cleanupRefusal={
            "outcome": "rejected", "message": "worker is Busy, not idle", "thrown": thrown, "retryable": True,
            "submissionId": "submission-1", "conversationId": "conversation-1",
            "observedAt": "2026-11-03T12:00:00.000Z"})
        task = self.task(child, status="completed", cleanup="pending", lastError=thrown)
        result = self.recover(child, task)
        self.assertEqual((result["kind"], result["command"]["method"]), ("retry_cleanup", "drive"))
        self.assertEqual(result["lastError"], thrown)
        self.assertIn("rejected guard", result["summary"])
        self.assertIn("retries this automatically within a bound", result["summary"])
        self.assertIn("rerun drive", result["summary"])
        self.assertIn("unconsumed_input", result["summary"])
        self.assertEqual((result["cleanupRefusal"]["retryable"], result["cleanupRefusal"]["thrown"],
                          result["cleanupRefusal"]["matchesWorker"]), (True, thrown, True))
        # A refusal for another submission, or a non-retryable one, needs inspection.
        for refusal in ({"submissionId": "submission-0"}, {"retryable": False}):
            stale = copy.deepcopy(child)
            stale["cleanupRefusal"].update(refusal)
            result = self.recover(stale, task)
            self.assertEqual(result["kind"], "retry_cleanup")
            self.assertIn("not a transient guard", result["summary"])
            self.assertNotIn("within a bound", result["summary"])

    def test_identity_mismatch_requires_inspection_without_command(self):
        child = fixture_child("completed")
        for code in ("child_state_missing", "worker_identity_mismatch", "evidence_identity_mismatch"):
            result = self.recover(child, diagnostics=[{"code": code}])
            self.assertEqual(result["kind"], "inspect")
            self.assertIsNone(result["command"])
            self.assertIn(code, result["summary"])
            restart, rollback = result["alternatives"]
            self.assertEqual((restart["method"], rollback["method"]), ("restart-agent", "rollback"))
            self.assert_request(restart)
            self.assert_request(rollback)
            self.assertEqual({key: restart["input"][key] for key in ("taskId", "role", "session", "kind")},
                             {"taskId": result["taskId"], "role": "triage", "session": "dev", "kind": "claude"})
            self.assertNotIn("control", restart["input"])
        self.assertEqual(self.recover(None, self.task(child))["kind"], "inspect")

    def busy_timeout(self, phase="waiting", **changes):
        child = fixture_child("acknowledged timed_out", phase=phase)
        value = json.loads(child["rawResponse"])
        value["agents"][0]["agent"]["state"] = "busy"
        value["agents"][0]["ready"] = False
        value["agents"][0]["not_ready_reason"] = "unconsumed_input"
        value.update(changes)
        child["rawResponse"] = json.dumps(value)
        return child

    def test_timeout_background_and_accepted_completion_resume_with_drive(self):
        accepted = {"status": "accepted", "assignment": {"taskId": "task", "digest": "assignment"}}
        cases = (
            (fixture_child("completed"), accepted, "completion unreconciled"),
            (fixture_child("completed with background"), None, "background work"),
            (fixture_child("completed", phase="background"), None, "background work"),
            (self.busy_timeout(), None, "ordinary waiting wait timed out"),
            (self.busy_timeout("background", submission_id=None), None, "ordinary background wait timed out"),
            (self.busy_timeout("spawned"), None, "ordinary spawned wait timed out"),
            (fixture_child("acknowledged failed", outcome="transport_error", failureKind="other",
                           lastError="transport_error"), None, "transport error"),
        )
        for child, submission, phrase in cases:
            result = self.recover(child, submission=submission)
            self.assertEqual((result["kind"], result["command"]), ("resume", {"method": "drive", "input": {}}), phrase)
            self.assertFalse(result["requiresUserRequest"])
            self.assertIn(phrase, result["summary"])
            self.assertIn("within a bound", result["summary"])
            self.assertIn("budget exhausted, rerun drive", result["summary"])
        preflight = fixture_child("legacy structured rejection", phase="preflight", outcome="preflight_transport",
                                  lastError="Yinz is unreachable")
        preflight["config"]["control"] = {"session": "dev", "paneId": "terminal_1"}
        result = self.recover(preflight, self.task(preflight))
        self.assertEqual((result["kind"], result["command"]["method"]), ("resume", "drive"))
        self.assertIn("never spawns after a launch intent", result["summary"])
        # Without control identity, or after a launch intent, preflight needs a human.
        for changes in ({"control": None}, {"launchId": "launch-1"}):
            child = copy.deepcopy(preflight)
            if "control" in changes:
                del child["config"]["control"]
            else:
                child.update(changes)
            self.assertEqual(self.recover(child, self.task(child))["kind"], "inspect", changes)
        # An idle (uncorrelated) or not_sent timeout is never an automatic re-observation.
        self.assertEqual(self.recover(fixture_child("acknowledged timed_out"))["kind"], "inspect")
        self.assertEqual(self.recover(fixture_child("not_sent timed_out"))["kind"], "discard")
        submitting = self.busy_timeout("submitting", prompt_delivery="started")
        self.assertEqual(self.recover(submitting)["kind"], "resume")
        self.assertEqual(self.recover(self.busy_timeout("submitting", acknowledged=False))["kind"], "inspect")

    def test_rejected_launch_v2_and_legacy_are_discard_eligible_with_request(self):
        for name, basis in (("v2 rejected", "rejected_launch"), ("legacy structured rejection", "rejected_launch")):
            result = self.recover(fixture_child(name))
            self.assertEqual(result["kind"], "discard", name)
            self.assertEqual(result["command"], {"method": "discard-agent", "input": {"request": USER_REQUEST}})
            self.assertTrue(result["requiresUserRequest"])
            self.assertEqual((result["discard"]["eligible"], result["discard"]["basis"]), (True, basis))
            self.assertIn("corrected configuration", result["summary"])
        # discard-agent refuses once lifecycle fields changed since dispatch.
        result = self.recover(fixture_child("v2 rejected"), state={**self.STATE, "phase": "classified"})
        self.assertEqual(result["kind"], "inspect")
        self.assertFalse(result["discard"]["eligible"])
        self.assertIn("resume the task before discarding", result["summary"])

    def test_resource_bearing_rejection_lists_failed_conditions(self):
        result = self.recover(fixture_child("legacy rejection with spawned pane"))
        self.assertEqual(result["kind"], "inspect")
        self.assertFalse(result["discard"]["eligible"])
        failed = [c["detail"] for c in result["discard"]["conditions"]["rejectedLaunch"] if not c["met"]]
        self.assertEqual(failed, [
            'no version-two launch receipt and phase is "launching", not launch_rejected',
            "saved rejection retains resources: spawned pane",
        ])
        running = self.recover(fixture_child("v2 running"))
        failed = [c["detail"] for c in running["discard"]["conditions"]["rejectedLaunch"] if not c["met"]]
        self.assertEqual(failed, [
            'launch receipt outcome is "started", expected rejected, cancelled or failed',
            'launch receipt process is "running", expected not_started or exited',
            'receipt disposition is "available", expected closed',
        ])
        self.assertEqual(set(running["discard"]["conditions"]), {"rejectedLaunch", "terminalFailure", "completedWorker"})

    def test_terminal_failure_acknowledged_failed_and_not_sent_are_eligible_but_acknowledged_timeout_is_not(self):
        for name in ("acknowledged failed", "not_sent timed_out"):
            result = self.recover(fixture_child(name))
            self.assertEqual(result["kind"], "discard", name)
            self.assertEqual((result["discard"]["eligible"], result["discard"]["basis"]),
                             ("unverified", "terminal_failure"))
            self.assertIn("re-waits the original submission", result["summary"])
            self.assert_request({**result["command"], "requiresUserRequest": result["requiresUserRequest"]})
        result = self.recover(fixture_child("acknowledged timed_out"))
        self.assertEqual(result["kind"], "inspect")
        self.assertIn('latest response outcome "timed_out" is neither an acknowledged failed',
                      " ".join(c["detail"] for c in result["discard"]["conditions"]["terminalFailure"]))

    def test_completed_without_evidence_prefers_continue_then_discard(self):
        child = fixture_child("completed")
        result = self.recover(child, submission={"status": "open"})
        self.assertEqual(result["kind"], "continue_or_discard")
        self.assertEqual(result["command"], {"method": "continue-agent",
                                             "input": {"taskId": result["taskId"], "request": USER_REQUEST}})
        self.assertTrue(result["requiresUserRequest"])
        self.assertEqual((result["discard"]["eligible"], result["discard"]["basis"]), ("unverified", "completed_unusable"))
        discard = result["alternatives"][0]
        self.assertEqual(discard["method"], "discard-agent")
        self.assert_request(discard)
        self.assertIn("confirmedStopped only if it cannot observe", discard["when"])
        result = self.recover(fixture_child("completed", cleanedUp=True))
        self.assertEqual(result["kind"], "continue_or_discard")
        self.assertIn("pass confirmedStopped", result["summary"])
        self.assertEqual(result["command"]["method"], "discard-agent")
        self.assertNotIn("continue-agent", [entry["method"] for entry in result["alternatives"]])
        # An unsent continuation is delivered only by rerunning continue-agent with the user's request.
        task = self.task(child, continuation={"id": "5f1e0c8a-2b1f-4d43-9a8e-1c3b5d7f9a0b",
                                              "request": "Submit", "prompt": "Finish"})
        result = self.recover(child, task)
        self.assertEqual((result["kind"], result["command"]["method"]), ("continuation_unsent", "continue-agent"))
        self.assertTrue(result["requiresUserRequest"])

    def test_interrupted_routes_to_drive_then_restart(self):
        result = self.recover(fixture_child("interrupted"))
        self.assertEqual((result["kind"], result["command"]["method"]), ("interrupted", "drive"))
        self.assertIn("wait --submission", result["summary"])
        self.assertEqual([entry["method"] for entry in result["alternatives"]], ["restart-agent", "rollback"])
        for entry in result["alternatives"]:
            self.assert_request(entry)
        self.assertFalse(result["discard"]["eligible"])
        # The lastError alone also reports the interruption.
        child = fixture_child("interrupted", outcome="ok")
        child["rawResponse"] = response(child, outcome="ok")
        self.assertEqual(self.recover(child)["kind"], "interrupted")

    def test_needs_input_routes_to_dialog(self):
        child = fixture_child("acknowledged timed_out", outcome="needs_input", failureKind="other",
                              lastError="Agent submission has not completed: needs_input")
        child["rawResponse"] = response(child, outcome="needs_input")
        result = self.recover(child)
        self.assertEqual(result["kind"], "dialog")
        self.assertIsNone(result["command"])
        self.assertIn("deliberately in the pane", result["summary"])
        self.assertIn("nothing answers dialogs automatically", result["summary"])
        self.assertEqual(result["alternatives"][0]["method"], "drive")

    def recovery_case(self, name):
        case = next(case for case in json.loads(RECOVERY_FIXTURE.read_text()) if case["name"] == name)
        return copy.deepcopy(case["child"])

    def test_background_failure_other_than_timeout_or_transport_is_not_a_bounded_retry(self):
        # drive retries a background child only as a correlated timeout or transport error;
        # status must show the advice that applies instead of promising a retry.
        result = self.recover(self.recovery_case("background needs_input"))
        self.assertEqual((result["kind"], result["command"]), ("dialog", None))
        self.assertIn("deliberately in the pane", result["summary"])
        for name in ("background failed", "background idle timeout"):
            result = self.recover(self.recovery_case(name))
            self.assertEqual((result["kind"], result["command"]), ("inspect", None), name)
            self.assertNotIn("within a bound", result["summary"], name)
        result = self.recover(self.recovery_case("background interrupted"))
        self.assertEqual(result["kind"], "interrupted")
        # Before any failure is recorded, drive simply observes the background work.
        result = self.recover(self.recovery_case("background without a recorded failure"))
        self.assertEqual((result["kind"], result["command"]["method"]), ("resume", "drive"))
        self.assertIn("no recorded failure", result["summary"])
        self.assertIn("Any other outcome stops drive", result["summary"])

    def test_acknowledged_failed_with_background_tasks_routes_to_discard(self):
        result = self.recover(self.recovery_case("acknowledged failed with background tasks"))
        self.assertEqual((result["kind"], result["command"]["method"]), ("discard", "discard-agent"))
        self.assertEqual((result["discard"]["eligible"], result["discard"]["basis"]),
                         ("unverified", "terminal_failure"))
        self.assertTrue(result["requiresUserRequest"])

    def test_shared_recovery_fixture_matches_drive_classification(self):
        cases = json.loads(RECOVERY_FIXTURE.read_text())
        self.assertGreaterEqual(len(cases), 15)
        for case in cases:
            child, expected = case["child"], case["expected"]
            submission = {"status": "accepted"} if case["receiptAccepted"] else None
            result = self.recover(child, submission=submission)
            self.assertEqual(result["kind"], expected["status"], case["name"])
            # Status promises drive only where drive retries, or observes before any failure.
            self.assertEqual(result["kind"] == "resume", expected["drive"] != "human", case["name"])
            if expected["drive"] is None:
                self.assertFalse(child.get("lastError"), case["name"])

    def test_recovery_is_pure_and_deterministic(self):
        for name in fixture_cases():
            child = fixture_child(name)
            task = self.task(child)
            inputs = (copy.deepcopy(self.STATE), task, child, {"status": "open"},
                      {"id": "r", "step": "plan", "next": {}, "status": "completed"}, [{"code": "background_work"}])
            before = copy.deepcopy(inputs)
            first = task_recovery(*inputs)
            self.assertEqual(task_recovery(*inputs), first, name)
            self.assertEqual(inputs, before, name)
            self.assertEqual(first["discard"]["conditions"].keys(),
                             {"rejectedLaunch", "terminalFailure", "completedWorker"})

    def test_shared_discard_fixture_matches(self):
        cases = json.loads(FIXTURE.read_text())
        self.assertEqual(len(cases), 10)
        for case in cases:
            actual = {key: all(c["met"] for c in function(case["child"]))
                      for key, function in (("rejectedLaunch", rejected_launch_conditions),
                                            ("terminalFailure", terminal_failure_conditions),
                                            ("completedWorker", completed_worker_conditions))}
            self.assertEqual(actual, case["expected"], case["name"])
