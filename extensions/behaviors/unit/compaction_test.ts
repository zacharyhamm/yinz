/** Review diffs out of line, preserved fingerprints and one-time compaction. */
import { Buffer } from "node:buffer";
import { join } from "node:path";
import {
  assignmentFileContents,
  assignmentManifest,
  type AssignmentSource,
  baselineView,
  FILE_NAMES,
  publishAssignmentFiles,
} from "../_lib/assignment_files.ts";
import { canonical, evidenceDigest } from "../_lib/canonical.ts";
import { handoffStateFields } from "../_lib/handoff_next.ts";
import { reviewPacket } from "../_lib/issue_evidence.ts";
import {
  activeFollowUps,
  carriedIgnores,
  codeReviewFingerprint,
  createState,
  implementationReviewFingerprint,
  type LifecycleState,
  pendingReviewFindings,
  planningGeneration,
  priorImplementationFindings,
  requireReviewAcceptance,
  StateSchema,
  transition,
} from "../_lib/lifecycle.ts";
import { GitBaseline, identityKey } from "../_lib/review_continuity.ts";
import {
  baselineBytes,
  compactBaseline,
  memoryDiffStore,
} from "../_lib/review_diffs.ts";
import { compactState, hasInlineDiffs } from "../_lib/state_compaction.ts";

function assert(value: unknown, message = "Assertion failed"): asserts value {
  if (!value) throw new Error(message);
}
function equal(actual: unknown, expected: unknown, message?: string) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  assert(a === e, message ?? `${a} !== ${e}`);
}
function throws(f: () => unknown, text: string) {
  try {
    f();
  } catch (error) {
    assert(String(error).includes(text), String(error));
    return;
  }
  throw new Error(`Expected ${text}`);
}

const issue = {
  workspace: "default",
  itemId: 144,
  text: "Compact lifecycle records",
  note: "Records stay small",
  digest: "d".repeat(64),
};
const main = "b".repeat(40);
const first = "a".repeat(40);
const repaired = "c".repeat(40);
const plan = {
  summary: "Repair",
  steps: [{ id: "fix", description: "Fix", files: ["lib.rs"] }],
  testingStrategy: "Focused",
  checks: [{ id: "focused", command: ["cargo", "x", "test"], covers: ["fix"] }],
};
/** A diff large enough that every embedded copy is visible in record size. */
function diffText(label: string, lines = 4000) {
  return `diff --git a/${label} b/${label}\n` +
    `+${label} line ✓\n`.repeat(lines);
}
const featureOne = diffText("first");
const featureTwo = diffText("second");
const delta = diffText("delta", 200);
function inlineBaseline(head: string, extra: Record<string, unknown> = {}) {
  return GitBaseline.parse({
    main,
    mergeBase: main,
    head,
    featureDiff: head === first ? featureOne : featureTwo,
    ancestry: "initial",
    baselineChanged: false,
    limitations: [],
    ...extra,
  });
}
const high = (id: string, extra: Record<string, unknown> = {}) => ({
  id,
  severity: "high" as const,
  description: `Finding ${id}`,
  affectedPaths: ["lib.rs"],
  evidence: ["lib.rs:1"],
  ...extra,
});

function implementing(): LifecycleState {
  let state = transition(createState(issue), "triage", {
    classification: "bug",
    summary: "Reproduced",
  });
  state = transition(state, "plan", plan);
  state = transition(state, "review_plan", { planVersion: 1, findings: [] });
  state = transition(state, "approve", { planVersion: 1, request: "Go" });
  return transition(state, "implement", {
    worktree: "/repo/issue",
    branch: "issue-144",
  });
}
function verified(state: LifecycleState): LifecycleState {
  const id = "c6da10b0-c980-4b82-b920-640238095f1a";
  return transition(state, "verify", {
    verification: {
      id,
      planVersion: 1,
      commit: state.codeReview!.commit,
      branch: "issue-144",
      startedAt: "2026-11-20T00:00:00.000Z",
      finishedAt: "2026-11-20T00:00:00.000Z",
      status: "passed",
      checks: plan.checks.map((check) => ({
        ...check,
        exitCode: 0,
        durationMs: 10,
        log: `check-log-${id}-${check.id}`,
        worktreeUnchanged: true,
        status: "passed",
      })),
    },
  });
}

/**
 * The shape issue 144 reached before diffs moved out of line: two code review
 * rounds and a UX review with inline baselines, a completed repair, ignored
 * findings, a low-severity follow-up, archived reviews and a passed verification.
 */
