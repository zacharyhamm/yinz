import { join } from "node:path";
import {
  assignmentFileContents,
  type AssignmentFiles,
  assignmentManifest,
  type AssignmentSource,
  evidenceTemplate,
  FILE_NAMES,
  publishAssignmentFiles,
} from "../_lib/assignment_files.ts";
import {
  assignmentCore,
  AssignmentSchema,
  type AssignmentTask,
  canonical,
  evidenceDigest,
  reviewPacket,
  validateEvidence,
} from "../_lib/issue_evidence.ts";
import {
  codeReviewFingerprint,
  createState,
  implementationReviewFingerprint,
  type LifecycleState,
  planningGeneration,
  priorFindingObligations,
  selectedRepairFindings,
  transition,
} from "../_lib/lifecycle.ts";
import { AgentConfigSchema } from "../_lib/yinz_agent.ts";

function assert(v: unknown, message = "Assertion failed"): asserts v {
  if (!v) throw new Error(message);
}
function equal(actual: unknown, expected: unknown, message?: string) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  assert(a === e, message ?? `${a} !== ${e}`);
}
function throws(f: () => unknown, text: string) {
  try {
    f();
  } catch (e) {
    assert(String(e).includes(text), String(e));
    return;
  }
  throw new Error(`Expected ${text}`);
}
async function rejects(f: () => Promise<unknown>, text: string) {
  try {
    await f();
  } catch (e) {
    assert(String(e).includes(text), String(e));
    return;
  }
  throw new Error(`Expected ${text}`);
}

const head = "a".repeat(40);
const id = "a6da10b0-c980-4b82-b920-000000000007";
const issue = {
  workspace: "test",
  itemId: 1,
  text: "All checks execute and all must pass",
  note: "Preserve public greetings",
  digest: "b".repeat(64),
};
const plan = {
  summary: "Repair checks",
  steps: [
    { id: "check", description: "Fix checks", files: ["check.ts"] },
    { id: "docs", description: "Document checks", files: ["README.md"] },
  ],
  checks: [{
    id: "check",
    command: ["deno", "test", "check.ts"],
    covers: ["check", "docs"],
  }],
  testingStrategy: "Focused",
};
const baseline = {
  main: "c".repeat(40),
  mergeBase: "c".repeat(40),
  head,
  featureDiff: "complete feature diff\n",
  ancestry: "initial" as const,
  baselineChanged: false,
  limitations: [],
};
const finding = {
  id: "same",
  severity: "high" as const,
  description: "Earlier failure lost",
  affectedPaths: ["check.ts"],
  checkIds: ["check"],
  evidence: ["check.ts:3"],
  requirement: "all checks must pass",
  mechanism: "overwrites earlier failure",
};

function classified() {
  return transition(createState(issue), "triage", {
    classification: "bug",
    summary: "Reproduced",
  });
}
function planned() {
  return transition(classified(), "plan", plan);
}
function revising() {
  return transition(planned(), "review_plan", {
    planVersion: 1,
    findings: [{ ...finding, id: "blocking" }, {
      id: "minor",
      severity: "low",
      description: "Naming",
    }],
  });
}
function implementing() {
  let s = transition(planned(), "review_plan", {
    planVersion: 1,
    findings: [],
  });
  s = transition(s, "approve", { planVersion: 1, request: "Implement" });
  return transition(s, "implement", { worktree: "/repo", branch: "feature" });
}
function reviewed() {
  let s = transition(implementing(), "review_code", {
    planVersion: 1,
    commit: head,
    steps: plan.steps.map((step) => ({ id: step.id, status: "implemented" })),
    findings: [finding],
    baseline,
  });
  s = transition(s, "review_ux", {
    planVersion: 1,
    commit: head,
    findings: [{ ...finding, description: "Premature success feedback" }],
    baseline,
    assessment: {
      impact: "material",
      summary: "Operator misled",
      assessedPaths: ["check.ts"],
      limitations: [],
    },
  });
  return s;
}
/** Re-implementation after a review: both roles have archived unresolved findings. */
function rereviewing() {
  return transition(reviewed(), "implement", {
    worktree: "/repo",
    branch: "feature",
  });
}
function selected() {
  const s = reviewed();
  return transition(s, "decide_review", {
    id,
    reviewFingerprint: codeReviewFingerprint(s),
    request: "Plan both fixes",
    repair: [{ role: "review_code", findingId: "same" }, {
      role: "review_ux",
      findingId: "same",
    }],
    ignore: [],
  });
}
function repairApproved() {
  const s = selected();
  const next = transition(s, "plan_review_fixes", {
    ...plan,
    findingMappings: selectedRepairFindings(s).map((p) => ({
      finding: p.ref,
      steps: ["check"],
      checks: ["check"],
    })),
  });
  return transition(next, "approve_review_fixes", {
    id,
    handoffId: id,
    request: "Implement both fixes",
  });
}

