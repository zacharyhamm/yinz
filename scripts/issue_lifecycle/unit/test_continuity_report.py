"""The report consumes exports without touching lifecycle state or executing checks."""
import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from continuity_report import compare, issue_metrics, normalize


def issue(key="issue-yinz-1", modern=False):
    task = {"id": "task-1", "role": "review_code", "createdAt": "2026-11-01T01:00:00Z", "issueDigest": "a"}
    if modern:
        task["contractVersion"] = 2
    return {"key": key, "issueDigest": "a", "completedAt": "2026-11-01T02:00:00Z", "assignments": [task, copy.deepcopy(task)], "requirementsDigests": ["a"], "rounds": [], "repairDecisions": [], "observations": []}


def repair_review(task_id, outcome, completed=None, commit="same", role="review_code", minute=0):
    task = {
        "id": task_id, "role": role,
        "createdAt": f"2026-11-01T01:{minute:02}:00Z",
        "evidence": {"payload": {"evidence": {
            "commit": commit,
            "priorFindings": [{"finding": {"role": role, "findingId": "f"}, "outcome": outcome}],
        }}},
    }
    if completed is not None:
        task["completedRepairDecisionIds"] = completed
    return task


def repair_issue():
    row = issue(modern=True)
    row["rounds"] = [{"assignmentId": "original", "launchCauses": [{"kind": "initial_work"}]}]
    row["repairDecisions"] = [{
        "id": "fix", "repair": [{"role": "review_code", "findingId": "f"}],
        "result": {"commit": "same"},
    }]
    return row