function legacyIssue(): LifecycleState {
  let state = transition(implementing(), "review_code", {
    planVersion: 1,
    commit: first,
    steps: [{ id: "fix", status: "implemented" }],
    findings: [high("race"), {
      id: "nit",
      severity: "low",
      description: "Naming",
    }],
    baseline: inlineBaseline(first),
  });
  const firstFingerprint = implementationReviewFingerprint(
    state,
    "review_code",
  );
  state = transition(state, "decide_review", {
    id: "a6da10b0-c980-4b82-b920-000000000001",
    reviewFingerprint: codeReviewFingerprint(state),
    request: "Repair the race",
    repair: [{ role: "review_code", findingId: "race" }],
    ignore: [],
  });
  state = transition(state, "implement_review_fixes", {
    summary: "Repaired",
    commit: repaired,
  });
  state = transition(state, "review_code", {
    planVersion: 1,
    commit: repaired,
    steps: [{ id: "fix", status: "implemented" }],
    findings: [
      high("race", {
        origin: {
          issue: "default/144",
          role: "review_code",
          reviewFingerprint: firstFingerprint,
          findingId: "race",
        },
      }),
      high("leak"),
    ],
    baseline: inlineBaseline(repaired, {
      previousHead: first,
      delta,
      deltaPaths: ["lib.rs"],
      ancestry: "descendant",
    }),
  });
  state = transition(state, "decide_review", {
    id: "a6da10b0-c980-4b82-b920-000000000002",
    reviewFingerprint: codeReviewFingerprint(state),
    request: "Accept both for now",
    repair: [],
    ignore: [
      { role: "review_code", findingId: "race" },
      { role: "review_code", findingId: "leak" },
    ],
  });
  state = transition(state, "review_ux", {
    planVersion: 1,
    commit: repaired,
    findings: [high("ux")],
    baseline: inlineBaseline(repaired, {
      previousHead: first,
      delta,
      deltaPaths: ["lib.rs"],
      ancestry: "descendant",
    }),
    assessment: {
      impact: "material",
      summary: "Visible",
      assessedPaths: ["lib.rs"],
      limitations: [],
    },
  });
  state = transition(state, "decide_review", {
    id: "a6da10b0-c980-4b82-b920-000000000003",
    reviewFingerprint: codeReviewFingerprint(state),
    request: "Accept the UX finding",
    repair: [],
    ignore: [{ role: "review_ux", findingId: "ux" }],
  });
  return verified(state);
}

Deno.test("legacy inline baselines parse; references allow exactly one form", () => {
  const inline = inlineBaseline(first);
  equal(GitBaseline.parse(inline), inline);
  const ref = { sha256: "e".repeat(64), bytes: 3, encoding: "utf8" };
  const { featureDiff: _body, ...identity } = inline;
  GitBaseline.parse({ ...identity, featureDiffRef: ref });
  throws(
    () => GitBaseline.parse({ ...inline, featureDiffRef: ref }),
    "either inline or by reference",
  );
  throws(() => GitBaseline.parse(identity), "either inline or by reference");
  throws(
    () => GitBaseline.parse({ ...identity, featureDiffRef: ref, delta: "x" }),
    "keeps no inline diff fields",
  );
  throws(
    () => GitBaseline.parse({ ...inline, deltaRef: ref }),
    "only alongside its feature diff",
  );
});

