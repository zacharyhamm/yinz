/** Optional persisted contracts; version two assignments enforce their completeness. */
import { z } from "npm:zod@4.6.7";

const Text = z.string().trim().min(1);
export const ObjectId = z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/);
const Digest = z.string().regex(/^[0-9a-f]{64}$/);
export const ReviewRole = z.enum(["review_code", "review_ux"]);
export const FindingIdentity = z.object({
  issue: Text,
  role: ReviewRole,
  reviewFingerprint: Digest,
  findingId: Text,
}).strict();
export const RepairMapping = z.object({
  finding: FindingIdentity,
  steps: z.array(Text).min(1),
  checks: z.array(Text).min(1),
}).strict();
export const RepairResponse = z.object({
  finding: FindingIdentity,
  status: z.enum(["addressed", "disputed", "unresolved"]),
  rationale: Text,
  affectedPaths: z.array(Text).min(1),
  evidence: z.array(Text).min(1),
  checkIds: z.array(Text),
}).strict();
export const PriorFindingOutcome = z.object({
  finding: FindingIdentity,
  outcome: z.enum(["persists", "verified_fixed", "reopened"]),
  rationale: Text,
  evidence: z.array(Text).min(1),
}).strict();
export const GitBaseline = z.object({
  main: ObjectId,
  mergeBase: ObjectId,
  head: ObjectId,
  featureDiff: z.string(),
  featureDiffEncoding: z.enum(["utf8", "base64"]).optional(),
  previousHead: ObjectId.optional(),
  delta: z.string().optional(),
  deltaEncoding: z.enum(["utf8", "base64"]).optional(),
  ancestry: z.enum(["initial", "same", "descendant", "divergent", "missing"]),
  baselineChanged: z.boolean(),
  baselineComparison: z.enum(["initial", "same", "changed", "unknown"])
    .optional(),
  limitations: z.array(Text),
}).strict();
export const AdvisoryEvidence = z.object({
  producingTask: z.uuid(),
  commit: ObjectId,
  requirementDigest: Digest,
  argv: z.array(Text).min(1),
  cwd: Text,
  result: z.enum(["passed", "failed", "skipped", "flaky"]),
  outputRef: Text,
  relevantFiles: z.array(Text).min(1),
  tests: z.array(Text),
  platform: Text,
  toolchain: Text,
  prerequisites: z.array(Text),
  clean: z.boolean(),
  dependencyScopeKnown: z.boolean(),
  // A claim of reuse always retains the original producing task/commit/log.
  reuse: z.object({
    atCommit: ObjectId,
    rationale: Text,
    dependencyComparison: z.object({
      evidence: z.array(Text).min(1),
      unchanged: z.boolean(),
      compared: z.array(z.enum([
        "implementation",
        "callers",
        "tests",
        "fixtures",
        "dependencies",
        "build_configuration",
        "requirements",
        "environment",
      ])).min(8).max(8).refine((xs) => new Set(xs).size === 8),
    }).strict().optional(),
    objectsAvailable: z.boolean(),
    outputAvailable: z.boolean(),
    prerequisitesMatch: z.boolean(),
  }).strict().optional(),
}).strict();
export function validateAdvisory(
  evidence: z.infer<typeof AdvisoryEvidence>,
  requirementDigest: string,
  currentCommit: string,
) {
  if (!evidence.reuse) {
    if (
      evidence.commit !== currentCommit ||
      evidence.requirementDigest !== requirementDigest
    ) {
      throw new Error(
        "New advisory execution must name the current commit and requirements",
      );
    }
    return;
  }
  const reuse = evidence.reuse;
  if (
    reuse.atCommit !== currentCommit ||
    evidence.requirementDigest !== requirementDigest ||
    evidence.result !== "passed" || !evidence.clean ||
    !evidence.dependencyScopeKnown ||
    !reuse.objectsAvailable || !reuse.outputAvailable ||
    !reuse.prerequisitesMatch ||
    (evidence.commit !== currentCommit &&
      !reuse.dependencyComparison?.unchanged)
  ) {
    throw new Error(
      "Advisory passing reuse lacks applicable execution/dependency evidence",
    );
  }
}
export const RoundCause = z.object({
  kind: z.enum([
    "initial_work",
    "newly_discovered_defect",
    "incomplete_repair",
    "changed_requirements",
    "verification_merge_failure",
    "recovery",
    "other",
    "unknown",
  ]),
  explanation: Text,
  evidence: z.array(Text).min(1),
  source: z.enum(["controller", "worker", "user", "recovery"]),
  // Attribution of introduction requires comparison evidence, never chronology alone.
  introducedByRepair: z.object({
    assignmentId: z.uuid(),
    comparisonEvidence: z.array(Text).min(1),
  }).strict().optional(),
}).strict();
export const RoundRecord = z.object({
  assignmentId: z.uuid(),
  predecessor: z.uuid().optional(),
  role: Text,
  at: z.iso.datetime(),
  issueDigest: Digest,
  generation: Text,
  launchCauses: z.array(RoundCause).min(1),
  corrections: z.array(
    z.object({
      at: z.iso.datetime(),
      source: Text,
      replaces: z.number().int().nonnegative().optional(),
      cause: RoundCause,
    }).strict(),
  ),
}).strict();
export const ContinuityContext = z.object({
  version: z.literal(1),
  assignmentId: z.uuid(),
  issueDigest: Digest,
  generation: Text,
  rollbackId: z.uuid().optional(),
  planVersion: z.number().int().nonnegative(),
  // Digest of the complete packet, which is built from the bound before snapshot.
  packetDigest: Digest,
  reviewFingerprints: z.array(Digest),
  baseline: GitBaseline.optional(),
}).strict();
export const identityKey = (ref: z.infer<typeof FindingIdentity>) =>
  JSON.stringify([ref.issue, ref.role, ref.reviewFingerprint, ref.findingId]);