class ContinuityReportTests(unittest.TestCase):
    def test_deduplicates_assignments_and_keeps_unknown_quality_unknown(self):
        result = issue_metrics(issue())
        self.assertEqual(result["rounds"], 1)
        self.assertEqual(result["unknownCauseRounds"], 1)
        self.assertEqual(result["elapsedMs"], 3600000)
        for key in ("unsupportedReopening", "postCompletionRegressions", "laterGateDefects", "firstRepairClosure"):
            self.assertIsNone(result[key])

    def test_pilot_selects_next_twenty_new_contract_completions_and_stratifies(self):
        baseline = {"schemaVersion": 1, "frozenAt": "2026-11-02T00:00:00Z", "issues": [issue()]}
        pilot = copy.deepcopy(baseline)
        pilot["issues"] = []
        for index in range(22):
            row = issue(f"issue-yinz-{index + 2}", True)
            row["completedAt"] = f"2026-11-03T{index:02}:00:00Z"
            if index % 2:
                row["requirementsDigests"].append("b")
                row["recoveries"] = ["recorded-recovery"]
            pilot["issues"].append(row)
        report = compare(baseline, pilot)
        self.assertEqual(report["pilot"]["summary"]["issues"], 20)
        self.assertEqual(len(report["pilot"]["strata"]), 2)
        self.assertIn("No improvement claim", report["assessment"])
        self.assertTrue(report["timingLimitations"])
        baseline["issues"][0]["assignments"][0]["contractVersion"] = 2
        with self.assertRaisesRegex(ValueError, "pre-change"):
            compare(baseline, pilot)

    def test_normalization_refuses_partial_export_and_uses_latest_task_projection(self):
        with self.assertRaisesRegex(ValueError, "incomplete"):
            normalize({"limited": True, "results": []})
        task = {"id": "task", "role": "implement", "createdAt": "2026-11-01T01:00:00Z", "before": {"issue": {"digest": "a"}}}
        state = {"phase": "done", "issue": {"digest": "a"}, "receipt": {"mergedCommit": "full-object"}}
        records = [{"name": "agent-task", "specName": "agent-task", "modelName": "issue", "version": 1, "createdAt": task["createdAt"], "content": task}, {"name": "agent-task", "specName": "agent-task", "modelName": "issue", "version": 2, "createdAt": "2026-11-01T01:30:00Z", "content": task}, {"name": "state", "specName": "state", "modelName": "issue", "version": 1, "createdAt": "2026-11-01T02:00:00Z", "content": state}]
        result = normalize({"limited": False, "results": records})
        self.assertEqual(len(result["issues"][0]["assignments"]), 1)
        self.assertEqual(len(result["issues"][0]["sourceRefs"]), 3)

    def test_reserved_assignment_carries_original_round_before_journal_projection(self):
        task = {"id": "reserved", "role": "plan", "createdAt": "2026-11-01T01:00:00Z", "contractVersion": 2,
                "before": {"issue": {"digest": "a"}}, "round": {"assignmentId": "reserved", "launchCauses": [{"kind": "unknown"}], "corrections": []}}
        records = [{"modelName": "issue", "name": "driver", "specName": "driver", "version": 1, "createdAt": task["createdAt"], "content": {"automaticRevision": {"assignment": task}}},
                   {"modelName": "issue", "name": "state", "specName": "state", "version": 1, "createdAt": "2026-11-01T02:00:00Z", "content": {"phase": "done", "issue": {"digest": "a"}}}]
        result = normalize({"limited": False, "results": records})
        self.assertEqual(len(result["issues"][0]["rounds"]), 1)
        self.assertEqual(issue_metrics(result["issues"][0])["rounds"], 1)

    def test_explicit_first_repair_outcomes_and_quality_observations(self):
        row = issue(modern=True)
        row["assignments"] = row["assignments"][:1]
        row["rounds"] = [{"assignmentId": "task-1", "launchCauses": [{"kind": "unknown"}], "corrections": [{"source": "finding", "cause": {"kind": "incomplete_repair"}}]}]
        row["repairDecisions"] = [{"id": "fix", "repair": [{"role": "review_code", "findingId": "f"}], "result": {"commit": "new"}}]
        row["assignments"][0]["evidence"] = {"payload": {"evidence": {"commit": "new", "priorFindings": [{"finding": {"role": "review_code", "findingId": "f"}, "outcome": "persists"}]}}}
        row["assignments"][0]["completedRepairDecisionIds"] = ["fix"]
        row["observations"] = [{"kind": "unsupported_reopening", "count": 0, "source": "assessor", "evidence": ["review-log"]}]
        result = issue_metrics(row)
        self.assertFalse(result["firstRepairClosure"])
        self.assertEqual(result["incompleteRepairEvidence"], 1)
        self.assertEqual(result["unsupportedReopening"], 0)
        del row["observations"][0]["evidence"]
        with self.assertRaisesRegex(ValueError, "attribution"):
            issue_metrics(row)

    def test_unknown_causes_require_explicit_indexed_replacements(self):
        cases = [
            (["unknown"], [], 1),
            (["unknown"], [(None, "incomplete_repair")], 1),
            (["unknown"], [(0, "incomplete_repair")], 0),
            (["unknown", "recovery"], [(1, "incomplete_repair")], 1),
            (["unknown", "unknown"], [(0, "incomplete_repair")], 1),
            (["unknown", "unknown"], [(0, "recovery"), (1, "incomplete_repair")], 0),
            (["unknown"], [(0, "recovery"), (0, "unknown")], 1),
            (["unknown"], [(0, "unknown"), (0, "recovery")], 0),
            (["unknown"], [(0, "recovery"), (None, "unknown")], 0),
            (["unknown"], [(1, "recovery")], 1),
        ]
        for causes, corrections, expected in cases:
            with self.subTest(causes=causes, corrections=corrections):
                row = issue()
                row["rounds"] = [{
                    "assignmentId": "task-1",
                    "launchCauses": [{"kind": kind} for kind in causes],
                    "corrections": [
                        {"cause": {"kind": kind}, "source": "finding"}
                        | ({"replaces": index} if index is not None else {})
                        for index, kind in corrections
                    ],
                }]
                before = copy.deepcopy(row)
                self.assertEqual(issue_metrics(row)["unknownCauseRounds"], expected)
                self.assertEqual(row, before)

    def test_unchanged_commit_requires_completed_repair_in_review_snapshot(self):
        for fresh_outcome in ("persists", "verified_fixed"):
            with self.subTest(fresh_outcome=fresh_outcome):
                original_outcome = "verified_fixed" if fresh_outcome == "persists" else "persists"
                row = repair_issue()
                row["assignments"] = [
                    repair_review("original", original_outcome, []),
                    repair_review("fresh", fresh_outcome, ["fix"], minute=10),
                ]
                result = issue_metrics(row)
                self.assertEqual(result["firstRepairClosure"], fresh_outcome == "verified_fixed")
                self.assertEqual(result["incompleteRepairs"], int(fresh_outcome == "persists"))

    def test_first_eligible_review_per_role_survives_later_repairs_at_same_commit(self):
        for first_outcome in ("persists", "verified_fixed"):
            with self.subTest(first_outcome=first_outcome):
                later_outcome = "verified_fixed" if first_outcome == "persists" else "persists"
                row = repair_issue()
                later = copy.deepcopy(row["repairDecisions"][0])
                later["id"] = "later-fix"
                row["repairDecisions"].append(later)
                row["assignments"] = [
                    repair_review("later", later_outcome, ["fix", "later-fix"], minute=20),
                    repair_review("original", later_outcome, []),
                    repair_review("first-fresh", first_outcome, ["fix"], minute=10),
                ]
                result = issue_metrics(row)
                self.assertEqual(result["firstRepairClosure"], first_outcome == "verified_fixed")
                self.assertEqual(result["incompleteRepairs"], 1)

    def test_repair_closure_needs_first_fresh_review_for_each_selected_role(self):
        row = repair_issue()
        row["repairDecisions"][0]["repair"].append({"role": "review_ux", "findingId": "f"})
        row["assignments"] = [
            repair_review("code", "verified_fixed", ["fix"], minute=10),
            repair_review("original-ux", "persists", [], role="review_ux"),
        ]
        self.assertIsNone(issue_metrics(row)["firstRepairClosure"])
        row["assignments"].append(repair_review("ux", "verified_fixed", ["fix"], role="review_ux", minute=20))
        self.assertTrue(issue_metrics(row)["firstRepairClosure"])

    def test_missing_correlation_wrong_commit_and_missing_first_outcome_remain_unknown(self):
        for task in (
            repair_review("uncorrelated", "persists"),
            repair_review("not-completed", "persists", []),
            repair_review("other-repair", "persists", ["other-fix"]),
            repair_review("wrong-commit", "persists", ["fix"], commit="other"),
        ):
            with self.subTest(task=task["id"]):
                row = repair_issue()
                row["assignments"] = [task]
                result = issue_metrics(row)
                self.assertIsNone(result["firstRepairClosure"])
                self.assertEqual(result["incompleteRepairs"], 0)
        row = repair_issue()
        first = repair_review("first", "verified_fixed", ["fix"])
        del first["evidence"]["payload"]["evidence"]["priorFindings"]
        row["assignments"] = [first, repair_review("later", "verified_fixed", ["fix"], minute=10)]
        self.assertIsNone(issue_metrics(row)["firstRepairClosure"])

    def test_normalization_retains_snapshot_repair_correlation_in_schema_one(self):
        row = repair_issue()
        completed = row["repairDecisions"][0]
        pending = {"id": "pending", "repair": [{"role": "review_code", "findingId": "f"}]}
        tasks = [
            repair_review("original", "persists"),
            repair_review("fresh", "verified_fixed", minute=10),
        ]
        records = []
        for index, task in enumerate(tasks):
            task["before"] = {"issue": {"digest": "a"}, "reviewDecisions": [pending] + ([completed] if index else [])}
            task["evidence"].update({"acceptedAt": task["createdAt"], "payloadDigest": "digest"})
            task["evidence"]["payload"]["role"] = task["role"]
            records.append({"modelName": "issue", "name": "agent-task", "specName": "agent-task",
                            "version": index, "createdAt": task["createdAt"], "content": task})
        records.append({"modelName": "issue", "name": "state", "specName": "state", "version": 1,
                        "createdAt": row["completedAt"], "content": {
                            "phase": "done", "issue": {"digest": "a"}, "reviewDecisions": [completed, pending],
                        }})
        export = {"results": records}
        before = copy.deepcopy(export)
        normalized = normalize(export)
        self.assertEqual(normalized["schemaVersion"], 1)
        assignments = normalized["issues"][0]["assignments"]
        self.assertEqual([t["completedRepairDecisionIds"] for t in assignments], [[], ["fix"]])
        self.assertEqual(export, before)
        self.assertTrue(issue_metrics(normalized["issues"][0])["firstRepairClosure"])

    def test_older_normalized_exports_are_readable_with_unknown_repair_closure(self):
        row = repair_issue()
        row["assignments"] = [repair_review("legacy", "verified_fixed")]
        baseline = {"schemaVersion": 1, "frozenAt": "2026-11-02T00:00:00Z", "issues": [row]}
        report = compare(baseline, {"schemaVersion": 1, "issues": []})
        self.assertIsNone(report["baseline"]["issues"][0]["firstRepairClosure"])
        self.assertEqual(report["baseline"]["summary"]["firstRepairClosure"], {"closed": 0, "assessable": 0})


if __name__ == "__main__":
    unittest.main()
