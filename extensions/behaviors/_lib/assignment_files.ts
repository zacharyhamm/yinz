/**
 * Contract-3 assignment files: a private, read-only copy of everything a worker
 * reads for its task, derived only from the saved task. They live outside every
 * worktree under the Git common directory, and the prompt binds their manifest.
 */
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { z } from "npm:zod@4.6.7";
import {
  initialPlanning,
  type LifecycleState,
  previousPlanFindings,
  priorFindingObligations,
  RevisionPlanSchema,
  selectedRepairFindings,
} from "./lifecycle.ts";
import {
  type AssignmentTask,
  canonical,
  type EvidenceTargetSchema,
  reviewPacket,
  type RoleSchema,
  SubmitRoleSchemas,
} from "./issue_evidence.ts";

const Digest = z.string().regex(/^[0-9a-f]{64}$/);
const File = z.object({
  path: z.string().min(1),
  sha256: Digest,
  bytes: z.number().int().nonnegative(),
}).strict();
export const AssignmentFilesSchema = z.object({
  version: z.literal(1),
  directory: z.string().min(1),
  files: z.object({
    target: File,
    snapshot: File,
    packet: File,
    template: File,
    schema: File,
    baseline: File.optional(),
    featureDiff: File.optional(),
    delta: File.optional(),
  }).strict(),
}).strict();
export type AssignmentFiles = z.infer<typeof AssignmentFilesSchema>;
export type AssignmentFileKey = keyof AssignmentFiles["files"];
export const FILE_NAMES = {
  target: "target.json",
  snapshot: "snapshot.json",
  packet: "packet.json",
  template: "evidence-template.json",
  schema: "evidence-schema.json",
  baseline: "baseline.json",
  featureDiff: "feature.diff",
  delta: "delta.diff",
} as const satisfies Record<AssignmentFileKey, string>;
const MANIFEST = "manifest.json";

type Role = z.infer<typeof RoleSchema>;
export type AssignmentSource =
  & Pick<
    AssignmentTask,
    "id" | "role" | "before" | "reviewCommit" | "continuity"
  >
  & { submission: z.infer<typeof EvidenceTargetSchema> };

const encoder = new TextEncoder();
function sha256(bytes: Uint8Array) {
  return createHash("sha256").update(bytes).digest("hex");
}
function diffBytes(text: string, encoding?: "utf8" | "base64") {
  return encoding === "base64"
    ? new Uint8Array(Buffer.from(text, "base64"))
    : encoder.encode(text);
}
function pretty(value: unknown) {
  return encoder.encode(`${JSON.stringify(value, null, 2)}\n`);
}

/** The worker-facing baseline: diff bodies are replaced by their decoded files. */
export function baselineView(
  baseline: NonNullable<
    NonNullable<AssignmentSource["continuity"]>["baseline"]
  >,
) {
  const {
    featureDiff,
    featureDiffEncoding,
    delta,
    deltaEncoding,
    ...rest
  } = baseline;
  const feature = diffBytes(featureDiff, featureDiffEncoding);
  const endpoint = delta === undefined
    ? undefined
    : diffBytes(delta, deltaEncoding);
  return {
    ...rest,
    featureDiff: {
      file: FILE_NAMES.featureDiff,
      sha256: sha256(feature),
      bytes: feature.length,
      sourceEncoding: featureDiffEncoding ?? "utf8",
    },
    ...(endpoint
      ? {
        delta: {
          file: FILE_NAMES.delta,
          sha256: sha256(endpoint),
          bytes: endpoint.length,
          sourceEncoding: deltaEncoding ?? "utf8",
        },
      }
      : {}),
  };
}

function planSkeleton() {
  return {
    summary: "",
    steps: [{ id: "", description: "", files: [] }],
    testingStrategy: "",
    checks: [{ id: "", command: [], covers: [] }],
  };
}

