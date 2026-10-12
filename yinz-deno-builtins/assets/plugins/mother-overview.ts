import type { ReadonlyWire } from "./status-bar/keys.ts";
import z, {
  definePlugin,
  type MotherRequest,
  type MotherOverview,
  type MotherJournalPage,
  type TimerHandle,
} from "yinz:plugin";

let visible = false;
let busy = false;
let timer: TimerHandle | undefined;
let overview: MotherOverview | undefined;
let history: MotherJournalPage | undefined;
let error = "";
let scroll = 0;
let selectedWorker: string | undefined;
let reviewVisible = false;
let review: { submission: string; revision: number } | undefined;
const clean = (value: unknown): string =>
  z.text.sanitize(typeof value === "string" ? value : (JSON.stringify(value) ?? "Unavailable"));
const failed = (cause: unknown): void => {
  error = String(cause);
  z.requestRender();
};
async function request<T>(command: ReadonlyWire<MotherRequest>): Promise<T> {
  const response = await z.agent.control({ Mother: command });
  if (response.outcome !== "ok" || response.output === null)
    throw new Error(
      response.message ?? "Supervision unavailable; reopen Conversation to inspect Mother.",
    );
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- The shared Mother protocol owns these host replies; display values are sanitized below.
  return JSON.parse(response.output) as T;
}
async function refresh(): Promise<void> {
  if (!visible || busy) return;
  busy = true;
  try {
    overview = await request<MotherOverview>("Overview");
    if (!selectedWorker || !(selectedWorker in overview.roster))
      selectedWorker = Object.keys(overview.roster)[0];
    error = "";
  } catch (cause) {
    error = String(cause);
  } finally {
    busy = false;
    z.requestRender();
  }
}
async function action(command: ReadonlyWire<MotherRequest>): Promise<void> {
  if (busy) return;
  busy = true;
  let actionError = "";
  try {
    await request(command);
    error = "";
  } catch (cause) {
    actionError = String(cause);
  } finally {
    busy = false;
    await refresh();
    if (actionError) {
      error = actionError;
      z.requestRender();
    }
  }
}
async function page(older: boolean): Promise<void> {
  if (busy) return;
  busy = true;
  try {
    history = await request<MotherJournalPage>({
      Journal: {
        after: older ? (history?.next_cursor ?? null) : null,
        through: older ? (history?.high_water ?? null) : null,
        limit: 50,
      },
    });
    error = "";
  } catch (cause) {
    error = String(cause);
  } finally {
    busy = false;
    z.requestRender();
  }
}
function workerLines(): string[] {
  const workers = Object.entries(overview?.roster ?? {});
  const result = ["", "Workers (turn ended does not mean verified task completion)"];
  if (workers.length === 0)
    result.push("No observed workers. Give Mother an assignment in Conversation.");
  for (const [pane, row] of workers) {
    result.push(
      `${pane === selectedWorker ? ">" : " "} ${pane} · ${clean(row["kind"])} · ${clean(row["state"])} · projects: ${clean(row["projects"])}`,
      `  Task: ${clean(row["assignment"] ?? "No saved task")} · Activity: ${clean(row["current_activity"] ?? row["detail"])}`,
      `  Last known outcome: ${clean(row["retained_result"] ?? row["last_outcome"] ?? "Unknown; inspect correlated results and available archives")}`,
    );
    if (row["needs_input"] === true)
      result.push(
        "  Attention: open the worker and review its request. Permissions require the user.",
      );
  }
  return result;
}
function lines(): string[] {
  if (!overview) return ["Mother Overview", error || "Loading supervision…"];
  const { status, checkpoint, batches } = overview;
  const memory = checkpoint.value ?? {};
  const result = [
    `Supervision: ${overview.paused ? "paused" : status.state} | ${status.pending} pending | ${status.blockers} need attention`,
    "p: pause/resume · h: history · n: next page · j/k: worker · f: focus · r: review missing result",
    "Up/Down: scroll · use the tab strip to return to Conversation",
  ];
  if (error) result.push(`STALE — ${error}`);
  if (overview.delivery_blocked) result.push(`Delivery held: ${overview.delivery_blocked}`);
  if (overview.paused || status.state === "paused")
    result.push("Observation continues. Running turns are not interrupted.");
  if (status.state === "degraded" || !overview.observation_ready)
    result.push(
      "Open Conversation; finish startup dialogs and verify lifecycle hooks with yinz hooks install.",
    );
  if (overview.gap)
    result.push(
      "Attention: retained evidence has gaps. Reconcile workers and available archives before continuing.",
    );
  result.push(
    "",
    "Assignment",
    `Goal: ${clean(memory["goal"] ?? memory["assignment"] ?? "Give Mother an assignment in Conversation.")}`,
  );
  for (const name of ["scope", "assignments", "questions", "decisions", "blockers"])
    if (memory[name] != null) result.push(`${name}: ${clean(memory[name])}`);
  result.push(
    ...workerLines(),
    "",
    "Attention",
    "Archives are not loaded by Overview. Missing evidence remains unknown.",
  );
  for (const receipt of overview.receipts.filter(
    (item) =>
      !item.settled ||
      (item.response?.outcome === "failed" && batches.some((batch) => batch.id === item.batch)),
  ))
    result.push(`${receipt.submission}: ${receipt.response?.outcome ?? "operation in progress"}`);
  result.push(
    "",
    "Recent activity — h: load retained history, n: next chronological page",
    ...activityLines(),
  );
  return result;
}
function activityLines(): string[] {
  const activity = history ?? overview?.recent;
  if (!activity) return [];
  const result: string[] = [];
  if (activity.retention_gap) result.push("Retention gap: earlier records are unavailable.");
  result.push(
    ...activity.records.map((record) => clean(record)),
    activity.next_cursor === null
      ? "End of this history boundary."
      : "n: more records through the same boundary",
  );
  return result;
}
function workerKey(char: string): void {
  const workers = Object.keys(overview?.roster ?? {});
  let worker = Math.max(0, workers.indexOf(selectedWorker ?? ""));
  if (char === "j") worker = Math.min(worker + 1, Math.max(0, workers.length - 1));
  if (char === "k") worker = Math.max(0, worker - 1);
  selectedWorker = workers[worker];
  if (char === "f" && workers[worker]?.startsWith("terminal_"))
    z.command("FocusTerminalPane", [Number(workers[worker]?.slice(9)), true, false]);
}
function characterKey(char: string): void {
  if (char === "p" && overview)
    void action(overview.paused || overview.status.state === "paused" ? "Resume" : "Pause").catch(
      failed,
    );
  if (char === "h") void page(false).catch(failed);
  if (char === "n" && history?.next_cursor != null) void page(true).catch(failed);
  workerKey(char);
  if (char === "r" && overview && !overview.operation_in_flight) {
    const receipt = overview.receipts.find(
      (item) => !item.settled && item.response?.outcome === "status_unavailable",
    );
    if (receipt)
      review = { submission: receipt.submission, revision: overview.checkpoint.revision };
  }
  if (char === "y" && review && reviewVisible) {
    void action({
      Resolve: { submission_id: review.submission, expect_rev: review.revision },
    }).catch(failed);
    review = undefined;
  }
}
export default definePlugin({
  load() {
    z.subscribe(["Visible", "Key"]);
  },
  update(event) {
    if (typeof event !== "object") return false;
    if ("Visible" in event) {
      visible = event.Visible;
      timer?.cancel();
      if (visible) {
        void refresh().catch(failed);
        timer = z.every(1000, refresh);
      }
    }
    if (!("Key" in event)) return false;
    const key = event.Key.bare_key;
    if (key === "Down") scroll += 1;
    else if (key === "Up") scroll = Math.max(0, scroll - 1);
    else if (key === "Esc") review = undefined;
    else if (typeof key === "object" && "Char" in key) characterKey(key.Char);
    z.requestRender();
    return true;
  },
  unload() {
    timer?.cancel();
  },
  render(rows, columns) {
    reviewVisible = !!review && rows >= 5 && columns >= 40;
    if (review)
      return [
        `Review submission ${review.submission}`,
        "Inspect workers and available archives first.",
        "Work may already have occurred. Outcome stays unknown; no replay.",
        reviewVisible
          ? "y: Reviewed; continue supervision"
          : "Enlarge the panel to show the recovery control.",
        "Esc: cancel",
      ]
        .slice(0, rows)
        .map((line) => z.text.truncate(clean(line), columns))
        .join("\n");
    const content = lines();
    scroll = Math.min(scroll, Math.max(0, content.length - rows));
    return content
      .slice(scroll, scroll + rows)
      .map((line) => z.text.truncate(clean(line), columns))
      .join("\n");
  },
});
