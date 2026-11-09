/** Task-specific Swamp submission authority, independent of the waiting controller. */
import { z } from "npm:zod@4.6.7";
import { readAgenda, StateSchema } from "./_lib/lifecycle.ts";
import { type Context, inspect } from "./_lib/runtime.ts";
import {
  type Assignment,
  assignmentCore,
  AssignmentSchema,
  canonical,
  evidenceDigest,
  EvidenceReceiptSchema,
  EvidenceTargetSchema,
  SubmissionSchema,
  SubmitArgsSchema,
  validateAssignment,
  validateEvidence,
  validateReceipt,
} from "./_lib/issue_evidence.ts";
import { AgentConfigSchema, promptDigest } from "./_lib/yinz_agent.ts";

export interface EvidenceContext
  extends Pick<Context, "signal" | "createFileWriter"> {
  repoDir: string;
  definition: { name: string };
  readResource: Context["readResource"];
  writeResource: Context["writeResource"];
  dataRepository: {
    getContent: (
      type: string,
      name: string,
      resource: string,
    ) => Promise<Uint8Array | null>;
  };
}
export const evidenceDependencies = {
  readAgenda,
  inspect: (context: EvidenceContext, worktree: string) =>
    inspect(context, worktree),
  realPath: (path: string) => Deno.realPath(path),
  now: () => new Date().toISOString(),
};
const ParentTaskSchema = z.object({
  id: z.uuid(),
  behaviorName: z.string(),
  role: AssignmentSchema.shape.role,
  config: AgentConfigSchema,
  before: StateSchema,
  prompt: z.string().min(1),
  promptDigest: z.string(),
  request: z.string().optional(),
  reviewCommit: z.string().optional(),
  status: z.string(),
  submission: EvidenceTargetSchema.optional(),
  discard: z.unknown().optional(),
});
async function parentResource(
  context: EvidenceContext,
  assignment: Assignment,
  name: string,
) {
  // Read only. Acquiring the issue or child execution lock here deadlocks the
  // controller that is waiting for this worker to submit.
  const bytes = await context.dataRepository.getContent(
    "@yinz/issue-lifecycle",
    assignment.parentId,
    name,
  );
  return bytes ? JSON.parse(new TextDecoder().decode(bytes)) : null;
}
export function createEvidenceMethods(dependencies = evidenceDependencies) {
  // Helpers receive dependencies explicitly so unit tests never read real Agenda
  // or Git. The only writable state is this behavior's submission resource.
  const checkLocation = async (
    context: EvidenceContext,
    assignment: Assignment,
  ) => {
    validateAssignment(assignment);
    if (
      context.definition.name !== `issue-evidence-${assignment.taskId}` ||
      await dependencies.realPath(context.repoDir) !== assignment.controlDir
    ) {
      throw new Error(
        "Use the assignment's evidence behavior in its canonical control checkout",
      );
    }
  };
  const checkTask = async (
    context: EvidenceContext,
    assignment: Assignment,
  ) => {
    await checkLocation(context, assignment);
    return parentTaskWithIdentity(context, assignment);
  };
  return {
    open: {
      kind: "create" as const,
      rollbackOnFailure: true,
      description:
        "Open exactly the controller's durable task assignment before launch",
      arguments: z.object({ assignment: AssignmentSchema }).strict(),
      execute: async (
        { assignment }: { assignment: Assignment },
        context: EvidenceContext,
      ) => {
        await checkTask(context, assignment);
        const raw = await context.readResource("submission");
        if (raw != null) {
          const saved = SubmissionSchema.parse(raw);
          if (canonical(saved.assignment) !== canonical(assignment)) {
            throw new Error("Cannot replace an existing submission assignment");
          }
          return { dataHandles: [] };
        }
        return {
          dataHandles: [
            await context.writeResource(
              "submission",
              "submission",
              SubmissionSchema.parse({ status: "open", assignment }),
            ),
          ],
        };
      },
    },
    submit: {
      kind: "update" as const,
      rollbackOnFailure: true,
      description:
        "Validate role evidence and durably acknowledge this exact task; final chat text is never evidence",
      arguments: SubmitArgsSchema,
      execute: async (
        input: z.infer<typeof SubmitArgsSchema>,
        context: EvidenceContext,
      ) => {
        const args = SubmitArgsSchema.parse(input);
        const saved = SubmissionSchema.parse(
          await context.readResource("submission"),
        );
        const { assignment } = saved;
        await checkLocation(context, assignment);
        if (
          args.taskId !== assignment.taskId ||
          args.assignmentDigest !== assignment.digest
        ) {
          throw new Error(
            "Submission belongs to a stale or different assignment",
          );
        }
        const { taskId: _task, assignmentDigest: _digest, ...raw } = args;
        const validated = validateEvidence(assignment, raw);
        const digest = evidenceDigest(validated.payload);
        if (saved.status === "superseded") {
          throw new Error(
            "Assignment is superseded; its historical receipt cannot advance the lifecycle",
          );
        }
        if (saved.status === "accepted") {
          const receipt = validateReceipt(assignment, saved.receipt);
          if (receipt.payloadDigest !== digest) {
            throw new Error(
              "Submission conflict: this task already accepted different evidence",
            );
          }
          return { dataHandles: [] };
        }
        const task = await checkTask(context, assignment);
        const recovery = await parentResource(context, assignment, "recovery");
        if (
          task.status !== "running" || task.discard ||
          (recovery &&
            z.object({ status: z.string() }).parse(recovery).status ===
              "pending")
        ) {
          throw new Error(
            "Assignment is no longer accepting evidence; controller recovery is pending",
          );
        }
        const current = StateSchema.parse(
          await parentResource(context, assignment, "state"),
        );
        if (canonical(current) !== canonical(assignment.before)) {
          throw new Error("Lifecycle changed; submission assignment is stale");
        }
        const fresh = await dependencies.readAgenda(
          current.issue.workspace,
          current.issue.itemId,
        );
        if (fresh.digest !== current.issue.digest) {
          throw new Error(
            "Agenda requirements changed; submission assignment is stale",
          );
        }
        if (
          validated.payload.role === "implement" ||
          validated.payload.role === "review_code" ||
          validated.payload.role === "review_ux"
        ) {
          if (!current.implementation) {
            throw new Error(
              "Pane implementation has no recorded issue worktree",
            );
          }
          const repo = await dependencies.inspect(
            context,
            current.implementation.worktree,
          );
          if (
            !repo.clean || repo.branch !== current.implementation.branch ||
            repo.head !== validated.payload.evidence.commit
          ) {
            throw new Error(
              "Pane result does not match the clean issue branch's current commit",
            );
          }
        }
        const receipt = EvidenceReceiptSchema.parse({
          taskId: assignment.taskId,
          assignmentDigest: assignment.digest,
          payloadDigest: digest,
          acceptedAt: dependencies.now(),
          ...validated,
        });
        const handle = await context.writeResource(
          "submission",
          "submission",
          SubmissionSchema.parse({ status: "accepted", assignment, receipt }),
        );
        return { dataHandles: [handle] };
      },
    },
    supersede: {
      kind: "update" as const,
      rollbackOnFailure: true,
      description:
        "Fence submissions using a matching durable controller rollback or discard intent",
      arguments: z.object({
        taskId: z.uuid(),
        assignmentDigest: AssignmentSchema.shape.digest,
        operationId: z.uuid(),
      }).strict(),
      execute: async (
        args: { taskId: string; assignmentDigest: string; operationId: string },
        context: EvidenceContext,
      ) => {
        const saved = SubmissionSchema.parse(
          await context.readResource("submission"),
        );
        const { assignment } = saved;
        if (
          args.taskId !== assignment.taskId ||
          args.assignmentDigest !== assignment.digest
        ) throw new Error("Cannot supersede another assignment");
        if (saved.status === "superseded") {
          // A subsequent explicit rollback may supersede an interrupted one;
          // this assignment is already fenced regardless of the operation ID.
          return { dataHandles: [] };
        }
        const task = await checkTask(context, assignment);
        const raw = await parentResource(context, assignment, "recovery");
        const recovery = raw == null ? undefined : z.object({
          id: z.string(),
          status: z.string(),
          task: z.object({ id: z.string() }).optional(),
        }).parse(raw);
        if (
          !(task.discard && args.operationId === task.id) &&
          !(recovery?.status === "pending" &&
            recovery.id === args.operationId && recovery.task?.id === task.id)
        ) {
          throw new Error(
            "Supersession requires a durable controller recovery or discard intent",
          );
        }
        return {
          dataHandles: [
            await context.writeResource(
              "submission",
              "submission",
              SubmissionSchema.parse({
                status: "superseded",
                assignment,
                operationId: args.operationId,
                ...(saved.status === "accepted"
                  ? { receipt: saved.receipt }
                  : {}),
              }),
            ),
          ],
        };
      },
    },
  };
}

