/**
 * Bounded self-healing of correlated worker failures. Every retry re-observes
 * the original task through resumeTask: it never resends a prompt and never
 * spawns after a launch intent. Everything else stays with a human.
 *
 * This module must not import issue_driver.ts.
 */
import type { z } from "npm:zod@4.6.7";
import {
  type AgentTaskSchema,
  childState,
  type IssueAgentContext,
} from "./issue_agents.ts";
import { SubmissionSchema } from "./issue_evidence.ts";
import {
  type AgentResponse,
  AgentResponseSchema,
  type AgentState,
  type CleanupRefusal,
} from "./yinz_agent.ts";

type Task = z.infer<typeof AgentTaskSchema>;

export type RetryClass = "long_wait" | "reobserve" | "cleanup" | "preflight";
export type WorkerVerdict =
  | { kind: "retry"; class: RetryClass; reason: string }
  | { kind: "human"; reason: string };

export const RECOVERY_POLICY: Record<
  RetryClass,
  { maxAttempts: number; delaysMs: readonly number[] }
> = {
  // Each attempt is itself a read-only wait of task.config.timeoutSeconds.
  long_wait: { maxAttempts: 48, delaysMs: [0] },
  reobserve: { maxAttempts: 2, delaysMs: [1_000, 5_000] },
  cleanup: {
    maxAttempts: 6,
    delaysMs: [2_000, 5_000, 10_000, 20_000, 30_000, 60_000],
  },
  preflight: { maxAttempts: 3, delaysMs: [5_000, 15_000, 30_000] },
};
/** The absolute recovery deadline for one drive invocation. */
export const RECOVERY_DEADLINE_MS = 8 * 3_600_000;

export type Grant = {
  allowed: boolean;
  attempt: number;
  limit: number;
  delayMs: number;
  elapsedMs: number;
  exhausted?: "attempts" | "deadline";
};

/**
 * Retry allowances for one drive invocation, per task and class. It is never
 * persisted: rerunning drive is an explicit act that starts a new budget.
 */
export class RecoveryBudget {
  #used = new Map<string, number>();
  #started: number;
  #now: () => number;
  #policy: typeof RECOVERY_POLICY;
  #deadlineMs: number;

  constructor(
    now: () => number,
    policy = RECOVERY_POLICY,
    deadlineMs = RECOVERY_DEADLINE_MS,
  ) {
    this.#now = now;
    this.#policy = policy;
    this.#deadlineMs = deadlineMs;
    this.#started = now();
  }

