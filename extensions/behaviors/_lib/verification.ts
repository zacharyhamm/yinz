/** Run the approved plan's checks at the reviewed commit and record the results. */
import { z } from "npm:zod@4.6.7";
import {
  type LifecycleState,
  transition,
  type Verification,
  verificationPlan,
  VerificationSchema,
} from "./lifecycle.ts";
import { type Context, type gitOperation, saveState } from "./runtime.ts";

export interface VerificationDependencies {
  gitOperation: typeof gitOperation;
  id: () => string;
  now: () => string;
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

/** The command an operator runs to read one check's stored output. */
export function checkLogQuery(issueName: string, log: string) {
  return `swamp data query 'modelName == ${
    JSON.stringify(issueName)
  } && name == ${JSON.stringify(log)}' --select content`;
}

/** Quote argv for display only; the lifecycle never runs it through a shell. */
export function displayCommand(argv: string[]) {
  return argv.map((value) =>
    /^[A-Za-z0-9_@%+=:,./-]+$/.test(value)
      ? value
      : `'${value.replaceAll("'", `'"'"'`)}'`
  ).join(" ");
}

/** How one check ended, shared by the checklist and drive's blocked reason. */
export function checkOutcome(check: Verification["checks"][number]) {
  if (check.skipped) return "not run";
  const ended = check.timedOut
    ? "timed out"
    : `exit ${check.exitCode ?? "(could not start)"}`;
  return check.worktreeUnchanged
    ? ended
    : `${ended}; changed the worktree, branch or HEAD`;
}

export function verificationChecklist(
  issueName: string,
  verification: Verification,
): string[] {
  return [
    `Verification ${verification.id}: ${verification.status} at ${verification.commit} (plan version ${verification.planVersion})`,
    ...verification.checks.map((check) =>
      `- [${check.status === "passed" ? "x" : " "}] ${check.id}: ${
        displayCommand(check.command)
      } -> ${checkOutcome(check)} in ${
        (check.durationMs / 1000).toFixed(1)
      }s (covers ${check.covers.join(", ")}). ${
        check.log
          ? `Log: ${checkLogQuery(issueName, check.log)}`
          : check.skipped
      }`
    ),
  ];
}

/**
 * The caller owns the issue lock and has confirmed that the clean worktree is at
 * the reviewed commit. Every check runs even after a failure, so the checklist
 * is complete. The exceptions are a check that changes the worktree and one the
 * Git helper cannot run: later checks are then recorded as not run, and the run
 * is still saved as a failure. No merge lock is taken.
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
  const checks: Verification["checks"] = [];
  // Set once the reviewed clean tree can no longer be trusted for later checks.
  let stopped: string | undefined;
  for (const check of plan.checks) {
    context.signal.throwIfAborted();
    if (stopped) {
      checks.push({
        ...check,
        exitCode: null,
        durationMs: 0,
        skipped: `Not run: ${stopped}`,
        worktreeUnchanged: false,
        status: "failed",
      });
      continue;
    }
    const log = checkLogName(id, check.id);
    let outcome: z.infer<typeof CheckOutcome>;
    try {
      outcome = CheckOutcome.parse(
        await dependencies.gitOperation(
          context,
          {
            action: "check",
            worktree: implementation.worktree,
            expectedBranch: implementation.branch,
            expectedHead: review.commit,
            argv: check.command,
          },
          log,
          "check-log",
        ),
      );
    } catch (error) {
      if (context.signal.aborted) throw error;
      // Keep completed results: record the refusal (for example, a worktree
      // that changed after the previous check) instead of losing the run.
      checks.push({
        ...check,
        exitCode: null,
        durationMs: 0,
        skipped: `Could not run: ${
          error instanceof Error ? error.message : String(error)
        }`,
        worktreeUnchanged: false,
        status: "failed",
      });
      stopped = `check ${check.id} could not run`;
      continue;
    }
    if (!outcome.worktreeUnchanged) {
      stopped = `check ${check.id} changed the worktree`;
    }
    context.logger.info("Check {check} exited {code} in {duration} ms", {
      check: check.id,
      code: outcome.exitCode,
      duration: outcome.durationMs,
    });
    checks.push({
      ...check,
      ...outcome,
      log,
      status: outcome.exitCode === 0 && outcome.worktreeUnchanged
        ? "passed" as const
        : "failed" as const,
    });
  }
  const verification = VerificationSchema.parse({
    ...record,
    finishedAt: dependencies.now(),
    status: checks.every((check) => check.status === "passed")
      ? "passed"
      : "failed",
    checks,
  });
  return saveState(
    context,
    "verify",
    transition(state, "verify", { verification }),
  );
}