async function parentTaskWithIdentity(
  context: EvidenceContext,
  assignment: Assignment,
) {
  // Path checks use the injected dependency above; identity checks do not do IO
  // outside Swamp's resource API.
  const task = ParentTaskSchema.parse(
    await parentResource(context, assignment, "agent-task"),
  );
  const core = assignmentCore(
    {
      modelId: assignment.parentId,
      definition: { name: assignment.parentName },
    },
    task,
    assignment.controlDir,
  );
  if (
    evidenceDigest(core) !== assignment.digest ||
    task.promptDigest !== assignment.promptDigest ||
    await promptDigest(task.prompt) !== assignment.promptDigest ||
    task.submission?.behaviorName !== context.definition.name ||
    task.submission.assignmentDigest !== assignment.digest ||
    task.submission.controlDir !== assignment.controlDir
  ) {
    throw new Error("Submission belongs to a stale or different assignment");
  }
  return task;
}

export const behavior = {
  type: "@yinz/issue-evidence",
  version: "2026.11.08.1",
  globalArguments: z.object({}).strict(),
  resources: {
    submission: {
      description:
        "Immutable assignment and accepted role evidence, acknowledgement and supersession",
      schema: SubmissionSchema,
      lifetime: "infinite",
      garbageCollection: 1000,
    },
  },
  methods: createEvidenceMethods(),
};
