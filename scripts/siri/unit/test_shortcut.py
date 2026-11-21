import plistlib
import unittest

from scripts.siri.shortcut import workflow


class DictationRecoveryTests(unittest.TestCase):
    def test_missing_cursor_clears_pre_dispatch_rejections(self):
        actions = plistlib.loads(workflow())["WFWorkflowActions"]
        steps = [(a["WFWorkflowActionIdentifier"].removeprefix("is.workflow.actions."),
                  a["WFWorkflowActionParameters"]) for a in actions]
        command = next(i for i, (name, p) in enumerate(steps)
                       if name == "downloadurl" and p["WFURL"]["Value"]["string"].endswith("/command"))

        def branch(steps, source, key, condition, equals=None):
            (get, lookup), (name, start) = steps[:2]
            self.assertEqual((get, lookup["WFDictionaryKey"]), ("getvalueforkey", key))
            self.assertEqual(lookup["WFInput"]["Value"]["OutputUUID"], source["UUID"])
            self.assertEqual((name, start["WFControlFlowMode"]), ("conditional", 0))
            self.assertEqual(start["WFInput"]["Variable"]["Value"]["OutputUUID"], lookup["UUID"])
            self.assertEqual(start["WFCondition"], condition)
            self.assertEqual(start.get("WFConditionalActionString"), equals)
            end = next(i for i, (_, p) in enumerate(steps)
                       if p.get("WFControlFlowMode") == 2 and p["GroupingIdentifier"] == start["GroupingIdentifier"])
            return steps[2:end], steps[end + 1:]

        reply = steps[command][1]
        missing_id, _ = branch(steps[command + 1:], reply, "request_id", 101)
        has_error, retry = branch(missing_id, reply, "error", 100)
        invalid, remainder = branch(has_error, missing_id[0][1], "code", 4, "invalid_dictation")
        oversized, remainder = branch(remainder, missing_id[0][1], "code", 4, "body_too_large")
        self.assertEqual(remainder, [])
        self.assertEqual([name for name, _ in oversized], [name for name, _ in invalid])
        self.assertEqual(oversized[0][1]["WFItems"], invalid[0][1]["WFItems"])
        self.assertEqual([name for name, _ in retry], ["speaktext", "exit"])
        self.assertIn("retry", retry[0][1]["WFText"]["Value"]["string"])
        self.assertEqual([name for name, _ in invalid],
                         ["dictionary", "gettext", "documentpicker.save", "speaktext", "exit"])
        record, content, save, speech, _ = [p for _, p in invalid]
        self.assertIn("replacement", speech["WFText"]["Value"]["string"])

        def fields(record):
            return {item["WFKey"]["Value"]["string"]: item["WFValue"]["Value"]
                    for item in record["WFItems"]["Value"]["WFDictionaryFieldValueItems"]}

        previous = next(p for name, p in reversed(steps[:command]) if name == "dictionary")
        self.assertEqual(fields(record), {**fields(previous), "pending": {"string": "", "attachmentsByRange": {}}})
        refs = content["WFText"]["Value"]["attachmentsByRange"]
        self.assertEqual(refs["{0, 1}"]["OutputUUID"], record["UUID"])
        self.assertEqual(save["WFInput"]["Value"]["OutputUUID"], content["UUID"])
        previous_save = next(p for name, p in reversed(steps[:command]) if name == "documentpicker.save")
        self.assertEqual(save["WFFileDestinationPath"], previous_save["WFFileDestinationPath"])
        self.assertTrue(save["WFSaveFileOverwrite"])
