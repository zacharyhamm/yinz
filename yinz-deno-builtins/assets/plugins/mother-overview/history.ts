import type { ReadonlyWire } from "../status-bar/keys.ts";
import z, { type DataRecord, type DataSnapshot, type MotherOverview } from "yinz:plugin";

export type RecordValue = Readonly<Record<string, unknown>>;
type Overview = ReadonlyWire<Pick<MotherOverview, "mother" | "status">> & {
  readonly roster: Readonly<Record<string, RecordValue>>;
};
export interface Agent {
  id: string;
  pane: string;
  conversation: string;
  name: string;
  row: RecordValue;
  live: boolean;
}
export interface Run {
  id: string;
  agent: string;
  value: RecordValue;
}
export interface Message {
  id: string;
  run: string;
  agent: string;
  revision: number;
  kind: string;
  text: string;
  time: unknown;
  raw: RecordValue;
}
export const clean = (value: unknown): string =>
  z.text.sanitize(typeof value === "string" ? value : (JSON.stringify(value) ?? "Unknown"));
export function object(value: unknown): RecordValue {
  if (typeof value === "string") return object(JSON.parse(value));
  if (value !== null && typeof value === "object" && !Array.isArray(value))
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- Object shape is checked; all field values remain unknown.
    return value as RecordValue;
  return {};
}
export function roster(overview: Overview, records: readonly RecordValue[]): Agent[] {
  const agents = new Map<string, Agent>();
  const own = overview.mother;
  const add = (pane: string, row: RecordValue, live: boolean, name = pane): void => {
    const conversation = typeof row["conversation_id"] === "string" ? row["conversation_id"] : "";
    const id = JSON.stringify([pane, conversation]);
    agents.set(id, { id, pane, conversation, name, row, live });
  };
  // Journal identities retain departed conversations, never infer ancestry from a pane number.
  for (const row of [...records].sort(
    (a, b) => Number(a["journal_cursor"]) - Number(b["journal_cursor"]),
  ))
    if (typeof row["pane_id"] === "string" && typeof row["conversation_id"] === "string") {
      const state =
        row["event_type"] === "pane_closed"
          ? "closed"
          : row["event_type"] === "pane_transferred"
            ? "transferred"
            : row["event_type"] === "pane_replaced" || row["event_type"] === "conversation_replaced"
              ? "replaced"
              : undefined;
      add(row["pane_id"], state ? { ...row, state, needs_input: false } : row, false);
    }
  for (const [pane, row] of Object.entries(overview.roster)) add(pane, row, true);
  if (own)
    add(
      own.pane_id,
      { ...own, kind: own.kind ?? "Mother", state: own.state ?? "unknown" },
      true,
      "Mother",
    );
  else add("", { kind: "Mother", state: overview.status.state }, false, "Mother");
  return [...agents.values()].sort(
    (a, b) =>
      Number(b.name === "Mother") - Number(a.name === "Mother") ||
      clean(a.row["projects"] ?? "").localeCompare(clean(b.row["projects"] ?? "")) ||
      a.id.localeCompare(b.id),
  );
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Forward the SDK query wire without converting it.
const fetch = (query: Parameters<typeof z.data.fetch>[0]): DataSnapshot =>
  z.data.fetch({ namespace: "shared:yinz", ...query });
function decodeRun(record: ReadonlyWire<DataRecord>, agent: string): Run {
  const value = object(record["header"] ?? record["payload"]);
  return { id: String(record["uid"] ?? value["id"]), agent, value };
}
export function runsFor(
  agent: Readonly<Agent>,
  overview: Overview,
): { runs: Run[]; warning: string } {
  const own = overview.mother;
  if (!own || !agent.conversation)
    return { runs: [], warning: "Conversation identity unavailable; archive not matched." };
  const matches = fetch({
    entity: "agent_run",
    filter: z.data.and(
      z.data.or(z.data.eq("host_id", own.host_id), z.data.eq("host_id", "")),
      z.data.eq("yinz_session_name", own.yinz_session_name),
      z.data.eq("pane_id", Number(agent.pane.replace("terminal_", ""))),
      z.data.eq("session_id", agent.conversation),
    ),
    sort: [["started_at_ms", "Desc"]],
    select: ["uid", "header", "payload"],
    limit: 2,
  });
  const first = matches.records[0];
  if (!first || matches.records.length > 1)
    return {
      runs: [],
      warning:
        matches.records.length > 0
          ? "Ambiguous archive identity."
          : "Missing archive for this conversation.",
    };
  return ancestry(decodeRun(first, agent.id));
}
function ancestry(first: Readonly<Run>): { runs: Run[]; warning: string } {
  const runs = [first];
  const seen = new Set([runs[0]?.id]);
  let next = runs[0]?.value["previous_run_id"];
  while (typeof next === "string" && next) {
    if (seen.has(next) || runs.length >= 32)
      return { runs, warning: "Incomplete ancestry: repeated link or 32-run limit." };
    seen.add(next);
    const record = fetch({
      entity: "agent_run",
      filter: z.data.eq("uid", next),
      select: ["uid", "header", "payload"],
      limit: 1,
    }).records[0];
    if (!record) return { runs, warning: `Incomplete ancestry: missing run ${next}.` };
    const previous = decodeRun(record, first.agent);
    runs.push(previous);
    next = previous.value["previous_run_id"];
  }
  const anchor = runs[0]?.value;
  const incomplete =
    typeof anchor?.["origin_run_id"] === "string" && anchor["origin_run_id"] !== runs.at(-1)?.id;
  return {
    runs,
    warning: incomplete ? "Incomplete ancestry: recorded origin is not reachable." : "",
  };
}
function message(record: RecordValue, runId: string, agent: string): Message {
  const revision = Number(record["revision"] ?? 0);
  return {
    id: `${runId}:${revision}`,
    run: runId,
    agent,
    revision,
    kind: clean(record["kind"] ?? "message"),
    text: clean(record["text"] ?? ""),
    time: record["created_at_ms"],
    raw: record,
  };
}
export function entries(
  run: Readonly<Run>,
  before?: number,
): { messages: Message[]; more: boolean } {
  const result = fetch({
    entity: "agent_run_entry",
    filter: z.data.and(
      z.data.eq("run_id", run.id),
      ...(before === undefined ? [] : [z.data.lt("revision", before)]),
    ),
    sort: [["revision", "Desc"]],
    limit: 51,
  });
  let records: readonly RecordValue[] = result.records;
  if (records.length === 0) {
    const stored = fetch({
      entity: "agent_run",
      filter: z.data.eq("uid", run.id),
      select: ["payload"],
      limit: 1,
    }).records[0];
    const payload = object(stored?.["payload"]);
    if (Array.isArray(payload["entries"]))
      records = payload["entries"]
        .map(object)
        .filter((row) => before === undefined || Number(row["revision"]) < before)
        .sort((a, b) => Number(b["revision"]) - Number(a["revision"]));
  }
  return {
    messages: records
      .slice(0, 50)
      .sort((a, b) => Number(a["revision"]) - Number(b["revision"]))
      .map((row) => message(row, run.id, run.agent)),
    more: records.length > 50 || result.raw.truncated,
  };
}
export function search(
  runs: readonly Readonly<Run>[],
  query: string,
): { messages: Message[]; more: boolean } {
  if (runs.length === 0) return { messages: [], more: false };
  const result = fetch({
    entity: "agent_run_entry",
    filter: z.data.in(
      "run_id",
      runs.map((run) => run.id),
    ),
    search: query,
    sort: [["created_at_ms", "Desc"]],
    limit: 101,
  });
  return {
    messages: result.records
      .slice(0, 100)
      .map((row) =>
        message(
          row,
          String(row["run_id"]),
          runs.find((run) => run.id === row["run_id"])?.agent ?? "",
        ),
      ),
    more: result.records.length > 100 || result.raw.truncated,
  };
}
export function timestamp(value: unknown, now = Date.now()): string {
  if (typeof value !== "number" || !Number.isFinite(new Date(value).getTime())) return "—";
  const minutes = Math.floor(Math.max(0, now - value) / 60000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)}h`;
  return `${Math.floor(minutes / 1440)}d`;
}
export function activity(record: RecordValue): Message {
  const text = [record["summary"], record["detail"], record["reason"]]
    .filter((value) => value != null)
    .map((value) => clean(value))
    .join("\n\n");
  return {
    id: `journal:${String(record["journal_cursor"])}`,
    run: "journal",
    agent: clean(record["pane_id"] ?? "Mother"),
    revision: Number(record["journal_cursor"] ?? 0),
    kind: clean(record["event_type"] ?? "activity"),
    text:
      text ||
      `${clean(record["pane_id"] ?? "Mother")} · ${clean(record["state"] ?? record["conversation_id"] ?? "Supervision event")}`,
    time: record["observed_at_ms"] ?? record["timestamp_ms"] ?? record["created_at_ms"],
    raw: record,
  };
}
