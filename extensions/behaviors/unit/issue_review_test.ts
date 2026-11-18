/** These tests establish fixture behavior; they do not measure reviewer quality. */
import {
  existingRegression,
  runChecks,
} from "./fixtures/issue_review/technical.ts";
import { cancelExport, ExportJob, help } from "./fixtures/issue_review/ux.ts";
import { after, before } from "./fixtures/issue_review/control.ts";
import { taskPrompt } from "../_lib/issue_agents.ts";
import { CHECK_POLICY_SUMMARY } from "../_lib/check_policy.ts";
import {
  createState,
  planningGeneration,
  transition,
} from "../_lib/lifecycle.ts";
import {
  assignmentFileContents,
  assignmentManifest,
} from "../_lib/assignment_files.ts";
import { evidenceDigest, reviewPacket } from "../_lib/issue_evidence.ts";
import { ContinuityContext } from "../_lib/review_continuity.ts";

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
function reviewState() {
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
  return state;
}
function assertReviewDuties(
  role: "review_code" | "review_ux" | "review_plan",
  prompt: string,
  identity: string[],
) {
  for (
    const text of [
      "/standards/review.md",
      "/standards/dimensions.md",
      ...identity,
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
  } else {
    assert(
      prompt.includes(
        "valid command argument arrays that execute directly without a shell",
      ),
      "Plain command wording",
    );
  }
}
Deno.test("production review prompts retain independent duties, standards and exact identity", () => {
  const commit = "a".repeat(40);
  const state = reviewState();
  for (const role of ["review_code", "review_ux", "review_plan"] as const) {
    const prompt = taskPrompt(
      role,
      state,
      "/fixture",
      "/standards",
      "Evaluation-only response contract",
      commit,
    );
    assertReviewDuties(role, prompt, [
      state.issue.note,
      `The commit being reviewed is ${commit}.`,
    ]);
  }
});

Deno.test("contract-3 review prompts retain duties and reference private assignment files", () => {
  const commit = "a".repeat(40);
  const state = reviewState();
  const id = "a6da10b0-c980-4b82-b920-000000000003";
  for (const role of ["review_code", "review_ux", "review_plan"] as const) {
    const featureDiff = "diff --git a/command.ts b/command.ts";
    const continuity = ContinuityContext.parse({
      version: 1,
      assignmentId: id,
      issueDigest: state.issue.digest,
      generation: planningGeneration(state),
      planVersion: state.planVersion,
      packetDigest: evidenceDigest(reviewPacket(state)),
      reviewFingerprints: [],
      ...(role === "review_plan" ? {} : {
        baseline: {
          main: "c".repeat(40),
          mergeBase: "c".repeat(40),
          head: commit,
          featureDiff,
          ancestry: "initial",
          baselineChanged: false,
          limitations: [],
        },
      }),
    });
    const files = assignmentManifest(
      `/repo/.git/yinz-lifecycle/assignments/${id}`,
      assignmentFileContents({
        id,
        role,
        before: state,
        ...(role === "review_plan" ? {} : { reviewCommit: commit }),
        continuity,
        submission: {
          protocol: 1,
          behaviorName: `issue-evidence-${id}`,
          controlDir: "/control",
          assignmentDigest: "d".repeat(64),
        },
      }),
    );
    const prompt = taskPrompt(
      role,
      state,
      "/fixture",
      "/standards",
      "Evaluation-only response contract",
      commit,
      undefined,
      continuity,
      { files },
    );
    assertReviewDuties(role, prompt, [
      `The commit being reviewed is ${commit}.`,
    ]);
    for (
      const text of [
        "Assignment contract version 3. Assignment files are read-only",
        `read ${files.files.packet.path} completely (sha256 ${continuity.packetDigest}`,
        `resolve inside ${files.files.snapshot.path}`,
      ]
    ) assert(prompt.includes(text), `${role} missing ${text}`);
    assert(
      prompt.includes(`Start from ${files.files.template.path};`) ===
        (role !== "review_plan"),
      `${role} template sentence`,
    );
    assert(!prompt.includes(featureDiff), `${role} inlines the diff`);
    assert(!prompt.includes("Bound review continuity: {"), role);
    assert(!prompt.includes("Decode featureDiff/delta from base64"), role);
    // The requirements stay authoritative through the bound snapshot file.
    assert(
      new TextDecoder().decode(
        assignmentFileContents({
          id,
          role,
          before: state,
          continuity,
          submission: {
            protocol: 1,
            behaviorName: `issue-evidence-${id}`,
            controlDir: "/control",
            assignmentDigest: "d".repeat(64),
          },
        }).get("snapshot"),
      ).includes(state.issue.note),
      `${role} snapshot keeps the requirements`,
    );
  }
});

Deno.test("implementation prompts allow a rebase only for recorded main conflicts and explain log access", () => {
  const state = transition(
    transition(
      createState({
        workspace: "test",
        itemId: 1,
        text: "Preserve command behavior",
        note: "Caller requires every check to pass",
        digest: "b".repeat(64),
      }),
      "triage",
      { classification: "feature", summary: "Inspect commands" },
    ),
    "plan",
    {
      summary: "Inspect command behavior",
      steps: [{ id: "command", description: "Update", files: ["c.ts"] }],
      testingStrategy: "Focused command check",
      checks: [{
        id: "command",
        command: ["deno", "test"],
        covers: ["command"],
      }],
    },
  );
  const logAccess = { issueName: "issue-test-1", controlDir: "/control dir" };
  const prompt = (
    role: Parameters<typeof taskPrompt>[0],
    lastFailure?: string,
    extras: Parameters<typeof taskPrompt>[8] = { logAccess },
  ) =>
    taskPrompt(
      role,
      { ...state, lastFailure },
      "/fixture",
      "/standards",
      "Submission",
      undefined,
      undefined,
      undefined,
      extras,
    );
  const access =
    `Read each log named in lastFailure (read-only; this is not the evidence interface) with: env -u SWAMP_SERVE_URL -u SWAMP_SERVER_URL -u SWAMP_REPO_DIR -u SWAMP_WORKFLOWS_DIR swamp data query 'modelName == "issue-test-1" && name == "<log>"' --select content --repo-dir '/control dir'`;
  const failed =
    "Verification b6da10b0-c980-4b82-b920-640238095f1a failed at " +
    `${"a".repeat(40)} (plan version 1): baseline lint (exit 101; log: ` +
    "baseline-log-b6da10b0-c980-4b82-b920-640238095f1a-lint)";
  const implement = prompt("implement", failed);
  assert(implement.includes(access), implement);
  assert(
    implement.includes(
      "Do not rebase, except when lastFailure records conflicts with local main, or a failed baseline check that local main (refs/heads/main) already fixes: then rebase this branch onto local main",
    ),
    "Rebase wording",
  );
  assert(
    implement.includes("prefer that rebase over re-fixing main's lines here"),
    "Stale-base baseline wording",
  );
  assert(!implement.includes("Do not rebase, merge"), "Old rebase wording");
  assert(
    prompt("implement_review_fixes", "x (exit 2; log: check-log-id-x)")
      .includes(access),
    "Repair implementer reads check logs",
  );
  for (
    const [role, lastFailure, extras] of [
      ["implement", undefined, { logAccess }],
      ["implement", "Merge attempt a failed: exit 101; log: /repo/ci.log", {
        logAccess,
      }],
      ["implement", failed, {}],
      ["review_code", failed, { logAccess }],
    ] as const
  ) {
    assert(
      !prompt(role, lastFailure, extras).includes("Read each log named"),
      `${role} ${lastFailure} must not explain log access`,
    );
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

Deno.test("paired continuity evaluation artifacts retain evaluated source identity and valid role obligations", async () => {
  const { ActionSchemas, StateSchema, validateImplementationReview } =
    await import("../_lib/lifecycle.ts");
  const root = new URL(
    "../../../docs/evaluations/lifecycle-continuity/reviewer-results/",
    import.meta.url,
  );
  const manifest = JSON.parse(
    await Deno.readTextFile(new URL("manifest.json", root)),
  );
  const source = await Deno.readFile(
    new URL("fixtures/issue_review/continuity.ts", import.meta.url),
  );
  const sha = async (bytes: Uint8Array) =>
    Array.from(
      new Uint8Array(
        await crypto.subtle.digest("SHA-256", new Uint8Array(bytes)),
      ),
      (b) => b.toString(16).padStart(2, "0"),
    ).join("");
  assert(
    await sha(source) === manifest.fixtureSourceSha256,
    "Repeat bounded live evaluation after changing the continuity fixture",
  );
  for (const [name, expected] of Object.entries(manifest.artifacts)) {
    assert(
      await sha(await Deno.readFile(new URL(name, root))) === expected,
      `Evaluated artifact ${name} changed`,
    );
  }
  for (const mode of ["original", "context"]) {
    const result = JSON.parse(
      await Deno.readTextFile(new URL(`${mode}-final.json`, root)),
    );
    assert(
      result.cases.length === 4,
      "All paired scenarios must remain available",
    );
    for (const item of result.cases) {
      const review = ActionSchemas.review_code.parse(item.review);
      if (mode === "context") {
        const state = StateSchema.parse(
          JSON.parse(
            await Deno.readTextFile(
              new URL(`context-${item.case}-state.json`, root),
            ),
          ),
        );
        validateImplementationReview(state, "review_code", review, true);
      }
      assert(
        review.findings.every((f) => !f.resolution),
        "Neither new defects nor ignored continuity may be silently resolved",
      );
    }
  }
});

Deno.test("planning and plan-review prompts state the submission check policy", () => {
  const state = transition(
    createState({
      workspace: "test",
      itemId: 1,
      text: "Preserve command behavior",
      note: "Caller requires every check to pass",
      digest: "b".repeat(64),
    }),
    "triage",
    { classification: "feature", summary: "Inspect commands" },
  );
  for (
    const role of [
      "triage",
      "plan",
      "review_plan",
      "implement",
      "review_code",
      "review_ux",
      "plan_review_fixes",
      "implement_review_fixes",
    ] as const
  ) {
    const prompt = taskPrompt(role, state, "/fixture", "/standards", "Submit");
    assert(
      prompt.includes(CHECK_POLICY_SUMMARY) ===
        ["plan", "review_plan", "plan_review_fixes", "implement_review_fixes"]
          .includes(role),
      `${role} check policy summary`,
    );
  }
});

Deno.test("repair workers fix selected findings directly and may add focused checks", () => {
  const prompt = taskPrompt(
    "implement_review_fixes",
    reviewState(),
    "/fixture",
    "/standards",
    "Submit",
  );
  for (
    const text of [
      "working directly from those findings",
      "there is no separate repair plan or approval",
      "A legacy repair plan in that entry, if present, is guidance only",
      "a new short id that no plan check uses",
      "report a finding that needs broader work as unresolved",
      CHECK_POLICY_SUMMARY,
    ]
  ) assert(prompt.includes(text), `repair prompt missing ${text}`);
  assert(!prompt.includes("approved repair plan"), "Old repair-plan wording");
});

Deno.test("delta re-review prompts bound scope to the repair diff and prior findings", () => {
  const base = "c".repeat(40);
  const commit = "a".repeat(40);
  const state = reviewState();
  const id = "a6da10b0-c980-4b82-b920-000000000004";
  const baseline = {
    main: "e".repeat(40),
    mergeBase: "e".repeat(40),
    head: commit,
    featureDiff: "diff --git a/command.ts b/command.ts",
    previousHead: base,
    delta: "diff --git a/command.ts b/command.ts",
    ancestry: "descendant",
    baselineChanged: false,
    limitations: [],
    deltaPaths: ["command.ts"],
  };
  const continuity = (scope: unknown) =>
    ContinuityContext.parse({
      version: 1,
      assignmentId: id,
      issueDigest: state.issue.digest,
      generation: planningGeneration(state),
      planVersion: state.planVersion,
      packetDigest: evidenceDigest(reviewPacket(state)),
      reviewFingerprints: [],
      baseline,
      scope,
    });
  const delta = {
    kind: "delta",
    base,
    paths: ["command.ts"],
    findings: [{
      issue: "test/1",
      role: "review_code",
      reviewFingerprint: "f".repeat(64),
      findingId: "race",
    }],
  };
  for (const role of ["review_code", "review_ux"] as const) {
    const prompt = (scope: unknown) => {
      const bound = continuity(scope);
      const files = assignmentManifest(
        `/repo/.git/yinz-lifecycle/assignments/${id}`,
        assignmentFileContents({
          id,
          role,
          before: state,
          reviewCommit: commit,
          continuity: bound,
          submission: {
            protocol: 1,
            behaviorName: `issue-evidence-${id}`,
            controlDir: "/control",
            assignmentDigest: "d".repeat(64),
          },
        }),
      );
      return taskPrompt(
        role,
        state,
        "/fixture",
        "/standards",
        "Evaluation-only response contract",
        commit,
        undefined,
        bound,
        { files },
      );
    };
    const scoped = prompt(delta);
    for (
      const text of [
        "This is a delta re-review after a repair pass, not a new audit.",
        `Review the repair diff from ${base}`,
        "It changes: command.ts.",
        "continuity.scope.findings",
        "its affectedPaths must include a path the repair changed",
        "Do not re-audit unchanged code or raise unrelated or pre-existing concerns.",
        "is context only.",
      ]
    ) assert(scoped.includes(text), `${role} delta prompt missing ${text}`);
    assert(
      !scoped.includes("New defects anywhere in the full feature scope"),
      `${role} delta prompt keeps full-scope wording`,
    );
    assert(
      scoped.includes("the evidence template carries each step's status") ===
        (role === "review_code"),
      `${role} step carry-forward wording`,
    );
    const full = prompt({ kind: "full" });
    assert(full.includes("it never bounds scope"), `${role} full wording`);
    assert(
      full.includes("New defects anywhere in the full feature scope"),
      `${role} full prompt reports anywhere`,
    );
    assert(!full.includes("delta re-review"), `${role} full prompt`);
  }
});