type Role = AssignmentTask["role"];
const IMPLEMENTATION_ROLES: Role[] = [
  "implement",
  "review_code",
  "review_ux",
  "plan_review_fixes",
  "implement_review_fixes",
];

function source(
  state: LifecycleState,
  role: Role,
  override: Partial<NonNullable<AssignmentSource["continuity"]>> = {},
) {
  const reviewCommit = role === "implement" || role === "triage" ||
      role === "plan" || role === "review_plan"
    ? undefined
    : head;
  const task = {
    id,
    behaviorName: `issue-agent-${id}`,
    role,
    before: state,
    ...(reviewCommit ? { reviewCommit } : {}),
    config: AgentConfigSchema.parse({
      session: "test",
      kind: "codex",
      cwd: "/repo",
    }),
    contractVersion: 3 as const,
    round: {
      assignmentId: id,
      role,
      at: "2026-11-11T00:00:00Z",
      issueDigest: state.issue.digest,
      generation: planningGeneration(state),
      launchCauses: [{
        kind: "unknown" as const,
        explanation: "Fixture",
        evidence: ["assignment fixture"],
        source: "controller" as const,
      }],
      corrections: [],
    },
    continuity: {
      version: 1 as const,
      assignmentId: id,
      issueDigest: state.issue.digest,
      generation: planningGeneration(state),
      rollbackId: state.rollbackId,
      planVersion: state.planVersion,
      packetDigest: evidenceDigest(reviewPacket(state)),
      reviewFingerprints: (["review_code", "review_ux"] as const).flatMap((
        r,
      ) =>
        (r === "review_code" ? state.codeReview : state.uxReview)
          ? [implementationReviewFingerprint(state, r)]
          : []
      ),
      ...(IMPLEMENTATION_ROLES.includes(role) ? { baseline } : {}),
      ...override,
    },
  };
  const core = assignmentCore(
    { modelId: "parent", definition: { name: "issue-test-1" } },
    task,
    "/control",
  );
  const assignment = AssignmentSchema.parse({
    ...core,
    digest: evidenceDigest(core),
    promptDigest: "d".repeat(64),
  });
  const submission = {
    protocol: 1 as const,
    behaviorName: `issue-evidence-${id}`,
    controlDir: "/control",
    assignmentDigest: assignment.digest,
  };
  return { task: { ...task, submission }, assignment };
}

const decode = (bytes?: Uint8Array) => new TextDecoder().decode(bytes);

Deno.test("assignment files derive deterministically from the saved task", () => {
  const { task } = source(rereviewing(), "review_code");
  const first = assignmentFileContents(task);
  const second = assignmentFileContents(structuredClone(task));
  equal([...first.keys()], [...second.keys()]);
  for (const [key, bytes] of first) {
    assert(decode(bytes) === decode(second.get(key)), key);
  }
  const directory = `/repo/.git/yinz-lifecycle/assignments/${id}`;
  equal(
    assignmentManifest(directory, first),
    assignmentManifest(directory, second),
  );
  const manifest = assignmentManifest(directory, first);
  for (const [key, file] of Object.entries(manifest.files)) {
    equal(
      file?.path,
      join(directory, FILE_NAMES[key as keyof typeof FILE_NAMES]),
    );
  }
  equal(
    decode(first.get("target")),
    canonical({ ...task.submission, taskId: id, role: "review_code" }),
  );
});

Deno.test("packet file digest equals continuity.packetDigest and snapshot is canonical before", () => {
  const { task } = source(repairApproved(), "implement_review_fixes");
  const contents = assignmentFileContents(task);
  const manifest = assignmentManifest("/assignments/x", contents);
  equal(manifest.files.packet.sha256, task.continuity.packetDigest);
  equal(decode(contents.get("packet")), canonical(reviewPacket(task.before)));
  equal(decode(contents.get("snapshot")), canonical(task.before));
  // A packet that no longer matches its bound digest is never published.
  throws(
    () =>
      assignmentFileContents(
        source(repairApproved(), "implement_review_fixes", {
          packetDigest: "f".repeat(64),
        }).task,
      ),
    "packetDigest",
  );
});