/** Placeholders ("" and []) fail validation; pre-filled identities are exact. */
export function evidenceTemplate(
  task: Pick<AssignmentSource, "role" | "before" | "reviewCommit">,
): { role: Role; evidence: Record<string, unknown> } {
  const state: LifecycleState = task.before;
  const plan = state.plan;
  const prior = (role: "review_code" | "review_ux") =>
    priorFindingObligations(state, role).map((p) => ({
      finding: p.ref,
      outcome: "",
      rationale: "",
      evidence: [],
    }));
  const evidence = (): Record<string, unknown> => {
    switch (task.role) {
      case "triage":
        return { classification: "", summary: "" };
      case "plan": {
        if (initialPlanning(state) || !plan) return planSkeleton();
        return {
          summary: plan.summary,
          steps: plan.steps,
          testingStrategy: plan.testingStrategy,
          ...(plan.assumptions ? { assumptions: plan.assumptions } : {}),
          checks: plan.checks ?? planSkeleton().checks,
          revision: {
            summary: "",
            responses: previousPlanFindings(state).filter((f) =>
              ["high", "critical"].includes(f.severity) && !f.resolution
            ).map((f) => ({
              findingId: f.id,
              status: "",
              rationale: "",
              evidence: [],
            })),
            acceptedDecisions: [],
          },
        };
      }
      case "review_plan":
        return { planVersion: plan?.version, findings: [] };
      case "implement":
        return { summary: "", commit: "" };
      case "review_code":
        return {
          planVersion: plan?.version,
          commit: task.reviewCommit,
          steps: (plan?.steps ?? []).map((s) => ({ id: s.id, status: "" })),
          findings: [],
          priorFindings: prior("review_code"),
        };
      case "review_ux":
        return {
          planVersion: plan?.version,
          commit: task.reviewCommit,
          findings: [],
          priorFindings: prior("review_ux"),
          assessment: {
            impact: "",
            summary: "",
            assessedPaths: [],
            limitations: [],
          },
        };
      case "plan_review_fixes":
        return {
          ...planSkeleton(),
          findingMappings: selectedRepairFindings(state).map((p) => ({
            finding: p.ref,
            steps: [],
            checks: [],
          })),
        };
      case "implement_review_fixes":
        return {
          summary: "",
          commit: "",
          responses: selectedRepairFindings(state).map((p) => ({
            finding: p.ref,
            status: "",
            rationale: "",
            affectedPaths: [],
            evidence: [],
            checkIds: [],
          })),
        };
    }
  };
  // JSON drops undefined values, so a missing assigned value is simply omitted.
  return JSON.parse(JSON.stringify({ role: task.role, evidence: evidence() }));
}

/** Exact file bytes, derived deterministically from the saved task. */
export function assignmentFileContents(
  task: AssignmentSource,
): Map<AssignmentFileKey, Uint8Array> {
  const continuity = task.continuity;
  if (!continuity) {
    throw new Error("Assignment files require a bound continuity contract");
  }
  const contents = new Map<AssignmentFileKey, Uint8Array>();
  contents.set(
    "target",
    encoder.encode(
      canonical({ ...task.submission, taskId: task.id, role: task.role }),
    ),
  );
  contents.set("snapshot", encoder.encode(canonical(task.before)));
  const packet = canonical(reviewPacket(task.before));
  // evidenceDigest is sha256(canonical(...)), so these bytes hash to packetDigest.
  if (sha256(encoder.encode(packet)) !== continuity.packetDigest) {
    throw new Error(
      "Assignment packet does not match its bound continuity packetDigest",
    );
  }
  contents.set("packet", encoder.encode(packet));
  contents.set("template", pretty(evidenceTemplate(task)));
  contents.set(
    "schema",
    pretty(z.toJSONSchema(
      task.role === "plan" && !initialPlanning(task.before)
        ? RevisionPlanSchema
        : SubmitRoleSchemas[task.role],
    )),
  );
  const baseline = continuity.baseline;
  if (baseline) {
    contents.set("baseline", encoder.encode(canonical(baselineView(baseline))));
    contents.set(
      "featureDiff",
      diffBytes(baseline.featureDiff, baseline.featureDiffEncoding),
    );
    if (baseline.delta !== undefined) {
      contents.set("delta", diffBytes(baseline.delta, baseline.deltaEncoding));
    }
  }
  return contents;
}

export function assignmentManifest(
  directory: string,
  contents: Map<AssignmentFileKey, Uint8Array>,
): AssignmentFiles {
  const files = Object.fromEntries(
    [...contents].map(([key, bytes]) => [key, {
      path: join(directory, FILE_NAMES[key]),
      sha256: sha256(bytes),
      bytes: bytes.length,
    }]),
  );
  return AssignmentFilesSchema.parse({ version: 1, directory, files });
}

/** The manifest written beside the files; the prompt binds its sha256. */
export function manifestBytes(files: AssignmentFiles) {
  return encoder.encode(canonical(files));
}

type Expected = { sha256: string; bytes: number };

