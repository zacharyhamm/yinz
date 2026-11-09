/** These tests establish fixture behavior; they do not measure reviewer quality. */
import {
  existingRegression,
  runChecks,
} from "./fixtures/issue_review/technical.ts";
import { cancelExport, ExportJob, help } from "./fixtures/issue_review/ux.ts";
import { after, before } from "./fixtures/issue_review/control.ts";
import { taskPrompt } from "../_lib/issue_agents.ts";
import { createState, transition } from "../_lib/lifecycle.ts";

function assert(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}

Deno.test("review fixture: final passing check masks earlier failure while weak assertion passes", () => {
  const result = runChecks([() => false, () => true]);
  assert(result.passed, "Fixture no longer demonstrates masked failure");
  assert(result.executed.length === 2, "All checks must execute");
  assert(existingRegression(), "The weak assertion should still pass");
});
Deno.test("review fixture: cancellation acknowledgement precedes draining completion without guidance", () => {
  const job = new ExportJob();
  const message = cancelExport(job);
  assert(message === "Export cancelled", "Fixture output changed");
  assert(
    String(job.state) === "draining" && job.pendingRows === 2,
    "Work remains",
  );
  job.tick();
  assert(Number(job.pendingRows) === 1, "Accepted rows continue writing");
  job.tick();
  assert(
    String(job.state) === "cancelled",
    "Cancellation eventually completes",
  );
  assert(!help.includes("status"), "Fixture has no status guidance");
});
Deno.test("review fixture: private extraction preserves public output including whitespace", () => {
  for (const name of ["Ada", "  Ada  ", "", " \n "]) {
    assert(before(name) === after(name), "Public output changed");
  }
});
Deno.test("production review prompts retain independent duties, standards and exact identity", () => {
  const commit = "a".repeat(40);
  let state = createState({
    workspace: "test",
    itemId: 1,
    text: "Preserve command behavior",
    note: "Caller requires every check to pass",
    digest: "b".repeat(64),
  });
  state = transition(state, "triage", {
    classification: "feature",
    summary: "Inspect commands",
  });
  state = transition(state, "plan", {
    summary: "Inspect command behavior",
    steps: [{
      id: "command",
      description: "Update command",
      files: ["command.ts"],
    }],
    testingStrategy: "Focused command check",
    checks: [{
      id: "command",
      command: ["deno", "test", "command.ts"],
      covers: ["command"],
    }],
  });
  for (const role of ["review_code", "review_ux", "review_plan"] as const) {
    const prompt = taskPrompt(
      role,
      state,
      "/fixture",
      "/standards",
      "Evaluation-only response contract",
      commit,
    );
    for (
      const text of [
        "/standards/review.md",
        "/standards/dimensions.md",
        state.issue.note,
        `The commit being reviewed is ${commit}.`,
        "planVersion",
        "focused-test",
        "reopening",
        "Evaluation-only response contract",
      ]
    ) {
      assert(prompt.includes(text), `${role} missing ${text}`);
    }
    if (role === "review_code") {
      for (
        const text of [
          "incorrect assumptions",
          "state transitions",
          "cleanup",
          "concurrency",
          "assertions that could pass",
          "exactly once",
          "advisory",
        ]
      ) assert(prompt.includes(text), text);
    } else if (role === "review_ux") {
      for (
        const text of [
          "affected users and tasks",
          "waiting",
          "cancellation",
          "recovery",
          "no_material_impact",
          "limitations",
          "user consequence",
          "aesthetic",
          "meaningful user decision",
        ]
      ) assert(prompt.includes(text), text);
    } else {assert(
        prompt.includes(
          "valid command argument arrays that execute directly without a shell",
        ),
        "Plain command wording",
      );}
  }
});