  take(taskId: string, kind: RetryClass): Grant {
    const policy = this.#policy[kind];
    const key = `${taskId}:${kind}`;
    const attempt = (this.#used.get(key) ?? 0) + 1;
    const delayMs =
      policy.delaysMs[Math.min(attempt - 1, policy.delaysMs.length - 1)];
    const elapsedMs = this.#now() - this.#started;
    const refused = (exhausted: "attempts" | "deadline"): Grant => ({
      allowed: false,
      attempt: attempt - 1,
      limit: policy.maxAttempts,
      delayMs: 0,
      elapsedMs,
      exhausted,
    });
    if (attempt > policy.maxAttempts) return refused("attempts");
    // The pending delay counts: never sleep past the deadline.
    if (elapsedMs + delayMs > this.#deadlineMs) return refused("deadline");
    this.#used.set(key, attempt);
    return {
      allowed: true,
      attempt,
      limit: policy.maxAttempts,
      delayMs,
      elapsedMs,
    };
  }
}

export function stripErrorPrefix(text: string) {
  return text.replace(/^Error: /, "");
}

/**
 * The child's saved failure is the one this error reports, not an older one.
 * AgentStateSchema trims lastError, while Swamp passes the untrimmed message
 * (CLI output usually ends in a newline), with or without an `Error: ` prefix.
 */
function freshFailure(child: AgentState, error: Error) {
  const observed = error.message.trim();
  return !!child.lastError &&
    (child.lastError === observed ||
      child.lastError === stripErrorPrefix(observed).trim());
}

/** cleanupTask's wrapping of a child's cleanup error text. */
function cleanupWrapped(text: string) {
  return text.startsWith("Worker cleanup")
    ? text
    : `Worker cleanup blocked: ${text}`;
}

/** The error reports exactly this saved refusal. */
function freshRefusal(refusal: CleanupRefusal, error: Error) {
  const observed = stripErrorPrefix(error.message);
  // Swamp may report the child's text with or without an `Error: ` prefix,
  // and cleanupTask wraps it before throwing.
  return [refusal.thrown, `Error: ${refusal.thrown}`].some((text) =>
    cleanupWrapped(text) === observed
  );
}

function savedResponse(child: AgentState): AgentResponse | undefined {
  if (!child.rawResponse) return undefined;
  try {
    const parsed = AgentResponseSchema.safeParse(JSON.parse(child.rawResponse));
    return parsed.success ? parsed.data : undefined;
  } catch {
    return undefined;
  }
}

/**
 * A correlated, acknowledged ordinary timeout of the original submission (or
 * startup, or background work), never a dialog, uncertain delivery or another
 * target. The caller supplies the child state.
 */
export function correlatedTimeout(
  task: Task,
  child: AgentState,
  error: unknown,
): boolean {
  // Match the current persisted failure, not a particular human-readable error.
  // A transport error after an older timeout must never reuse that timeout.
  if (
    !["spawned", "submitting", "waiting", "background"].includes(
      child.phase,
    ) ||
    child.failureKind !== "timeout" ||
    child.outcome !== "timed_out" || !child.paneId || !child.rawResponse ||
    !(error instanceof Error) || !freshFailure(child, error) ||
    (child.phase !== "spawned" &&
      (!child.conversationId || !child.submissionId))
  ) return false;
  const response = savedResponse(child);
  if (!response) return false;
  const matches = response.agents.filter((agent) =>
    agent.pane_id === child.paneId
  );
  if (matches.length !== 1) return false;
  const agent = matches[0];
  // A saved `submitting` phase means the prompt response was lost; only the
  // original submission's acknowledged, started reply proves acceptance.
  if (
    child.phase === "submitting" &&
    !(response.acknowledged && response.prompt_delivery === "started" &&
      response.submission_id === child.submissionId)
  ) return false;
  return response.outcome === "timed_out" &&
    (!child.launchServer || !response.server_instance ||
      response.server_instance === child.launchServer) &&
    (!child.turnId || !response.turn_id || response.turn_id === child.turnId) &&
    (!agent.cwd || agent.cwd === task.config.cwd) &&
    response.prompt_delivery !== "not_sent" &&
    // acceptSubmission enters `background` only after a correlated completed
    // reply, so the phase itself is the acknowledgement.
    (child.phase === "spawned" || child.phase === "background" ||
      response.acknowledged || !!response.turn_id) &&
    (child.phase === "spawned" ||
      response.submission_id === child.submissionId ||
      (child.phase === "background" && response.submission_id == null)) &&
    agent.session === task.config.session &&
    agent.agent.kind === task.config.kind &&
    (child.conversationId == null ||
      agent.agent.session_id === child.conversationId) &&
    agent.agent.state.toLowerCase() === "busy" &&
    (agent.not_ready_reason == null ||
      ["not_idle", "unresolved_turn", "unconsumed_input"].includes(
        agent.not_ready_reason,
      ));
}

function humanReason(task: Task, child: AgentState): string {
  const outcome = child.outcome;
  if (child.phase === "launch_rejected") {
    return "launch was rejected; discard the original task deliberately";
  }
  if (child.phase === "launching") {
    return "launch outcome is uncertain; never spawn again";
  }
  if (child.phase === "preflight" && !task.config.control) {
    return "legacy worker has no control identity; restart it deliberately";
  }
  switch (outcome) {
    case "needs_input":
      return "needs_input: a dialog requires deliberate inspection";
    case "target_changed":
    case "target_closed":
      return `${outcome}: the pane or conversation was replaced or closed; never act against a replacement`;
    case "failed":
      return "failed: provider or host failure needs a deliberate discard, restart or continue";
    case "interrupted":
      return "interrupted: the submission or its turn was interrupted; continue or restart deliberately";
    case "status_unavailable":
      return "status_unavailable: the worker's identity is unknown";
    case "hooks_unavailable":
      return "hooks_unavailable: install current Yinz hooks explicitly";
    case "timed_out":
      return "timed_out without an acknowledged, correlated busy observation of the original target (uncertain delivery or identity mismatch)";
    case "transport_error":
      return "transport_error without a fresh recorded reply for the original submission";
    case "preflight_transport":
      return "preflight transport failure is not fresh or follows a launch intent";
  }
  return `${outcome ?? child.phase}: requires deliberate inspection${
    child.lastError ? ` (${child.lastError})` : ""
  }`;
}

/** Pure classification. The first matching rule wins. */
export function classifyWorkerFailure(input: {
  task: Task;
  child?: AgentState;
  receiptAccepted: boolean;
  error: unknown;
}): WorkerVerdict {
  const { task, child, receiptAccepted, error } = input;
  const human = (reason: string): WorkerVerdict => ({ kind: "human", reason });
  if (!(error instanceof Error)) return human("the failure is not an error");
  if (task.discard) return human("a discard intent is pending");
  if (["discarded", "superseded"].includes(task.status)) {
    return human(`the task is ${task.status}`);
  }
  if (task.status === "completed" && task.cleanup === "pending") {
    const refusal = child?.cleanupRefusal;
    if (
      child && refusal?.retryable === true &&
      refusal.submissionId === child.submissionId &&
      refusal.conversationId === child.conversationId &&
      freshRefusal(refusal, error)
    ) {
      return {
        kind: "retry",
        class: "cleanup",
        reason: `transient cleanup guard: ${
          refusal.message ?? refusal.outcome
        }`,
      };
    }
    return human(
      `cleanup refusal requires inspection: ${
        refusal?.message ?? error.message
      }`,
    );
  }
  if (!["running", "applying"].includes(task.status)) {
    return human(`the task is ${task.status}`);
  }
  if (!child) return human("no durable child state");
  if (task.continuation && child.continuation?.id !== task.continuation.id) {
    return human(
      "the recorded continuation has not reached the worker; resuming would send its prompt",
    );
  }
  if (
    child.phase === "completed" && child.outcome === "completed" &&
    !child.backgroundTasks.length && receiptAccepted && !!task.submission
  ) {
    return {
      kind: "retry",
      class: "reobserve",
      reason:
        "completion_unreconciled: the child completed and its evidence is accepted",
    };
  }
  if (correlatedTimeout(task, child, error)) {
    return {
      kind: "retry",
      class: "long_wait",
      reason: `ordinary ${child.phase} wait timeout of the original target`,
    };
  }
  if (
    child.outcome === "transport_error" && child.submissionId &&
    child.paneId &&
    ["waiting", "submitting", "background"].includes(child.phase) &&
    freshFailure(child, error)
  ) {
    return {
      kind: "retry",
      class: "reobserve",
      reason: "recorded transport error after submission",
    };
  }
  if (
    task.config.control && child.phase === "preflight" && !child.launchId &&
    !child.paneId && !child.submissionId &&
    child.outcome === "preflight_transport" && freshFailure(child, error)
  ) {
    return {
      kind: "retry",
      class: "preflight",
      reason: "launch preflight transport failure before any launch intent",
    };
  }
  return human(humanReason(task, child));
}

/** Reads the child state and evidence receipt, then classifies. */
export async function inspectWorkerFailure(
  context: IssueAgentContext,
  task: Task,
  error: unknown,
): Promise<WorkerVerdict> {
  const child = await childState(context, task).catch(() => undefined);
  let receiptAccepted = false;
  if (task.submission) {
    const bytes = await context.dataRepository.getContent(
      "@yinz/issue-evidence",
      task.submission.behaviorName,
      "submission",
    );
    receiptAccepted = !!bytes &&
      SubmissionSchema.parse(JSON.parse(new TextDecoder().decode(bytes)))
          .status === "accepted";
  }
  return classifyWorkerFailure({ task, child, receiptAccepted, error });
}
