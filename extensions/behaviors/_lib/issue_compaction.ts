/**
 * One-time compaction of an issue saved before review diffs and lifecycle
 * snapshots moved out of its records. It reads each large record alone, never
 * reads the handoff, and writes only once every precondition holds.
 */
import { z } from "npm:zod@4.6.7";
import { handoffStateFields } from "./handoff_next.ts";
import {
  AgentTaskSchema,
  type IssueAgentContext,
  RecoverySchema,
  saveSnapshot,
  taskBeforeDigest,
} from "./issue_agents.ts";
import { DriverSchema } from "./issue_driver.ts";
import { type LifecycleState, stateDigest } from "./lifecycle.ts";
import { type Handle, loadState, saveState } from "./runtime.ts";
import { isInlineBaseline } from "./review_continuity.ts";
import {
  compactBaseline,
  type DiffStore,
  swampDiffStore,
} from "./review_diffs.ts";
import { compactState } from "./state_compaction.ts";

type Task = z.infer<typeof AgentTaskSchema>;

export interface CompactionDependencies {
  id: () => string;
  now: () => string;
  diffStore?: (context: IssueAgentContext) => DiffStore;
}

/**
 * A resolved task without inline lifecycle states or diff bodies: each state
 * becomes a digest of its compacted form, saved as a snapshot, so fencing
 * against the compacted lifecycle state keeps its meaning.
 */
export async function compactTask(
  context: Pick<IssueAgentContext, "writeResource">,
  store: DiffStore,
  task: Task,
): Promise<{ task: Task; changed: boolean }> {
  const { before, next, ...rest } = task;
  let changed = false;
  const out: Task = { ...rest } as Task;
  const snapshot = async (state: LifecycleState) =>
    saveSnapshot(context, (await compactState(state, store)).state);
  if (before) {
    out.beforeDigest = await snapshot(before);
    changed = true;
  } else out.beforeDigest = task.beforeDigest;
  if (next) {
    out.nextDigest = await snapshot(next);
    changed = true;
  }
  if (task.evidence) {
    const { next: receiptNext, ...receipt } = task.evidence;
    const evidence = structuredClone(receipt) as NonNullable<Task["evidence"]>;
    if (receiptNext) {
      evidence.nextDigest = stateDigest(
        (await compactState(receiptNext, store)).state,
      );
      changed = true;
    }
    const payload = evidence.payload;
    if (
      (payload.role === "review_code" || payload.role === "review_ux") &&
      payload.evidence.baseline && isInlineBaseline(payload.evidence.baseline)
    ) {
      payload.evidence.baseline = await compactBaseline(
        store,
        payload.evidence.baseline,
      );
      changed = true;
    }
    out.evidence = evidence;
  }
  if (task.continuity?.baseline && isInlineBaseline(task.continuity.baseline)) {
    out.continuity = {
      ...task.continuity,
      baseline: await compactBaseline(store, task.continuity.baseline),
    };
    changed = true;
  }
  return { task: AgentTaskSchema.parse(out), changed };
}

const unresolved = (task: Task) =>
  !["completed", "discarded", "superseded"].includes(task.status) ||
  task.cleanup === "pending" || !!task.discard && task.status !== "discarded";

/**
 * Brings an issue's state, current task and driver to the compact record shape.
 * Idempotent: a compact issue is left untouched. The caller owns the issue lock.
 */
export async function compactIssue(
  context: IssueAgentContext,
  dependencies: CompactionDependencies,
): Promise<{ dataHandles: Handle[] }> {
  const store = (dependencies.diffStore ?? swampDiffStore)(context);
  const rawRecovery = await context.readResource("recovery");
  if (
    rawRecovery != null &&
    RecoverySchema.parse(rawRecovery).status === "pending"
  ) {
    throw new Error(
      "Rollback is incomplete; finish it with resume-rollback before compacting",
    );
  }
  // Each large record is read, compacted and released before the next.
  let compactedTask: Task | undefined;
  let taskChanged = false;
  let taskId: string | undefined;
  {
    const raw = await context.readResource("agent-task");
    if (raw != null) {
      const task = AgentTaskSchema.parse(raw);
      if (unresolved(task)) {
        throw new Error(
          `Pane task ${task.id} (${task.role}) is ${task.status}${
            task.cleanup === "pending" ? " with cleanup pending" : ""
          }; resolve it with drive or resume-agent before compacting`,
        );
      }
      taskId = task.id;
      ({ task: compactedTask, changed: taskChanged } = await compactTask(
        context,
        store,
        task,
      ));
    }
  }
  let legacyDigest: string;
  let state: LifecycleState;
  let stateChanged: boolean;
  {
    const legacy = await loadState(context);
    legacyDigest = stateDigest(legacy);
    ({ state, changed: stateChanged } = await compactState(legacy, store));
  }
  let compactedDriver: z.infer<typeof DriverSchema> | undefined;
  let driverChanged = false;
  {
    const raw = await context.readResource("driver");
    if (raw != null) {
      const driver = DriverSchema.parse(raw);
      for (
        const reserved of [
          driver.automaticRevision,
          driver.revisionAuthorization,
        ]
      ) {
        if (
          reserved && reserved.assignment.id !== taskId &&
          reserved.previousAssignmentId === taskId &&
          taskBeforeDigest(reserved.assignment) === legacyDigest
        ) {
          throw new Error(
            "A reserved pane assignment is unresolved; resume drive before compacting",
          );
        }
      }
      const compact = async (task: Task) => {
        const result = await compactTask(context, store, task);
        driverChanged ||= result.changed;
        return result.task;
      };
      compactedDriver = DriverSchema.parse({
        ...driver,
        ...(driver.implementation
          ? { implementation: await compact(driver.implementation) }
          : {}),
        ...(driver.automaticRevision
          ? {
            automaticRevision: {
              ...driver.automaticRevision,
              assignment: await compact(driver.automaticRevision.assignment),
            },
          }
          : {}),
        ...(driver.revisionAuthorization
          ? {
            revisionAuthorization: {
              ...driver.revisionAuthorization,
              assignment: await compact(
                driver.revisionAuthorization.assignment,
              ),
            },
          }
          : {}),
      });
    }
  }
  if (!stateChanged && !taskChanged && !driverChanged) {
    context.logger.info("Issue {issue} records are already compact", {
      issue: context.definition.name,
    });
    return { dataHandles: [] };
  }
  const handles: Handle[] = [];
  // Replace the stale handoff unread: it may embed many copies of the old
  // state. Every later write reads only this small invalidated record.
  const reason =
    "Issue records were compacted; this handoff is superseded. Run drive for the current handoff.";
  handles.push(
    await context.writeResource("handoff", "handoff", {
      id: dependencies.id(),
      status: "invalidated",
      decision: "blocked",
      reason,
      createdAt: dependencies.now(),
      ...handoffStateFields(state),
      presentation: reason,
    }),
  );
  if (stateChanged) {
    handles.push(...(await saveState(context, "compact", state)).dataHandles);
  }
  if (taskChanged && compactedTask) {
    handles.push(
      await context.writeResource("agent-task", "agent-task", compactedTask),
    );
  }
  if (driverChanged && compactedDriver) {
    handles.push(
      await context.writeResource("driver", "driver", compactedDriver),
    );
  }
  context.logger.info(
    "Compacted issue {issue}: state {state}, task {task}, driver {driver}",
    {
      issue: context.definition.name,
      state: stateChanged ? "compacted" : "unchanged",
      task: taskChanged ? "compacted" : "unchanged",
      driver: driverChanged ? "compacted" : "unchanged",
    },
  );
  return { dataHandles: handles };
}