Deno.test("base64 baselines decode to raw diff files; utf8 is verbatim; absent delta has no file", () => {
  const raw = new Uint8Array([0xff, 0x00, 0x0a]);
  const encoded = btoa(String.fromCharCode(...raw));
  const binary = assignmentFileContents(
    source(rereviewing(), "review_code", {
      baseline: {
        ...baseline,
        featureDiff: encoded,
        featureDiffEncoding: "base64",
        delta: "endpoint ✓\n",
        deltaEncoding: "utf8",
        previousHead: "e".repeat(40),
      },
    }).task,
  );
  equal([...binary.get("featureDiff")!], [...raw]);
  equal(decode(binary.get("delta")), "endpoint ✓\n");
  const view = JSON.parse(decode(binary.get("baseline")));
  equal(view.featureDiff.file, "feature.diff");
  equal(view.featureDiff.sourceEncoding, "base64");
  equal(view.featureDiff.bytes, 3);
  equal(view.delta.file, "delta.diff");
  assert(!("featureDiffEncoding" in view) && !("deltaEncoding" in view));
  const utf8 = assignmentFileContents(
    source(rereviewing(), "review_code").task,
  );
  equal(decode(utf8.get("featureDiff")), baseline.featureDiff);
  assert(!utf8.has("delta"));
  assert(!("delta" in JSON.parse(decode(utf8.get("baseline")))));
  // Roles without a captured baseline have no diff files.
  const planning = assignmentFileContents(
    source(planned(), "review_plan").task,
  );
  assert(
    !planning.has("baseline") && !planning.has("featureDiff") &&
      !planning.has("delta"),
  );
});

Deno.test("delta code re-review templates carry step conformance and omit follow-ups", () => {
  const changed = "d".repeat(40);
  let s = transition(implementing(), "review_code", {
    planVersion: 1,
    commit: head,
    steps: [{ id: "check", status: "implemented" }, {
      id: "docs",
      status: "deviated",
      justification: "Documented in the command help instead",
    }],
    findings: [finding, {
      id: "nit",
      severity: "low",
      description: "Naming",
      affectedPaths: ["check.ts"],
      evidence: ["check.ts:9"],
    }],
    baseline,
  });
  s = transition(s, "decide_review", {
    id,
    reviewFingerprint: codeReviewFingerprint(s),
    request: "Repair the lost failure",
    repair: [{ role: "review_code", findingId: "same" }],
    ignore: [],
  });
  // No repair plan: the worker's template lists only the selected findings.
  const repair = evidenceTemplate(source(s, "implement_review_fixes").task)
    .evidence;
  equal(
    (repair.responses as { finding: unknown }[]).map((r) => r.finding),
    selectedRepairFindings(s).map((p) => p.ref),
  );
  assert(
    !("checks" in repair),
    "Repair checks are optional, not a placeholder",
  );
  s = transition(s, "implement_review_fixes", {
    summary: "Kept every failure",
    commit: changed,
  });
  const obligations = priorFindingObligations(s, "review_code");
  equal(obligations.map((p) => p.ref.findingId), ["same"]);
  const delta = evidenceTemplate({
    ...source(s, "review_code").task,
    reviewCommit: changed,
    continuity: {
      ...source(s, "review_code").task.continuity,
      scope: {
        kind: "delta",
        base: head,
        paths: ["check.ts"],
        findings: obligations.map((p) => p.ref),
      },
    },
  }).evidence;
  equal(delta.steps, [{ id: "check", status: "implemented" }, {
    id: "docs",
    status: "deviated",
    justification: "Documented in the command help instead",
  }]);
  equal(
    (delta.priorFindings as { finding: unknown }[]).map((p) => p.finding),
    obligations.map((p) => p.ref),
  );
  // Without a delta scope the reviewer assesses every step afresh.
  equal(
    evidenceTemplate(source(s, "review_code").task).evidence.steps,
    plan.steps.map((step) => ({ id: step.id, status: "" })),
  );
});