async function verifyPublished(
  path: string,
  name: string,
  uid: number,
  expected: Expected,
) {
  const differs = () =>
    new Error(
      `Assignment file ${name} differs from the saved assignment; refusing delivery`,
    );
  const info = await Deno.lstat(path);
  if (
    !info.isFile || info.uid !== uid || info.mode === null ||
    (info.mode & 0o777) !== 0o400 || info.size !== expected.bytes
  ) throw differs();
  const bytes = await Deno.readFile(path);
  if (bytes.length !== expected.bytes || sha256(bytes) !== expected.sha256) {
    throw differs();
  }
}

/** Publishes one file atomically, or verifies the file already there. */
async function publishFile(
  directory: string,
  uid: number,
  name: string,
  expected: Expected,
  produce: () => Uint8Array | undefined,
) {
  const path = join(directory, name);
  let exists = true;
  try {
    await Deno.lstat(path);
  } catch (error) {
    if (!(error instanceof Deno.errors.NotFound)) throw error;
    exists = false;
  }
  if (exists) return await verifyPublished(path, name, uid, expected);
  const unreproducible = (cause?: string) =>
    new Error(
      `Assignment file ${name} is missing and cannot be reproduced byte-for-byte; inspect, then explicitly restart the worker${
        cause ? ` (${cause})` : ""
      }`,
    );
  let bytes: Uint8Array | undefined;
  try {
    bytes = produce();
  } catch (error) {
    throw unreproducible(
      error instanceof Error ? error.message : String(error),
    );
  }
  if (
    !bytes || bytes.length !== expected.bytes ||
    sha256(bytes) !== expected.sha256
  ) throw unreproducible();
  const temporary = join(directory, `.publishing-${crypto.randomUUID()}`);
  const file = await Deno.open(temporary, {
    write: true,
    createNew: true,
    mode: 0o600,
  });
  try {
    try {
      if ((await file.stat()).uid !== uid) {
        throw new Error("Assignment directory must be owned locally");
      }
      let offset = 0;
      while (offset < bytes.length) {
        offset += await file.write(bytes.subarray(offset));
      }
      await Deno.chmod(temporary, 0o400);
      await file.sync();
    } finally {
      file.close();
    }
    try {
      // Hard-link publication is atomic and fails if the final name exists.
      await Deno.link(temporary, path);
    } catch (error) {
      if (!(error instanceof Deno.errors.AlreadyExists)) throw error;
    }
    const published = await Deno.open(directory, { read: true });
    try {
      await published.sync();
    } finally {
      published.close();
    }
    await verifyPublished(path, name, uid, expected);
  } finally {
    await Deno.remove(temporary);
  }
}

/**
 * Publishes the saved task's assignment files before any prompt is delivered.
 * Idempotent: existing files must match the manifest exactly, and a missing file
 * is regenerated only when it reproduces byte for byte (otherwise fail closed).
 */
export async function publishAssignmentFiles(
  task: { assignmentFiles: AssignmentFiles } & AssignmentSource,
): Promise<void> {
  const files = AssignmentFilesSchema.parse(task.assignmentFiles);
  const directory = files.directory;
  await Deno.mkdir(dirname(directory), { recursive: true });
  try {
    await Deno.mkdir(directory, { mode: 0o700 });
  } catch (error) {
    if (!(error instanceof Deno.errors.AlreadyExists)) throw error;
  }
  const info = await Deno.lstat(directory);
  if (
    !info.isDirectory || info.uid === null || info.mode === null ||
    (info.mode & 0o777) !== 0o700
  ) {
    throw new Error("Assignment directory must be private and owned locally");
  }
  let contents: Map<AssignmentFileKey, Uint8Array> | undefined;
  for (
    const [key, file] of Object.entries(files.files) as [
      AssignmentFileKey,
      AssignmentFiles["files"][AssignmentFileKey],
    ][]
  ) {
    if (!file) continue;
    const name = FILE_NAMES[key];
    if (file.path !== join(directory, name)) {
      throw new Error(
        `Assignment file ${name} is outside its assignment directory; refusing delivery`,
      );
    }
    await publishFile(directory, info.uid, name, file, () => {
      contents ??= assignmentFileContents(task);
      return contents.get(key);
    });
  }
  const manifest = manifestBytes(files);
  await publishFile(directory, info.uid, MANIFEST, {
    sha256: sha256(manifest),
    bytes: manifest.length,
  }, () => manifest);
}
