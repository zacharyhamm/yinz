/** Run the approved plan's checks at the reviewed commit and record the results. */
import { z } from "npm:zod@4.6.7";
import {
  BaselineCheckSchema,
  checkOutcome,
  displayCommand,
  type LifecycleState,
  probeConflictPaths,
  type RebaseProbe,
  RebaseProbeSchema,
  transition,
  type Verification,
  verificationPlan,
  VerificationSchema,
} from "./lifecycle.ts";
import { type Context, type gitOperation, saveState } from "./runtime.ts";

export { checkOutcome, displayCommand } from "./lifecycle.ts";

/** Control-checkout policy read live at each verification. */
export const VerificationSettingsSchema = z.object({
  baselineChecks: z.array(BaselineCheckSchema).max(16).refine(
    (checks) => new Set(checks.map((check) => check.id)).size === checks.length,
    "Baseline check IDs must be unique",
  ),
  rebaseProbe: z.boolean(),
}).strict();
export type VerificationSettings = z.infer<typeof VerificationSettingsSchema>;
/** Settings for fixtures that run neither baseline checks nor the probe. */
export const NO_VERIFICATION_EXTRAS: VerificationSettings = {
  baselineChecks: [],
  rebaseProbe: false,
};

/** Read `.yinz/issue-lifecycle.json`'s verification section through the Git helper. */
export async function loadVerificationSettings(
  context: Context,
  git: typeof gitOperation,
) {
  return VerificationSettingsSchema.parse(
    await git(context, { action: "verification-settings" }),
  );
}

export interface VerificationDependencies {
  gitOperation: typeof gitOperation;
  id: () => string;
  now: () => string;
  verificationSettings: (context: Context) => Promise<VerificationSettings>;
}

const CheckOutcome = z.object({
  exitCode: z.number().int().nullable(),
  durationMs: z.number().int().nonnegative(),
  worktreeUnchanged: z.boolean(),
  timedOut: z.literal(true).optional(),
}).strip();

export function checkLogName(verificationId: string, checkId: string) {
  return `check-log-${verificationId}-${checkId}`;
}

/** A distinct prefix, so a plan check named `baseline-x` cannot collide. */
export function baselineLogName(verificationId: string, checkId: string) {
  return `baseline-log-${verificationId}-${checkId}`;
}

/** The command an operator runs to read one check's stored output. */
export function checkLogQuery(issueName: string, log: string) {
  return `swamp data query 'modelName == ${
    JSON.stringify(issueName)
  } && name == ${JSON.stringify(log)}' --select content`;
}

function probeLine(probe: RebaseProbe) {
  if (probe.status === "clean") {
    return `- [x] rebase probe: no conflicts with local main ${probe.main}`;
  }
  if (probe.status === "conflicts") {
    return `- [ ] rebase probe: conflicts with local main ${probe.main} in ${
      probeConflictPaths(probe)
    }`;
  }
  return `- rebase probe unavailable: ${probe.reason} (not a failure; the merge gate rebases under its lock)`;
}

type RunLine =
  | Verification["checks"][number]
  | NonNullable<
    Verification["baseline"]
  >[number];

function runLine(
  issueName: string,
  label: string,
  check: RunLine,
  covers?: string[],
) {
  return `- [${check.status === "passed" ? "x" : " "}] ${label}: ${
    displayCommand(check.command)
  } -> ${checkOutcome(check)} in ${(check.durationMs / 1000).toFixed(1)}s${
    covers ? ` (covers ${covers.join(", ")})` : ""
  }. ${
    check.log ? `Log: ${checkLogQuery(issueName, check.log)}` : check.skipped
  }`;
}

export function verificationChecklist(
  issueName: string,
  verification: Verification,
): string[] {
  return [
    `Verification ${verification.id}: ${verification.status} at ${verification.commit} (plan version ${verification.planVersion})`,
    ...(verification.rebaseProbe ? [probeLine(verification.rebaseProbe)] : []),
    ...(verification.baseline ?? []).map((check) =>
      runLine(issueName, `baseline ${check.id}`, check)
    ),
    ...verification.checks.map((check) =>
      runLine(issueName, check.id, check, check.covers)
    ),
  ];
}

async function rebaseProbe(
  context: Context,
  dependencies: VerificationDependencies,
  commit: string,
): Promise<RebaseProbe> {
  try {
    return RebaseProbeSchema.parse(
      await dependencies.gitOperation(context, {
        action: "rebase-probe",
        expectedHead: commit,
      }),
    );
  } catch (error) {
    if (context.signal.aborted) throw error;
    const message = (error instanceof Error ? error.message : String(error))
      .trim().slice(0, 400);
    return {
      status: "unavailable",
      reason: message || "The rebase probe failed",
    };
  }
}

