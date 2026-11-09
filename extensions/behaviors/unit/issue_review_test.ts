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

Deno.test("review evaluation sources retain the byte identities used by live reviewers", async () => {
  const identities = {
    technical:
      "a93312b9d026a31204f99c6b1f1c4d22ad7d735e17b0d38cae72547d020093e2",
    ux: "a827a2e564c77062efd8abfbe7d3c5aeae5691a6bdec88c56dae0c8ef19c0728",
    control: "5adaeb48a7a59f05c5ed55c86dd29c7f5c97ba145c286d7456d33663350ebad9",
  };
  for (const [name, expected] of Object.entries(identities)) {
    const source = await Deno.readFile(
      new URL(`fixtures/issue_review/${name}.ts`, import.meta.url),
    );
    const digest = new Uint8Array(
      await crypto.subtle.digest("SHA-256", source),
    );
    const actual = Array.from(
      digest,
      (byte) => byte.toString(16).padStart(2, "0"),
    ).join("");
    assert(
      actual === expected,
      `${name} changed: repeat and record the bounded live evaluation before updating its identity`,
    );
  }
});
