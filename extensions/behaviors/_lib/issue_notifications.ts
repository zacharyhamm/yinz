/** Best-effort attention requests for durable human-intervention handoffs. */
import type { Handoff } from "./issue_driver.ts";
import type { Context } from "./runtime.ts";
import { type YinzRunner } from "./yinz_agent.ts";

const labels: Partial<Record<Handoff["decision"], string>> = {
  authorize_plan_revision: "Plan revision needs a decision",
  approve_plan: "Plan approval needed",
  address_review_findings: "Review findings need a decision",
  approve_review_fixes: "Review fixes need approval",
  authorize_merge: "Merge authorization needed",
  authorize_closeout: "Close-out decision needed",
  blocked: "Lifecycle blocked",
};

export async function notifyHandoff(
  context: Context,
  handoff: Handoff,
  control: { session: string; paneId: string } | undefined,
  run: YinzRunner,
) {
  const label = labels[handoff.decision];
  if (!label) return;
  try {
    if (!control) throw new Error("Control pane identity is unavailable");
    const issue = handoff.state.issue;
    const reason = handoff.reason.replace(/\s+/g, " ").trim();
    const brief = reason.length > 240 ? `${reason.slice(0, 239)}…` : reason;
    const result = await run(
      [
        "--session",
        control.session,
        "action",
        "notify",
        "--pane-id",
        control.paneId,
        "--",
        `${issue.workspace}/${issue.itemId}: ${label}. ${brief}`,
      ],
      undefined,
      AbortSignal.any([context.signal, AbortSignal.timeout(5000)]),
    );
    if (result.code !== 0) {
      throw new Error(result.stderr || `Yinz exited ${result.code}`);
    }
  } catch (error) {
    context.logger.info("Cannot send lifecycle notification: {reason}", {
      reason: String(error),
    });
  }
}
