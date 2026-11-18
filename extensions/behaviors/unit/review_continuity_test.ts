import {
  activeReviewFix,
  archiveReviews,
  codeReviewFingerprint,
  createState,
  implementationMergeGate,
  implementationReviewFingerprint,
  type LifecycleState,
  priorImplementationFindings,
  refreshIssue,
  reviewScope,
  rollbackState,
  selectedRepairFindings,
  transition,
  validateImplementationReview,
  validateRepairMappings,
  validateRepairResponses,
  validateRepairResult,
} from "../_lib/lifecycle.ts";
import {
  assignmentCore,
  AssignmentSchema,
  evidenceDigest,
  reviewPacket,
  validateAssignment,
  validateEvidence,
  validateReceipt,
} from "../_lib/issue_evidence.ts";
import {
  AdvisoryEvidence,
  validateAdvisory,
} from "../_lib/review_continuity.ts";
import { AgentConfigSchema } from "../_lib/yinz_agent.ts";
import {
  approvedDesign,
  completeChecks,
  greetingAfterExtraction,
  greetingAfterRepair,
  greetingBefore,
  repairedChecks,
} from "./fixtures/issue_review/continuity.ts";

function assert(v: unknown, message = "Assertion failed"): asserts v {
  if (!v) throw new Error(message);
}
function equal(actual: unknown, expected: unknown) {
  assert(actual === expected, `${actual} !== ${expected}`);
}
function rejects(f: () => unknown, text: string) {
  try {
    f();
  } catch (e) {
    assert(String(e).includes(text), String(e));
    return;
  }
  throw new Error(`Expected ${text}`);
}
const head = "a".repeat(40);
const id = "a6da10b0-c980-4b82-b920-000000000001";
const issue = {
  workspace: "test",
  itemId: 1,
  text: "All checks execute and all must pass",
  note: "Preserve public greetings",
  digest: "b".repeat(64),
};
const plan = {
  summary: "Repair checks",
  steps: [{ id: "check", description: "Fix checks", files: ["check.ts"] }],
  checks: [{
    id: "check",
    command: ["deno", "test", "check.ts"],
    covers: ["check"],
  }],
  testingStrategy: "Focused",
};
const baseline = {
  main: "c".repeat(40),
  mergeBase: "c".repeat(40),
  head,
  featureDiff: "complete feature diff",
  ancestry: "initial" as const,
  baselineChanged: false,
  limitations: [],
};
function fixture() {
  let s = transition(createState(issue), "triage", {
    classification: "bug",
    summary: "Reproduced",
  });
  s = transition(s, "plan", plan);
  s = transition(s, "review_plan", { planVersion: 1, findings: [] });
  s = transition(s, "approve", { planVersion: 1, request: "Implement" });
  return transition(s, "implement", { worktree: "/repo", branch: "feature" });
}
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
function reviewed() {
  let s = fixture();
  s = transition(s, "review_code", {
    planVersion: 1,
    commit: head,
    steps: [{ id: "check", status: "implemented" }],
    findings: [finding],
    baseline,
  });
  return transition(s, "review_ux", {
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
function assignment(
  state: LifecycleState,
  role: "review_code" | "plan_review_fixes" | "implement_review_fixes" =
    "review_code",
  modern = true,
  contractVersion: 2 | 3 = 2,
  bound: {
    reviewCommit?: string;
    baseline?: Record<string, unknown>;
    scope?: unknown;
  } = {},
) {
  const task = {
    id,
    behaviorName: `issue-agent-${id}`,
    role,
    before: state,
    reviewCommit: bound.reviewCommit ?? head,
    config: AgentConfigSchema.parse({
      session: "test",
      kind: "codex",
      cwd: "/repo",
    }),
  };
  const legacy = assignmentCore(
    { modelId: "parent", definition: { name: "issue-test-1" } },
    task,
    "/repo",
  );
  const core = modern
    ? {
      ...legacy,
      contractVersion,
      round: {
        assignmentId: id,
        role,
        at: "2026-11-11T00:00:00Z",
        issueDigest: state.issue.digest,
        generation: legacy.generation,
        launchCauses: [{
          kind: "unknown",
          explanation: "Fixture",
          evidence: ["assignment fixture"],
          source: "controller",
        }],
        corrections: [],
      },
      continuity: {
        version: 1,
        assignmentId: id,
        issueDigest: state.issue.digest,
        generation: legacy.generation,
        rollbackId: state.rollbackId,
        planVersion: state.planVersion,
        packetDigest: evidenceDigest(reviewPacket(state)),
        reviewFingerprints: (["review_code", "review_ux"] as const).flatMap((
          role,
        ) =>
          (role === "review_code" ? state.codeReview : state.uxReview)
            ? [implementationReviewFingerprint(state, role)]
            : []
        ),
        baseline: bound.baseline ?? baseline,
        ...(bound.scope ? { scope: bound.scope } : {}),
      },
    }
    : legacy;
  return AssignmentSchema.parse({
    ...core,
    digest: evidenceDigest(core),
    promptDigest: "d".repeat(64),
  });
}
for (const version of [2, 3] as const) {
  Deno.test(`continuity: version-${version === 2 ? "two" : "three"} repair receipts retain review archival on replay`, () => {
    let state = selected();
    const findings = selectedRepairFindings(state);
    state = transition(state, "plan_review_fixes", {
      ...plan,
      findingMappings: findings.map((p) => ({
        finding: p.ref,
        steps: ["check"],
        checks: ["check"],
      })),
    });
    state = transition(state, "approve_review_fixes", {
      id,
      handoffId: id,
      request: "Implement both fixes",
    });
    archiveReviews(state, "Earlier review", "review_code");
    const current = assignment(state, "implement_review_fixes", true, version);
    equal(current.contractVersion, version);
    const validated = validateEvidence(current, {
      role: "implement_review_fixes",
      evidence: {
        summary: "Repaired",
        commit: head,
        responses: findings.map((p) => ({
          finding: p.ref,
          status: "addressed",
          rationale: "Fixed and probed",
          affectedPaths: ["check.ts"],
          checkIds: ["check"],
          evidence: ["probe.log"],
        })),
      },
    });
    assert(validated.next.reviewHistory?.length === 2);
    assert(!validated.next.codeReview && !validated.next.uxReview);
    assert(current.before.reviewHistory?.length === 1);
    const receipt = {
      taskId: current.taskId,
      assignmentDigest: current.digest,
      payloadDigest: evidenceDigest(validated.payload),
      acceptedAt: "2026-11-11T00:00:00Z",
      ...validated,
    };
    assert(validateReceipt(current, receipt).next.reviewHistory?.length === 2);
    rejects(
      () =>
        validateReceipt(current, {
          ...receipt,
          next: { ...receipt.next, reviewHistory: state.reviewHistory },
        }),
      "original assignment",
    );
  });
}

Deno.test("continuity: delta scope is controller-bound and stamped on review evidence", () => {
  const repairedHead = "e".repeat(40);
  const repaired = transition(selected(), "implement_review_fixes", {
    summary: "Kept every failure",
    commit: repairedHead,
  });
  const deltaBaseline = {
    ...baseline,
    head: repairedHead,
    previousHead: head,
    delta: "repair diff",
    ancestry: "descendant" as const,
    deltaPaths: ["check.ts"],
  };
  const scope = reviewScope(repaired, "review_code", deltaBaseline);
  assert(scope.kind === "delta" && scope.base === head);
  const bound = (value: unknown) =>
    assignment(repaired, "review_code", true, 3, {
      reviewCommit: repairedHead,
      baseline: deltaBaseline,
      scope: value,
    });
  rejects(
    () => validateAssignment(bound({ kind: "full" })),
    "does not match its assignment",
  );
  const current = bound(scope);
  validateAssignment(current);
  const evidence = {
    planVersion: 1,
    commit: repairedHead,
    steps: [{ id: "check", status: "implemented" }],
    findings: [] as Record<string, unknown>[],
    priorFindings: scope.findings.map((finding) => ({
      finding,
      outcome: "verified_fixed",
      rationale: "Every failure is retained",
      evidence: ["check.ts:3"],
    })),
  };
  const validated = validateEvidence(current, {
    role: "review_code",
    evidence,
  });
  equal(
    JSON.stringify((validated.payload.evidence as { scope?: unknown }).scope),
    JSON.stringify(scope),
  );
  equal(
    JSON.stringify(validated.next.codeReview!.scope),
    JSON.stringify(scope),
  );
  rejects(
    () =>
      validateEvidence(current, {
        role: "review_code",
        evidence: { ...evidence, scope: { kind: "full" } },
      }),
    "Reviewer scope differs",
  );
  rejects(
    () =>
      validateEvidence(current, {
        role: "review_code",
        evidence: {
          ...evidence,
          findings: [{
            id: "unrelated",
            severity: "medium",
            description: "Pre-existing documentation gap",
            affectedPaths: ["README.md"],
            evidence: ["README.md:1"],
          }],
        },
      }),
    "Delta re-review reports only regressions",
  );
});
Deno.test("continuity: role collisions retain separate origins and exact repair coverage", () => {
  const s = selected();
  const selectedFindings = selectedRepairFindings(s);
  assert(selectedFindings.length === 2);
  assert(selectedFindings[0].ref.role !== selectedFindings[1].ref.role);
  const mappings = selectedFindings.map((p) => ({
    finding: p.ref,
    steps: ["check"],
    checks: ["check"],
  }));
  validateRepairMappings(s, { ...plan, findingMappings: mappings }, true);
  rejects(() => validateRepairMappings(s, plan, true), "exactly once");
  rejects(
    () =>
      validateRepairMappings(s, {
        ...plan,
        findingMappings: [mappings[0], mappings[0]],
      }, true),
    "exactly once",
  );
  rejects(
    () =>
      validateRepairMappings(s, {
        ...plan,
        findingMappings: mappings.map((m) => ({ ...m, checks: ["unknown"] })),
      }, true),
    "known steps",
  );
  const responses = selectedFindings.map((p) => ({
    finding: p.ref,
    status: "disputed" as const,
    rationale: "Design conflict",
    affectedPaths: ["check.ts"],
    checkIds: ["check"],
    evidence: ["check.ts:7"],
  }));
  validateRepairResponses(
    s,
    { summary: "Partial", commit: head, responses },
    true,
  );
  rejects(
    () =>
      validateRepairResponses(s, {
        summary: "Partial",
        commit: head,
        responses: responses.slice(1),
      }, true),
    "exactly once",
  );
  rejects(
    () =>
      validateRepairResult(s, {
        summary: "Broader",
        commit: head,
        responses,
        scopeChanges: ["Change public API"],
      }),
    "broader scope needs a plan revision",
  );
});
Deno.test("continuity: archives survive ordinary implementation, refresh and UX-only rollback", () => {
  const s = reviewed();
  const ordinary = transition(s, "implement", {
    worktree: "/repo",
    branch: "feature",
  });
  assert(ordinary.reviewHistory?.length === 2 && !ordinary.codeReview);
  const refreshed = refreshIssue(s, { ...issue, digest: "e".repeat(64) });
  assert(refreshed.reviewHistory?.every((h) => h.issueDigest === issue.digest));
  assert(priorImplementationFindings(refreshed).length === 0);
  const ux = rollbackState(s, "review_ux", id);
  assert(ux.codeReview && !ux.uxReview && ux.reviewHistory?.length === 1);
  archiveReviews(ordinary, "retry");
  assert(ordinary.reviewHistory?.length === 2);
});
Deno.test("continuity: omissions do not close findings; explicit verified fixes need current evidence", () => {
  const s = transition(reviewed(), "implement", {
    worktree: "/repo",
    branch: "feature",
  });
  const prior = priorImplementationFindings(s).find((p) =>
    p.ref.role === "review_code"
  )!;
  const review = {
    planVersion: 1,
    commit: head,
    steps: [{ id: "check", status: "implemented" as const }],
    findings: [],
  };
  rejects(
    () => validateImplementationReview(s, "review_code", review, true),
    "exactly once",
  );
  const fixed = {
    ...review,
    priorFindings: [{
      finding: prior.ref,
      outcome: "verified_fixed" as const,
      rationale: "Executed failure/success sequence",
      evidence: ["probe.log"],
    }],
  };
  validateImplementationReview(s, "review_code", fixed, true);
  const next = transition(s, "review_code", fixed);
  assert(
    priorImplementationFindings(next).find((p) => p.ref.role === "review_code")
      ?.fixed,
  );
  const reopened = { ...review, findings: [{ ...finding, origin: prior.ref }] };
  rejects(
    () => validateImplementationReview(next, "review_code", reopened, true),
    "requires new evidence",
  );
  const evidence = "check.ts:9 newly changed caller";
  validateImplementationReview(next, "review_code", {
    ...reopened,
    findings: [{
      ...reopened.findings[0],
      reopening: {
        rationale: "Caller changed since probe",
        evidence: [evidence],
      },
    }],
    priorFindings: [{
      finding: prior.ref,
      outcome: "reopened",
      rationale: "Earlier fix no longer applies to changed caller",
      evidence: [evidence],
    }],
  }, true);
});
Deno.test("continuity: ignored concerns stay unresolved and require a fresh fingerprint decision", () => {
  let s = reviewed();
  s = transition(s, "decide_review", {
    id,
    reviewFingerprint: codeReviewFingerprint(s),
    request: "Ignore both",
    repair: [],
    ignore: [{ role: "review_code", findingId: "same" }, {
      role: "review_ux",
      findingId: "same",
    }],
  });
  const old = codeReviewFingerprint(s);
  s = transition(s, "implement", { worktree: "/repo", branch: "feature" });
  assert(priorImplementationFindings(s).every((p) => !p.fixed));
  assert(
    s.reviewDecisions![0].reviewFingerprint === old && !activeReviewFix(s),
  );
});
for (const version of [2, 3] as const) {
  Deno.test(`continuity: contract-${version} assignment binding rejects missing context, requirements and baseline tampering; legacy parses`, () => {
    const s = fixture();
    const legacy = assignment(s, "review_code", false);
    validateAssignment(legacy);
    const current = assignment(s, "review_code", true, version);
    equal(current.contractVersion, version);
    validateAssignment(current);
    rejects(
      () => validateAssignment({ ...current, continuity: undefined }),
      "continuity",
    );
    rejects(
      () =>
        validateAssignment({
          ...current,
          before: { ...s, issue: { ...issue, digest: "f".repeat(64) } },
        }),
      "continuity",
    );
    rejects(
      () =>
        validateAssignment({
          ...current,
          continuity: {
            ...current.continuity!,
            baseline: { ...baseline, head: "f".repeat(40) },
          },
        }),
      "continuity",
    );
    const evidence = {
      role: "review_code",
      evidence: {
        planVersion: 1,
        commit: "f".repeat(40),
        steps: [{ id: "check", status: "implemented" }],
        findings: [],
      },
    };
    rejects(() => validateEvidence(current, evidence), "different commit");
    // Changing the contract version changes the bound digest.
    rejects(
      () =>
        validateAssignment({
          ...current,
          contractVersion: version === 2 ? 3 : 2,
        }),
      "digest",
    );
  });
}
const advisory = () =>
  AdvisoryEvidence.parse({
    producingTask: id,
    commit: head,
    requirementDigest: issue.digest,
    argv: ["deno", "test", "check.ts"],
    cwd: "/repo",
    result: "passed",
    outputRef: "swamp:check-log",
    relevantFiles: ["check.ts"],
    tests: ["aggregate"],
    platform: "linux",
    toolchain: "deno 2",
    prerequisites: [],
    clean: true,
    dependencyScopeKnown: true,
    reuse: {
      atCommit: "e".repeat(40),
      rationale: "Compared full dependency scope",
      objectsAvailable: true,
      outputAvailable: true,
      prerequisitesMatch: true,
      dependencyComparison: {
        evidence: ["dependency-comparison.log"],
        unchanged: true,
        compared: [
          "implementation",
          "callers",
          "tests",
          "fixtures",
          "dependencies",
          "build_configuration",
          "requirements",
          "environment",
        ],
      },
    },
  });
Deno.test("continuity: advisory reuse invalidates changed/unknown dependencies, environment, missing evidence and failures", () => {
  const a = advisory();
  const target = a.reuse!.atCommit;
  validateAdvisory(a, issue.digest, target);
  for (
    const patch of [
      { clean: false },
      { dependencyScopeKnown: false },
      { result: "flaky" as const },
      { result: "failed" as const },
      { requirementDigest: "f".repeat(64) },
    ]
  ) {
    rejects(
      () => validateAdvisory({ ...a, ...patch }, issue.digest, target),
      "reuse",
    );
  }
  for (
    const patch of [
      { objectsAvailable: false },
      { outputAvailable: false },
      { prerequisitesMatch: false },
      { dependencyComparison: undefined },
      {
        dependencyComparison: {
          ...a.reuse!.dependencyComparison!,
          unchanged: false,
        },
      },
    ]
  ) {
    rejects(
      () =>
        validateAdvisory(
          { ...a, reuse: { ...a.reuse!, ...patch } },
          issue.digest,
          target,
        ),
      "reuse",
    );
  }
  const s = reviewed();
  s.codeReview!.advisoryEvidence = [{
    ...a,
    reuse: { ...a.reuse!, atCommit: head },
  }];
  // Advisory evidence cannot manufacture the deterministic verification receipt.
  assert(implementationMergeGate(s).kind === "unverified");
  rejects(
    () => transition(s, "ready", { commit: head, mergeRequest: "Merge" }),
    "Run verify",
  );
});
Deno.test("paired reviewer fixtures demonstrate wrong approved design, incomplete repair, outside regression and stable tradeoff", () => {
  assert(approvedDesign([() => false, () => true]));
  let called = 0;
  assert(
    !repairedChecks([() => false, () => {
      called++;
      return true;
    }]),
  );
  assert(called === 0);
  assert(
    !completeChecks([() => false, () => {
      called++;
      return true;
    }]),
  );
  assert(Number(called) === 1);
  assert(greetingBefore(" Ada ") !== greetingAfterRepair(" Ada "));
  for (const name of [" Ada ", "", "\n", "ÉVA"]) {
    assert(greetingBefore(name) === greetingAfterExtraction(name));
  }
});

Deno.test("continuity: packet references retain full historical diffs without copying them into every review", () => {
  const state = reviewed();
  archiveReviews(state, "superseded");
  const packet = reviewPacket(state);
  const ref = packet.reviewArchives[0].review.baseline!.featureDiff;
  assert(
    typeof ref === "object" &&
      ref.sourceRef ===
        "assignment.before.reviewHistory[0].review.baseline.featureDiff",
  );
  assert(
    state.reviewHistory![0].review.baseline!.featureDiff ===
      "complete feature diff",
  );
});
Deno.test("continuity: reuse must retain known original provenance and cannot invent a passing investigation", () => {
  let state = reviewed();
  const execution = { ...advisory(), reuse: undefined };
  state.codeReview!.advisoryEvidence = [execution];
  state = transition(state, "implement", {
    worktree: "/repo",
    branch: "feature",
  });
  const current = assignment(state);
  const prior = priorImplementationFindings(state).find((p) =>
    p.ref.role === "review_code"
  )!;
  const review = {
    role: "review_code",
    evidence: {
      planVersion: 1,
      commit: head,
      steps: [{ id: "check", status: "implemented" }],
      findings: [],
      priorFindings: [{
        finding: prior.ref,
        outcome: "verified_fixed",
        rationale: "Current source inspected",
        evidence: ["check.ts:5"],
      }],
      advisoryEvidence: [{
        ...execution,
        reuse: { ...advisory().reuse!, atCommit: head },
      }],
    },
  };
  validateEvidence(current, review);
  rejects(
    () =>
      validateEvidence(current, {
        ...review,
        evidence: {
          ...review.evidence,
          advisoryEvidence: review.evidence.advisoryEvidence.map((a) => ({
            ...a,
            outputRef: "invented-log",
          })),
        },
      }),
    "original provenance",
  );
});
function goldenPacketStates() {
  const repair = selected();
  const repairPlanned = transition(repair, "plan_review_fixes", {
    ...plan,
    findingMappings: selectedRepairFindings(repair).map((p) => ({
      finding: p.ref,
      steps: ["check"],
      checks: ["check"],
    })),
  });
  const approved = transition(repairPlanned, "approve_review_fixes", {
    id,
    handoffId: id,
    request: "Implement both fixes",
  });
  archiveReviews(approved, "Earlier review", "review_code");
  // Records saved before ignore carry-forward have no generation or rollbackId.
  for (const state of [repair, approved]) {
    for (const decision of state.reviewDecisions ?? []) {
      delete decision.generation;
      delete decision.rollbackId;
    }
  }
  // A decision on code findings before UX review runs.
  const codeOnly = reviewed();
  delete codeOnly.uxReview;
  const codeStage = transition(codeOnly, "decide_review", {
    id,
    reviewFingerprint: codeReviewFingerprint(codeOnly),
    request: "Repair the code finding",
    repair: [{ role: "review_code", findingId: "same" }],
    ignore: [],
  });
  return {
    fixture: fixture(),
    reviewed: reviewed(),
    repair,
    approved,
    codeStage,
  };
}
// Golden digests recorded on the code before contract 3. validateAssignment
// recomputes packetDigest for every open or accepted contract-2/3 assignment,
// so any change to reviewPacket(), canonical() or evidenceDigest() output
// fences in-flight tasks and accepted receipts.
const GOLDEN_PACKET_DIGESTS: Record<string, string> = {
  fixture: "ff8ad4bfa2558e6c1b1128f7c8442d29f3d2e9e8b347e4519cc58ced741f9c4b",
  reviewed: "a06c449fa009d723fc8c981442cbaab08ae41d13dc36c2a9b2a76fde78d63b62",
  repair: "043a7bd4790e94bc78a6298adaac79db77b7c35eddbd803b7356136d436060e9",
  approved: "bf507d6479489206c3099850cdea93b4f7bb001c525522cd4ca039debe9bf289",
  // Recorded when code-stage decisions were introduced.
  codeStage: "954075ee92511dfda5928b91e5a903ada11649c1d9fbcfff7b8e5d0d8de65d6d",
};
Deno.test("reviewPacket output is byte-stable for bound assignments", () => {
  const states = goldenPacketStates();
  const actual = Object.fromEntries(
    Object.entries(states).map((
      [name, state],
    ) => [name, evidenceDigest(reviewPacket(state))]),
  );
  for (const [name, digest] of Object.entries(GOLDEN_PACKET_DIGESTS)) {
    assert(actual[name] === digest, `${name}: ${actual[name]} !== ${digest}`);
  }
  assert(
    Object.keys(actual).length === Object.keys(GOLDEN_PACKET_DIGESTS).length,
  );
});