Deno.test("templates pre-fill identities per role", () => {
  const rereview = rereviewing();
  const code = evidenceTemplate(source(rereview, "review_code").task);
  equal(code.role, "review_code");
  equal(code.evidence.planVersion, 1);
  equal(code.evidence.commit, head);
  equal(
    code.evidence.steps,
    plan.steps.map((s) => ({ id: s.id, status: "" })),
  );
  const obligations = priorFindingObligations(rereview, "review_code");
  assert(obligations.length === 1, "fixture has archived unresolved findings");
  equal(
    (code.evidence.priorFindings as { finding: unknown }[]).map((p) =>
      p.finding
    ),
    obligations.map((p) => p.ref),
  );
  const ux = evidenceTemplate(source(rereview, "review_ux").task);
  equal(ux.evidence.assessment, {
    impact: "",
    summary: "",
    assessedPaths: [],
    limitations: [],
  });
  equal(
    (ux.evidence.priorFindings as { finding: unknown }[]).map((p) => p.finding),
    priorFindingObligations(rereview, "review_ux").map((p) => p.ref),
  );
  assert(!("steps" in ux.evidence));
  const selectedState = selected();
  const identities = selectedRepairFindings(selectedState).map((p) => p.ref);
  assert(identities.length === 2);
  const mappings = evidenceTemplate(
    source(selectedState, "plan_review_fixes").task,
  ).evidence.findingMappings as { finding: unknown }[];
  equal(mappings.map((m) => m.finding), identities);
  const responses = evidenceTemplate(
    source(repairApproved(), "implement_review_fixes").task,
  ).evidence.responses as { finding: unknown }[];
  equal(
    responses.map((r) => r.finding),
    selectedRepairFindings(repairApproved()).map((p) => p.ref),
  );
  const revision = evidenceTemplate(source(revising(), "plan").task).evidence;
  equal(revision.summary, plan.summary);
  equal(revision.steps, plan.steps);
  equal(revision.checks, plan.checks);
  assert(!("version" in revision));
  equal(
    (revision.revision as { responses: { findingId: string }[] }).responses
      .map((r) => r.findingId),
    ["blocking"],
  );
  const initial = evidenceTemplate(source(classified(), "plan").task).evidence;
  assert(!("revision" in initial));
  equal(evidenceTemplate(source(planned(), "review_plan").task).evidence, {
    planVersion: 1,
    findings: [],
  });
});

type Evidence = Record<string, unknown>;
const filled = (role: Role, e: Evidence): Evidence => {
  const step = { id: "check", description: "Fix checks", files: ["check.ts"] };
  const check = {
    id: "check",
    command: ["deno", "test", "check.ts"],
    covers: ["check"],
  };
  const prior = (p: Evidence) => ({
    ...p,
    outcome: "verified_fixed",
    rationale: "Current source inspected",
    evidence: ["check.ts:5"],
  });
  switch (role) {
    case "triage":
      return { classification: "bug", summary: "Reproduced" };
    case "plan":
      return e.revision
        ? {
          ...e,
          revision: {
            ...(e.revision as Evidence),
            summary: "Targeted revision",
            responses: ((e.revision as Evidence).responses as Evidence[]).map((
              r,
            ) => ({
              ...r,
              status: "disputed",
              rationale: "Covered",
              evidence: ["check.ts:1"],
            })),
          },
        }
        : {
          ...e,
          summary: "Plan",
          steps: [step],
          testingStrategy: "Focused",
          checks: [check],
        };
    case "review_plan":
      return e;
    case "implement":
      return { summary: "Done", commit: head };
    case "review_code":
      return {
        ...e,
        steps: (e.steps as Evidence[]).map((s) => ({
          ...s,
          status: "implemented",
        })),
        priorFindings: (e.priorFindings as Evidence[]).map(prior),
      };
    case "review_ux":
      return {
        ...e,
        priorFindings: (e.priorFindings as Evidence[]).map(prior),
        assessment: {
          impact: "no_material_impact",
          summary: "Output unchanged",
          assessedPaths: ["check.ts"],
          limitations: [],
        },
      };
    case "plan_review_fixes":
      return {
        ...e,
        summary: "Repair",
        steps: [step],
        testingStrategy: "Focused",
        checks: [check],
        findingMappings: (e.findingMappings as Evidence[]).map((m) => ({
          ...m,
          steps: ["check"],
          checks: ["check"],
        })),
      };
    case "implement_review_fixes":
      return {
        ...e,
        summary: "Repaired",
        commit: head,
        responses: (e.responses as Evidence[]).map((r) => ({
          ...r,
          status: "addressed",
          rationale: "Fixed and probed",
          affectedPaths: ["check.ts"],
          evidence: ["probe.log"],
          checkIds: ["check"],
        })),
      };
  }
};