/**
 * The caller owns the issue lock and has confirmed that the clean worktree is at
 * the reviewed commit. The rebase probe runs first, then the configured
 * baseline checks, then the plan checks. Every check runs even after a failure,
 * so the checklist is complete. The exceptions are a check that changes the
 * worktree and one the Git helper cannot run: later checks are then recorded
 * as not run, and the run is still saved as a failure. No merge lock is taken.
 */
export async function runVerification(
  context: Context,
  state: LifecycleState,
  dependencies: VerificationDependencies,
) {
  const plan = verificationPlan(state);
  const review = state.codeReview;
  const implementation = state.implementation;
  if (!plan?.checks) {
    throw new Error(
      "This legacy plan has no structured checks to verify; revise the plan to add checks",
    );
  }
  if (!review || !implementation) {
    throw new Error("Verification requires a recorded code review");
  }
  const id = dependencies.id();
  const startedAt = dependencies.now();
  const record = {
    id,
    planVersion: plan.version,
    commit: review.commit,
    branch: implementation.branch,
    startedAt,
  };
  // Validate phase, approval, both current reviews and commit binding before running any command.
  transition(state, "verify", {
    verification: {
      ...record,
      finishedAt: startedAt,
      status: "passed",
      checks: plan.checks.map((check) => ({
        ...check,
        exitCode: 0,
        durationMs: 0,
        log: checkLogName(id, check.id),
        worktreeUnchanged: true,
        status: "passed",
      })),
    },
  });
  // A configuration error stops verification before anything runs.
  const settings = VerificationSettingsSchema.parse(
    await dependencies.verificationSettings(context),
  );
  context.signal.throwIfAborted();
  const probe = settings.rebaseProbe
    ? await rebaseProbe(context, dependencies, review.commit)
    : undefined;
  if (probe) {
    context.logger.info("Rebase probe: {status}", { status: probe.status });
  }
  // Set once the reviewed clean tree can no longer be trusted for later checks.
  let stopped: string | undefined;
  async function runOne(
    entry: { id: string; command: string[] },
    log: string,
  ) {
    context.signal.throwIfAborted();
    if (stopped) {
      return {
        exitCode: null,
        durationMs: 0,
        skipped: `Not run: ${stopped}`,
        worktreeUnchanged: false,
        status: "failed" as const,
      };
    }
    let outcome: z.infer<typeof CheckOutcome>;
    try {
      outcome = CheckOutcome.parse(
        await dependencies.gitOperation(
          context,
          {
            action: "check",
            worktree: implementation!.worktree,
            expectedBranch: implementation!.branch,
            expectedHead: review!.commit,
            argv: entry.command,
          },
          log,
          "check-log",
        ),
      );
    } catch (error) {
      if (context.signal.aborted) throw error;
      // Keep completed results: record the refusal (for example, a worktree
      // that changed after the previous check) instead of losing the run.
      stopped = `check ${entry.id} could not run`;
      return {
        exitCode: null,
        durationMs: 0,
        skipped: `Could not run: ${
          error instanceof Error ? error.message : String(error)
        }`,
        worktreeUnchanged: false,
        status: "failed" as const,
      };
    }
    if (!outcome.worktreeUnchanged) {
      stopped = `check ${entry.id} changed the worktree`;
    }
    context.logger.info("Check {check} exited {code} in {duration} ms", {
      check: entry.id,
      code: outcome.exitCode,
      duration: outcome.durationMs,
    });
    return {
      ...outcome,
      log,
      status: outcome.exitCode === 0 && outcome.worktreeUnchanged
        ? "passed" as const
        : "failed" as const,
    };
  }
  const baseline: NonNullable<Verification["baseline"]> = [];
  for (const check of settings.baselineChecks) {
    baseline.push({
      ...check,
      ...await runOne(
        { id: `baseline ${check.id}`, command: check.command },
        baselineLogName(id, check.id),
      ),
    });
  }
  const checks: Verification["checks"] = [];
  for (const check of plan.checks) {
    checks.push({
      ...check,
      ...await runOne(check, checkLogName(id, check.id)),
    });
  }
  const verification = VerificationSchema.parse({
    ...record,
    finishedAt: dependencies.now(),
    status: [...baseline, ...checks].every((check) =>
        check.status === "passed"
      ) && probe?.status !== "conflicts"
      ? "passed"
      : "failed",
    checks,
    ...(baseline.length ? { baseline } : {}),
    ...(probe ? { rebaseProbe: probe } : {}),
  });
  return saveState(
    context,
    "verify",
    transition(state, "verify", { verification }),
  );
}