Deno.test("compaction keeps every fingerprint, decision and finding identity", async () => {
  const legacy = legacyIssue();
  const legacyJson = JSON.stringify(legacy);
  assert(hasInlineDiffs(legacy));
  requireReviewAcceptance(legacy);
  const store = memoryDiffStore();
  const { state, changed } = await compactState(legacy, store);
  assert(changed);
  assert(!hasInlineDiffs(state));
  // Each distinct body is stored once, however many records embedded it.
  equal(store.files.size, 3);
  const json = JSON.stringify(state);
  assert(!json.includes("first line"), "feature diff body left inline");
  assert(json.length * 20 < legacyJson.length, `${json.length} bytes`);
  // The same values legacy readers computed, now through recorded aliases.
  equal(codeReviewFingerprint(state), codeReviewFingerprint(legacy));
  equal(
    codeReviewFingerprint({ ...state, uxReview: undefined }),
    codeReviewFingerprint({ ...legacy, uxReview: undefined }),
  );
  for (const role of ["review_code", "review_ux"] as const) {
    equal(
      implementationReviewFingerprint(state, role),
      implementationReviewFingerprint(legacy, role),
    );
  }
  state.reviewDecisions!.forEach((decision, index) => {
    const old = legacy.reviewDecisions![index];
    equal(decision.reviewFingerprint, old.reviewFingerprint);
    equal(
      implementationReviewFingerprint(
        state,
        "review_code",
        decision.codeReview,
      ),
      implementationReviewFingerprint(legacy, "review_code", old.codeReview),
    );
  });
  const facts = (s: LifecycleState) => ({
    prior: priorImplementationFindings(s).map((r) => [
      identityKey(r.ref),
      r.fixed,
      r.sourceReview,
    ]),
    pending: pendingReviewFindings(s),
    carried: carriedIgnores(s),
    followUps: activeFollowUps(s).map((f) => identityKey(f.finding)),
  });
  equal(facts(state), facts(legacy));
  equal(facts(state).followUps.length, 1);
  equal(state.verification, legacy.verification);
  equal(state.reviewFollowUps, legacy.reviewFollowUps);
  equal(state.reviewHistory!.length, legacy.reviewHistory!.length);
  // Without the recorded fingerprints the user's ignores would stop applying.
  const unaliased = { ...state, fingerprintAliases: undefined };
  assert(codeReviewFingerprint(unaliased) !== codeReviewFingerprint(legacy));
  throws(
    () => requireReviewAcceptance(unaliased),
    "obtain the user's decision",
  );
  // Ignored findings still need no decision, so merge authorization proceeds.
  requireReviewAcceptance(state);
  const ready = transition(state, "ready", {
    commit: repaired,
    mergeRequest: "Merge it",
  });
  equal(ready.mergeAuthorization?.commit, repaired);
  // A reload sees the same state and fingerprints.
  const reloaded = StateSchema.parse(JSON.parse(json));
  equal(codeReviewFingerprint(reloaded), codeReviewFingerprint(legacy));
});

Deno.test("compaction is idempotent and stores each body once", async () => {
  const store = memoryDiffStore();
  const once = (await compactState(legacyIssue(), store)).state;
  const files = new Map(store.files);
  const twice = await compactState(once, store);
  assert(!twice.changed);
  equal(canonical(twice.state), canonical(once));
  equal([...store.files.keys()], [...files.keys()]);
  // A fresh compaction of the same legacy state is deterministic.
  const again = (await compactState(legacyIssue(), memoryDiffStore())).state;
  equal(evidenceDigest(again), evidenceDigest(once));
});

Deno.test("reviews recorded after compaction fingerprint their compact form stably", async () => {
  const store = memoryDiffStore();
  const state = (await compactState(legacyIssue(), store)).state;
  const baseline = await compactBaseline(store, inlineBaseline(repaired));
  const next = transition(
    transition(state, "implement", {
      worktree: "/repo/issue",
      branch: "issue-144",
    }),
    "review_code",
    {
      planVersion: 1,
      commit: repaired,
      steps: [{ id: "fix", status: "implemented" }],
      findings: [],
      baseline,
    },
  );
  const reloaded = StateSchema.parse(JSON.parse(JSON.stringify(next)));
  equal(
    implementationReviewFingerprint(reloaded, "review_code"),
    implementationReviewFingerprint(next, "review_code"),
  );
  // The archived legacy reviews keep their recorded fingerprints.
  equal(
    reloaded.reviewHistory!.map((h) => h.fingerprint),
    next.reviewHistory!.map((h) => h.fingerprint),
  );
});

function source(
  state: LifecycleState,
  baseline: GitBaseline,
): AssignmentSource {
  const id = "a6da10b0-c980-4b82-b920-000000000009";
  return {
    id,
    role: "review_code",
    before: state,
    reviewCommit: baseline.head,
    submission: {
      protocol: 1,
      behaviorName: `issue-evidence-${id}`,
      controlDir: "/repo/control",
      assignmentDigest: "f".repeat(64),
    },
    continuity: {
      version: 1,
      assignmentId: id,
      issueDigest: state.issue.digest,
      generation: planningGeneration(state),
      rollbackId: state.rollbackId,
      planVersion: state.planVersion,
      packetDigest: evidenceDigest(reviewPacket(state)),
      reviewFingerprints: [],
      baseline,
    },
  };
}