Deno.test("an unedited template is rejected and filling only placeholders is accepted", () => {
  const cases: [Role, LifecycleState][] = [
    ["triage", createState(issue)],
    ["plan", classified()],
    ["plan", revising()],
    ["review_plan", planned()],
    ["implement", implementing()],
    ["review_code", rereviewing()],
    [
      "review_ux",
      transition(rereviewing(), "review_code", {
        planVersion: 1,
        commit: head,
        steps: plan.steps.map((s) => ({ id: s.id, status: "implemented" })),
        findings: [],
        priorFindings: priorFindingObligations(rereviewing(), "review_code")
          .map((p) => ({
            finding: p.ref,
            outcome: "verified_fixed",
            rationale: "Fixed",
            evidence: ["check.ts:5"],
          })),
        baseline,
      }),
    ],
    ["plan_review_fixes", selected()],
    ["implement_review_fixes", repairApproved()],
  ];
  for (const [role, state] of cases) {
    const { task, assignment } = source(state, role);
    const template = evidenceTemplate(task);
    // The schema file describes exactly what the helper submits.
    JSON.parse(decode(assignmentFileContents(task).get("schema")));
    if (role === "review_plan") {
      // An empty findings list is itself a complete plan review; there is no
      // placeholder to leave unfilled (see the template contract).
      validateEvidence(assignment, template);
    } else {
      let rejected = false;
      try {
        validateEvidence(assignment, template);
      } catch {
        rejected = true;
      }
      assert(rejected, `${role} unedited template must be rejected`);
    }
    const result = validateEvidence(assignment, {
      role,
      evidence: filled(role, template.evidence),
    });
    equal(result.payload.role, role);
  }
});

async function tempTask() {
  const root = await Deno.makeTempDir();
  const { task } = source(rereviewing(), "review_code");
  const directory = join(
    await Deno.realPath(root),
    "common",
    "yinz-lifecycle",
    "assignments",
    id,
  );
  const assignmentFiles: AssignmentFiles = assignmentManifest(
    directory,
    assignmentFileContents(task),
  );
  return { root, directory, task: { ...task, assignmentFiles } };
}
async function listing(directory: string) {
  const entries = [];
  for await (const entry of Deno.readDir(directory)) {
    const info = await Deno.lstat(join(directory, entry.name));
    entries.push(`${entry.name}:${info.ino}:${info.mode! & 0o777}`);
  }
  return entries.sort();
}

Deno.test("publication is private, immutable and idempotent", async () => {
  const { root, directory, task } = await tempTask();
  try {
    await publishAssignmentFiles(task);
    equal((await Deno.lstat(directory)).mode! & 0o777, 0o700);
    const names = [
      ...Object.keys(task.assignmentFiles.files).map((k) =>
        FILE_NAMES[k as keyof typeof FILE_NAMES]
      ),
      "manifest.json",
    ].sort();
    const first = await listing(directory);
    equal(first.map((e) => e.split(":")[0]), names);
    assert(first.every((e) => e.endsWith(`:${0o400}`)), first.join(","));
    equal(
      await Deno.readTextFile(join(directory, "manifest.json")),
      canonical(task.assignmentFiles),
    );
    // A second publication writes nothing and leaves no temporary file.
    await publishAssignmentFiles(structuredClone(task));
    equal(await listing(directory), first);

    // Tampering, even at the same size, refuses delivery.
    const packet = join(directory, "packet.json");
    const original = await Deno.readFile(packet);
    await Deno.chmod(packet, 0o600);
    const tampered = new Uint8Array(original);
    tampered[0] = tampered[0] === 0x7b ? 0x5b : 0x7b;
    await Deno.writeFile(packet, tampered);
    await Deno.chmod(packet, 0o400);
    await rejects(() => publishAssignmentFiles(task), "differs");
    // A writable copy with the right bytes is still not the published file.
    await Deno.chmod(packet, 0o600);
    await Deno.writeFile(packet, original);
    await rejects(() => publishAssignmentFiles(task), "differs");
    await Deno.remove(packet);

    // A deleted file is regenerated byte for byte.
    await Deno.remove(join(directory, "evidence-template.json"));
    await publishAssignmentFiles(task);
    equal(
      (await listing(directory)).map((e) => e.split(":")[0]),
      names,
    );
    equal(await Deno.readFile(packet), original);

    // Regeneration that differs from the manifest fails closed.
    await Deno.remove(join(directory, "target.json"));
    await rejects(
      () =>
        publishAssignmentFiles({
          ...task,
          submission: { ...task.submission, controlDir: "/elsewhere" },
        }),
      "cannot be reproduced",
    );
    await rejects(
      () =>
        publishAssignmentFiles({
          ...task,
          continuity: { ...task.continuity, packetDigest: "f".repeat(64) },
        }),
      "cannot be reproduced",
    );
    await publishAssignmentFiles(task);

    // A directory that is not private is refused before any file is written.
    await Deno.chmod(directory, 0o755);
    await rejects(() => publishAssignmentFiles(task), "private");
    await Deno.chmod(directory, 0o700);
  } finally {
    await Deno.chmod(directory, 0o700).catch(() => {});
    await Deno.remove(root, { recursive: true });
  }
});
