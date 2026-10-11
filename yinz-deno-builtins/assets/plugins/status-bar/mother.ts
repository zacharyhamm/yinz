import type { SessionInfo } from "yinz:plugin";
import type { ReadonlyWire } from "./keys.ts";

export function motherLabel(sessions: ReadonlyWire<SessionInfo[]>): string {
  const status = sessions.find((s) => s.is_current_session)?.mother_supervision;
  if (!status) return "";
  const labels: Record<string, string> = {
    watching: "watching",
    processing: "working",
    queued: "queued",
    paused: "paused",
    needs_attention: "attention",
    degraded: "degraded",
  };
  return ` Mother: ${labels[status.state] ?? "degraded"} (${status.pending}/${status.blockers}) `;
}
