"""Consistency diagnostics must not infer termination from time or evidence."""
import copy
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from issue_lifecycle.observations import diagnose


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
