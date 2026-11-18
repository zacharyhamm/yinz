"""Decision flags map to exact method inputs; chaining merges at most once."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from issue_lifecycle.decisions import (
    CHAINED, MERGE_REACHING, InputError, auto_merge_allowed, build_inputs, check_chain_flags, continue_to_gate,
    finding_ref, should_auto_merge,
)

HANDOFF = "a6da10b0-c980-4b82-b920-000000000001"


def integrate(**state):
    return {"status": "current", "decision": "integrate",
            "state": {"phase": "ready", "mergeAuthorization": {"commit": "c" * 40, "request": "Merge"}, **state}}


class BuildInputsTests(unittest.TestCase):
    def test_decision_flags_map_to_method_inputs(self):
        self.assertEqual(build_inputs("approve", {}, handoff=HANDOFF, request="Approve plan 1"),
                         {"handoffId": HANDOFF, "request": "Approve plan 1"})
        self.assertEqual(build_inputs("ready", {}, handoff=HANDOFF, request="Merge it"),
                         {"handoffId": HANDOFF, "mergeRequest": "Merge it"})
        self.assertEqual(build_inputs("closeout", {}, handoff=HANDOFF, decision="decline", request="Keep it"),
                         {"handoffId": HANDOFF, "decision": "decline", "request": "Keep it"})
        self.assertEqual(build_inputs("drive", {"agent": {"kind": "claude"}}, handoff=HANDOFF, request="Revise once"),
                         {"agent": {"kind": "claude"}, "revisionApproval": {"handoffId": HANDOFF, "request": "Revise once"}})
        self.assertEqual(build_inputs("adjudicate-plan", {"dispositions": []}, handoff=HANDOFF),
                         {"dispositions": [], "handoffId": HANDOFF})
        self.assertEqual(build_inputs("approve-review-fixes", {}, handoff=HANDOFF, request="Fix it"),
                         {"handoffId": HANDOFF, "request": "Fix it"})
        self.assertEqual(build_inputs("rollback", {"step": "plan"}, request="Replan"),
                         {"step": "plan", "request": "Replan"})
        self.assertEqual(build_inputs("decide-review", {}, handoff=HANDOFF, request="Fix one",
                                      repair=["review_code/a"], ignore=["review_ux/b c"], ignore_remaining=True),
                         {"handoffId": HANDOFF, "request": "Fix one",
                          "repair": [{"role": "review_code", "findingId": "a"}],
                          "ignore": [{"role": "review_ux", "findingId": "b c"}], "ignoreRemaining": True})

    def test_without_flags_the_input_file_passes_through(self):
        file_inputs = {"planVersion": 1, "request": "Approve"}
        self.assertEqual(build_inputs("approve", file_inputs), file_inputs)
        self.assertEqual(build_inputs("status", {}), {})

    def test_drive_requires_handoff_and_request_together(self):
        for kwargs in ({"handoff": HANDOFF}, {"request": "Revise once"}):
            with self.subTest(kwargs=kwargs), self.assertRaisesRegex(InputError, "together"):
                build_inputs("drive", {}, **kwargs)

    def test_auto_repair_flags_set_or_withdraw_the_standing_request(self):
        self.assertEqual(build_inputs("drive", {"agent": {"kind": "claude"}}, request="Fix every blocker",
                                      auto_repair=True),
                         {"agent": {"kind": "claude"}, "autoRepair": {"request": "Fix every blocker"}})
        self.assertEqual(build_inputs("drive", {}, no_auto_repair=True), {"autoRepair": {"withdraw": True}})
        for kwargs, message in (
            ({"auto_repair": True}, "takes --request"),
            ({"auto_repair": True, "request": "r", "handoff": HANDOFF}, "no --handoff"),
            ({"no_auto_repair": True, "request": "r"}, "no --handoff or --request"),
            ({"auto_repair": True, "no_auto_repair": True, "request": "r"}, "conflict"),
        ):
            with self.subTest(kwargs=kwargs), self.assertRaisesRegex(InputError, message):
                build_inputs("drive", {}, **kwargs)
        with self.assertRaisesRegex(InputError, "conflicts with autoRepair"):
            build_inputs("drive", {"autoRepair": {"request": "file"}}, request="flag", auto_repair=True)
        for command in ("decide-review", "ready", "approve"):
            with self.subTest(command=command), self.assertRaisesRegex(InputError, "apply only to drive"):
                build_inputs(command, {}, handoff=HANDOFF, request="r", auto_repair=True)

    def test_invalid_handoff_and_finding_refs_are_rejected(self):
        with self.assertRaisesRegex(InputError, "UUID"):
            build_inputs("approve", {}, handoff="not-a-uuid", request="Approve")
        for ref in ("code/a", "review_code/", "review_ux", "review_code/ a"):
            with self.subTest(ref=ref), self.assertRaises(InputError):
                build_inputs("decide-review", {}, handoff=HANDOFF, request="r", repair=[ref])
        self.assertEqual(finding_ref("review_ux/x"), {"role": "review_ux", "findingId": "x"})

    def test_blank_text_is_rejected(self):
        with self.assertRaisesRegex(InputError, "--request"):
            build_inputs("approve", {}, handoff=HANDOFF, request="  ")
        with self.assertRaisesRegex(InputError, "--skip-ci-request"):
            build_inputs("merge", {}, skip_ci_request="\n")

    def test_flags_conflicting_with_the_input_file_are_rejected(self):
        for command, file_inputs, kwargs in (
            ("approve", {"request": "file"}, {"request": "flag"}),
            ("ready", {"mergeRequest": "file"}, {"request": "flag"}),
            ("approve", {"handoffId": HANDOFF}, {"handoff": HANDOFF}),
            ("drive", {"revisionApproval": {}}, {"handoff": HANDOFF, "request": "flag"}),
            ("decide-review", {"repair": []}, {"repair": ["review_code/a"]}),
            ("decide-review", {"ignoreRemaining": False}, {"ignore_remaining": True}),
            ("closeout", {"decision": "accept"}, {"decision": "decline"}),
            ("merge", {"skipCiRequest": "file"}, {"skip_ci_request": "flag"}),
        ):
            with self.subTest(command=command, kwargs=kwargs), self.assertRaisesRegex(InputError, "conflicts"):
                build_inputs(command, file_inputs, **kwargs)

    def test_flags_on_the_wrong_command_are_rejected(self):
        for command, kwargs in (
            ("implement", {"handoff": HANDOFF}),
            ("summarize", {"request": "done"}),
            ("approve", {"repair": ["review_code/a"]}),
            ("ready", {"ignore_remaining": True}),
            ("approve", {"decision": "accept"}),
            ("approve", {"skip_ci_request": "Skip CI"}),
            ("decide-review", {"skip_ci_request": "Skip CI"}),
        ):
            with self.subTest(command=command, kwargs=kwargs), self.assertRaises(InputError):
                build_inputs(command, {}, **kwargs)

    def test_ci_waiver_maps_to_inputs_only_for_merge(self):
        self.assertEqual(build_inputs("merge", {}, skip_ci_request="Skip CI"), {"skipCiRequest": "Skip CI"})
        self.assertEqual(build_inputs("ready", {}, handoff=HANDOFF, request="Merge", skip_ci_request="Skip CI"),
                         {"handoffId": HANDOFF, "mergeRequest": "Merge"})
        self.assertEqual(build_inputs("drive", {}, skip_ci_request="Skip CI"), {})

    def test_chain_flags_apply_only_where_they_change_something(self):
        for command in CHAINED:
            check_chain_flags(command, no_drive=True)
        check_chain_flags("drive", no_merge=True)
        check_chain_flags("ready", no_merge=True)
        for command, kwargs in (("drive", {"no_drive": True}), ("closeout", {"no_drive": True}),
                                ("rollback", {"no_drive": True}), ("merge", {"no_merge": True}),
                                ("status", {"no_merge": True}), ("decide-review", {"no_merge": True}),
                                ("resume-rollback", {"no_merge": True})):
            with self.subTest(command=command, kwargs=kwargs), self.assertRaises(InputError):
                check_chain_flags(command, **kwargs)


class ChainingTests(unittest.TestCase):
    def test_only_ready_and_drive_merge_automatically(self):
        self.assertTrue(auto_merge_allowed("ready"))
        self.assertTrue(auto_merge_allowed("drive"))
        self.assertFalse(auto_merge_allowed("drive", no_merge=True))
        self.assertFalse(auto_merge_allowed("ready", no_merge=True))
        # decide-review on a legacy ready state and resume-rollback to merge can
        # reach an authorized integrate handoff; they stop there.
        for command in (CHAINED | {"status"}) - MERGE_REACHING:
            with self.subTest(command=command):
                self.assertFalse(auto_merge_allowed(command))
        # merge's follow-up drive never merges a second time.
        self.assertFalse(auto_merge_allowed("merge"))

    def test_should_auto_merge_truth_table(self):
        self.assertTrue(should_auto_merge(integrate()))
        # repair leaves lastFailure set until the next merge begins; it must not suppress a fresh merge.
        self.assertTrue(should_auto_merge(integrate(lastFailure="CI failed before repair")))
        for handoff in (
            integrate(attempt={"id": "existing"}),
            integrate(mergeAuthorization=None),
            integrate(phase="implementing"),
            {**integrate(), "decision": "authorize_merge"},
            {**integrate(), "status": "invalidated"},
            {"status": "current", "decision": "integrate"},
        ):
            with self.subTest(handoff=handoff):
                self.assertFalse(should_auto_merge(handoff))

    def fakes(self, handoffs, merge_codes):
        calls = []

        def drive(inputs):
            calls.append(("drive", inputs))
            return handoffs.pop(0)

        def merge():
            calls.append(("merge",))
            return merge_codes.pop(0)
        return calls, drive, merge

    def test_merge_runs_at_most_once_then_drives_with_later_inputs(self):
        summarize = {"status": "current", "decision": "summarize", "state": {"phase": "merged"}}
        calls, drive, merge = self.fakes([integrate(), summarize], [0])
        self.assertEqual(continue_to_gate(drive, merge, {"agent": 1}, {"control": 2}), (summarize, 0))
        self.assertEqual(calls, [("drive", {"agent": 1}), ("merge",), ("drive", {"control": 2})])
        # A second integrate after the merge is presented, not merged again.
        calls, drive, merge = self.fakes([integrate(), integrate()], [0, 0])
        self.assertEqual(continue_to_gate(drive, merge, {}, {})[0], integrate())
        self.assertEqual([call[0] for call in calls], ["drive", "merge", "drive"])

    def test_failed_merge_still_drives_and_returns_its_code(self):
        blocked = {"status": "current", "decision": "blocked", "state": {"phase": "ready"}}
        calls, drive, merge = self.fakes([integrate(), blocked], [101])
        self.assertEqual(continue_to_gate(drive, merge, {}, {}), (blocked, 101))
        self.assertEqual([call[0] for call in calls], ["drive", "merge", "drive"])

    def test_disallowed_or_completed_merge_never_merges(self):
        for kwargs in ({"allow_merge": False}, {"merge_ran": True}):
            with self.subTest(kwargs=kwargs):
                calls, drive, merge = self.fakes([integrate()], [])
                self.assertEqual(continue_to_gate(drive, merge, {}, {}, **kwargs), (integrate(), 0))
                self.assertEqual(calls, [("drive", {})])

    def test_non_integrate_handoff_drives_once(self):
        handoff = {"status": "current", "decision": "approve_plan", "state": {"phase": "planned"}}
        calls, drive, merge = self.fakes([handoff], [])
        self.assertEqual(continue_to_gate(drive, merge, {}, {}), (handoff, 0))
        self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()