Deno.test("assignment files receive the exact diff bytes from references (utf8 and base64)", async () => {
  const binary = new Uint8Array([0xff, 0xfe, 0x00, 0x80, 0x0a, 0x61]);
  const cases = [
    { feature: featureOne, featureEncoding: undefined, endpoint: delta },
    {
      feature: Buffer.from(binary).toString("base64"),
      featureEncoding: "base64" as const,
      endpoint: Buffer.from(binary.toReversed()).toString("base64"),
    },
  ];
  for (const { feature, featureEncoding, endpoint } of cases) {
    const inline = GitBaseline.parse({
      main,
      mergeBase: main,
      head: repaired,
      featureDiff: feature,
      ...(featureEncoding ? { featureDiffEncoding: featureEncoding } : {}),
      previousHead: first,
      delta: endpoint,
      ...(featureEncoding ? { deltaEncoding: featureEncoding } : {}),
      deltaPaths: ["lib.rs"],
      ancestry: "descendant",
      baselineChanged: false,
      limitations: [],
    });
    const store = memoryDiffStore();
    const ref = await compactBaseline(store, inline);
    equal(ref.featureDiffRef?.encoding, featureEncoding ?? "utf8");
    equal(canonical(baselineView(ref)), canonical(baselineView(inline)));
    const state = implementing();
    const expected = assignmentFileContents(source(state, inline));
    const resolved = await baselineBytes(store, ref);
    const actual = assignmentFileContents(source(state, ref), resolved);
    equal([...actual.keys()].sort(), [...expected.keys()].sort());
    for (const [key, bytes] of expected) {
      if (key === "packet" || key === "snapshot") continue;
      equal(
        Buffer.from(actual.get(key)!).toString("hex"),
        Buffer.from(bytes).toString("hex"),
        key,
      );
    }
    const decode = (text: string) =>
      featureEncoding
        ? new Uint8Array(Buffer.from(text, "base64"))
        : new TextEncoder().encode(text);
    equal([...actual.get("featureDiff")!], [...decode(feature)]);
    equal([...actual.get("delta")!], [...decode(endpoint)]);
    throws(
      () => assignmentFileContents(source(state, ref)),
      "resolve them before writing",
    );
    throws(
      () =>
        assignmentFileContents(source(state, ref), {
          featureDiff: resolved.delta!,
          delta: resolved.featureDiff,
        }),
      "do not match their baseline digests",
    );
    // Publication regenerates missing files by digest from the store.
    const root = await Deno.makeTempDir({ prefix: "yinz-compact-files-" });
    try {
      const task = source(state, ref);
      const directory = join(root, task.id);
      const assignmentFiles = assignmentManifest(directory, actual);
      await publishAssignmentFiles({ ...task, assignmentFiles }, store);
      equal(
        [...await Deno.readFile(join(directory, FILE_NAMES.featureDiff))],
        [...decode(feature)],
      );
      equal(
        [...await Deno.readFile(join(directory, FILE_NAMES.delta))],
        [...decode(endpoint)],
      );
    } finally {
      await Deno.remove(root, { recursive: true });
    }
  }
});

Deno.test("records stay small as review rounds accumulate", async () => {
  const store = memoryDiffStore();
  const mebibyte = diffText("round", 75_000);
  assert(mebibyte.length >= 1 << 20);
  let state = implementing();
  let commit = first;
  const rounds = 8;
  for (let round = 0; round < rounds; round++) {
    const baseline = await compactBaseline(
      store,
      GitBaseline.parse({
        main,
        mergeBase: main,
        head: commit,
        featureDiff: `${mebibyte}round ${round}\n`,
        ancestry: "initial",
        baselineChanged: false,
        limitations: [],
      }),
    );
    const review = {
      planVersion: 1,
      commit,
      findings: [high(`finding-${round}`)],
      baseline,
    };
    state = transition(state, "review_code", {
      ...review,
      steps: [{ id: "fix", status: "implemented" }],
    });
    state = transition(state, "review_ux", {
      ...review,
      assessment: {
        impact: "material",
        summary: "Visible",
        assessedPaths: ["lib.rs"],
        limitations: [],
      },
    });
    state = transition(state, "decide_review", {
      id: `a6da10b0-c980-4b82-b920-${String(round).padStart(12, "0")}`,
      reviewFingerprint: codeReviewFingerprint(state),
      request: "Repair",
      repair: [{ role: "review_code", findingId: `finding-${round}` }],
      ignore: [{ role: "review_ux", findingId: `finding-${round}` }],
    });
    commit = (round + 1).toString(16).padStart(40, "0");
    state = transition(state, "implement_review_fixes", {
      summary: "Repaired",
      commit,
    });
  }
  const size = JSON.stringify(state).length;
  equal(state.reviewDecisions!.length, rounds);
  assert(state.reviewHistory!.length >= rounds);
  // Eight MiB of distinct diffs, referenced from dozens of places.
  assert(size < 128 * 1024, `state is ${size} bytes`);
  const handoff = JSON.stringify(handoffStateFields(state));
  assert(handoff.length < 1024, `handoff state fields are ${handoff.length}`);
});
