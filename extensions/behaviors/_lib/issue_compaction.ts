/**
 * One-time compaction of an issue saved before review diffs and lifecycle
 * snapshots moved out of its records. It reads each large record once, never
 * reads the handoff, and writes records only once every precondition holds.
 */
import { z } from "npm:zod@4.6.7";
import { handoffStateFields } from "./handoff_next.ts";
import {
  AgentTaskSchema,
  type IssueAgentContext,
  loadSnapshot,
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
 * Lifecycle states renamed by one compaction: each old digest maps to the
 * digest of its compacted form, and `saved` lists compacted digests whose
 * snapshot exists.
 */
export type StateRenames = {
  digests: Map<string, string>;
  saved: Set<string>;
};
export function stateRenames(): StateRenames {
  return { digests: new Map(), saved: new Set() };
}

/**
 * A resolved task without inline lifecycle states or diff bodies: each state
 * becomes a digest of its compacted form, saved as a snapshot, so fencing
 * against the compacted lifecycle state keeps its meaning. A task that already
 * names its states by digest (dispatched after snapshots moved out, against a
 * state that still embedded diffs) is renamed the same way: its dispatch
 * snapshot is compacted and re-saved, and other digests take their name from
 * `renames`, which one compaction shares across its records.
 */
export async function compactTask(
  context: Pick<IssueAgentContext, "readResource" | "writeResource">,
  store: DiffStore,
  task: Task,
  renames: StateRenames = stateRenames(),
): Promise<{ task: Task; changed: boolean }> {
  const { before, next, ...rest } = task;
  let changed = false;
  const out: Task = { ...rest } as Task;
  const snapshot = async (state: LifecycleState) => {
    const digest = await saveSnapshot(
      context,
      (await compactState(state, store)).state,
    );
    renames.digests.set(stateDigest(state), digest);
    renames.saved.add(digest);
    return digest;
  };
  /** The compacted name of a digest-only state; `load` ensures its snapshot. */
  const rename = async (digest: string, load: boolean) => {
    const known = renames.digests.get(digest);
    if (!load || (known !== undefined && renames.saved.has(known))) {
      return known ?? digest;
    }
    // A missing or damaged snapshot cannot be compacted; fences still follow
    // a known rename.
    const saved = await loadSnapshot(context, digest).catch(() => undefined);
    if (!saved) return known ?? digest;
    const compacted = await compactState(saved, store);
    const result = compacted.changed
      ? await saveSnapshot(context, compacted.state)
      : digest;
    renames.digests.set(digest, result);
    renames.saved.add(result);
    return result;
  };
  if (before) {
    out.beforeDigest = await snapshot(before);
    changed = true;
  } else {
    out.beforeDigest = await rename(task.beforeDigest!, true);
    changed ||= out.beforeDigest !== task.beforeDigest;
  }
  if (next) {
    out.nextDigest = await snapshot(next);
    changed = true;
  } else if (task.nextDigest) {
    out.nextDigest = await rename(task.nextDigest, false);
    changed ||= out.nextDigest !== task.nextDigest;
  }
  if (task.evidence) {
    const { next: receiptNext, ...receipt } = task.evidence;
    const evidence = structuredClone(receipt) as NonNullable<Task["evidence"]>;
    if (receiptNext) {
      evidence.nextDigest = stateDigest(
        (await compactState(receiptNext, store)).state,
      );
      changed = true;
    } else if (evidence.nextDigest) {
      const digest = await rename(evidence.nextDigest, false);
      changed ||= digest !== evidence.nextDigest;
      evidence.nextDigest = digest;
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
 * Brings an issue's state, current task, driver and completed rollback record to
 * the compact record shape. Idempotent: a compact issue is left untouched. The
 * caller owns the issue lock.
 */
export async function compactIssue(
  context: IssueAgentContext,
  dependencies: CompactionDependencies,
): Promise<{ dataHandles: Handle[] }> {
  const store = (dependencies.diffStore ?? swampDiffStore)(context);
  const rawRecovery = await context.readResource("recovery");
  const recovery = rawRecovery == null
    ? undefined
    : RecoverySchema.parse(rawRecovery);
  if (recovery?.status === "pending") {
    throw new Error(
      "Rollback is incomplete; finish it with resume-rollback before compacting",
    );
  }
  let task: Task | undefined;
  {
    const raw = await context.readResource("agent-task");
    if (raw != null) {
      task = AgentTaskSchema.parse(raw);
      if (unresolved(task)) {
        throw new Error(
          `Pane task ${task.id} (${task.role}) is ${task.status}${
            task.cleanup === "pending" ? " with cleanup pending" : ""
          }; resolve it with drive or resume-agent before compacting`,
        );
      }
    }
  }
  const taskId = task?.id;
  // Records that name a state by digest keep naming the same, compacted state.
  const renamed = stateRenames();
  let legacyDigest: string;
  let state: LifecycleState;
  let stateChanged: boolean;
  {
    const legacy = await loadState(context);
    legacyDigest = stateDigest(legacy);
    ({ state, changed: stateChanged } = await compactState(legacy, store));
    renamed.digests.set(legacyDigest, stateDigest(state));
  }
  const rawDriver = await context.readResource("driver");
  const driver = rawDriver == null ? undefined : DriverSchema.parse(rawDriver);
  for (
    const reserved of [driver?.automaticRevision, driver?.revisionAuthorization]
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
  let compactedTask: Task | undefined;
  let taskChanged = false;
  if (task) {
    ({ task: compactedTask, changed: taskChanged } = await compactTask(
      context,
      store,
      task,
      renamed,
    ));
  }
  let compactedDriver: z.infer<typeof DriverSchema> | undefined;
  let driverChanged = false;
  if (driver) {
    const compact = async (value: Task) => {
      const result = await compactTask(context, store, value, renamed);
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
  // A finished rollback is history that status still reads with the other
  // records: its states and tasks keep their content with diffs by reference.
  let compactedRecovery: z.infer<typeof RecoverySchema> | undefined;
  let recoveryChanged = false;
  if (recovery) {
    const compactRecoveryState = async (value: LifecycleState) => {
      const result = await compactState(value, store);
      recoveryChanged ||= result.changed;
      return result.state;
    };
    const compactRecoveryTask = async (value: Task) => {
      const result = await compactTask(context, store, value, renamed);
      recoveryChanged ||= result.changed;
      return result.task;
    };
    compactedRecovery = RecoverySchema.parse({
      ...recovery,
      before: await compactRecoveryState(recovery.before),
      next: await compactRecoveryState(recovery.next),
      ...(recovery.reconciled
        ? { reconciled: await compactRecoveryState(recovery.reconciled) }
        : {}),
      ...(recovery.task
        ? { task: await compactRecoveryTask(recovery.task) }
        : {}),
      ...(recovery.replacement
        ? { replacement: await compactRecoveryTask(recovery.replacement) }
        : {}),
    });
  }
  if (!stateChanged && !taskChanged && !driverChanged && !recoveryChanged) {
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
  if (recoveryChanged && compactedRecovery) {
    handles.push(
      await context.writeResource("recovery", "recovery", compactedRecovery),
    );
  }
  context.logger.info(
    "Compacted issue {issue}: state {state}, task {task}, driver {driver}, recovery {recovery}",
    {
      issue: context.definition.name,
      state: stateChanged ? "compacted" : "unchanged",
      task: taskChanged ? "compacted" : "unchanged",
      driver: driverChanged ? "compacted" : "unchanged",
      recovery: recoveryChanged ? "compacted" : "unchanged",
    },
  );
  return { dataHandles: handles };
}
