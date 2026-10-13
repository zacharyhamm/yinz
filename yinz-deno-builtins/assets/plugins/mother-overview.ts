import type { ReadonlyWire } from "./status-bar/keys.ts";
import z, {
  definePlugin,
  type MotherRequest,
  type MotherOverview,
  type MotherJournalPage,
  type TimerHandle,
  type KeyWithModifier,
  type Mouse,
} from "yinz:plugin";

let visible = false;
let busy = false;
let timer: TimerHandle | undefined;
let overview: MotherOverview | undefined;
let history: MotherJournalPage | undefined;
let error = "";

let reviewVisible = false;
let review: { submission: string; revision: number } | undefined;
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
    if (visible) reconcile();
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
async function journalPage(loadOlder: boolean): Promise<void> {
  if (busy) return;
  busy = true;
  try {
    history = await request<MotherJournalPage>({
      Journal: {
        after: loadOlder ? (history?.next_cursor ?? null) : null,
        through: loadOlder ? (history?.high_water ?? null) : null,
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
import {
  activity,
  clean,
  entries,
  roster,
  runsFor,
  search,
  timestamp,
  type Agent,
  type Message,
  type Run,
  type RecordValue,
} from "./mother-overview/history.ts";

let agents: Agent[] = [];
let selected = "";
let attention = false;
let region: "roster" | "timeline" = "roster";
let view: "conversation" | "assignment" | "activity" = "conversation";
let messages: Message[] = [];
let runs: Run[] = [];
let runIndex = 0;
let more = false;
let messageId = "";
let follow = true;
let newestPage = true;
let reader: Message | undefined;
let readerScroll = 0;
let source = false;
let help = false;
let query: string | undefined;
let hits: Message[] | undefined;
let searchTerms: string[] = [];
let archiveError = "";
let notice = "";
const journal = new Map<number, RecordValue>();
let rosterHits = new Map<number, string>();
let timelineHits = new Map<number, string>();
let pageSize = 5;
const markdown = new Map<string, string[]>();
const needsAttention = (agent: Readonly<Agent>): boolean =>
  agent.row["needs_input"] === true || agent.row["state"] === "needs_input";
const shownAgents = (): Agent[] =>
  agents.filter((agent) => !attention || agent.name === "Mother" || needsAttention(agent));
const chosen = (): Agent | undefined => agents.find((agent) => agent.id === selected);
function remember(records: readonly RecordValue[]): void {
  for (const record of records) journal.set(Number(record["journal_cursor"]), record);
  // The supervisor retains bounded evidence; keep the client bounded too.
  while (journal.size > 4096) journal.delete(journal.keys().next().value ?? 0);
}
function reconcile(): void {
  if (!overview) return;
  remember(overview.recent?.records ?? []);
  agents = roster(overview, [...journal.values()]);
  const shown = shownAgents();
  if (!shown.some((agent) => agent.id === selected)) {
    selected = shown[0]?.id ?? "";
    if (!hits) select(selected);
  } else if (follow && view === "conversation" && !reader && !hits) loadConversation(false);
}
function select(id: string): void {
  selected = id;
  reader = undefined;
  messageId = "";
  follow = true;
  hits = undefined;
  loadConversation(true);
}
function loadConversation(reset: boolean): void {
  const agent = chosen();
  if (!overview || !agent) return;
  if (reset) {
    messages = [];
    runs = [];
    runIndex = 0;
  }
  try {
    const loaded = runsFor(agent, overview);
    runs = loaded.runs;
    archiveError = loaded.warning;
    if (runs.length === 0) {
      messages = [];
      more = false;
      return;
    }
    const run = runs[reset ? 0 : runIndex];
    if (!run) return;
    const page = entries(run);
    newestPage = true;
    messages = page.messages;
    more = page.more;
    if (follow) messageId = messages.at(-1)?.id ?? "";
  } catch (cause) {
    archiveError = `Archive unavailable: ${String(cause)}`;
  }
}
function older(): void {
  try {
    const run = runs[runIndex];
    if (!run) return;
    if (more) {
      const page = entries(run, messages[0]?.revision);
      newestPage = false;
      messages = page.messages;
      more = page.more;
    } else if (runIndex + 1 < runs.length) {
      const previous = runs[runIndex + 1];
      if (!previous) return;
      const page = entries(previous);
      runIndex += 1;
      messages = page.messages;
      more = page.more;
    }
    follow = false;
    messageId = messages.at(-1)?.id ?? "";
  } catch (cause) {
    archiveError = `Archive unavailable: ${String(cause)}`;
  }
}
function currentMessages(): Message[] {
  if (hits) return hits;
  if (view === "activity")
    return (history?.records ?? overview?.recent?.records ?? []).map((record) => activity(record));
  if (view === "assignment")
    return Object.entries(
      overview?.checkpoint.value ?? { goal: "Give Mother an assignment in Conversation." },
    ).map(([key, value]) => ({
      id: key,
      run: "checkpoint",
      agent: "Mother",
      revision: overview?.checkpoint.revision ?? 0,
      kind: key,
      text: clean(value),
      time: null,
      raw: { [key]: value },
    }));
  return messages;
}
function selectedMessage(): Message | undefined {
  const content = currentMessages();
  return content.find((message) => message.id === messageId) ?? content.at(-1);
}
function move(delta: number): void {
  if (reader) {
    readerScroll = Math.max(0, readerScroll + delta);
    return;
  }
  if (region === "roster") {
    const list = shownAgents();
    const index = Math.max(
      0,
      list.findIndex((agent) => agent.id === selected),
    );
    const agent = list[Math.max(0, Math.min(list.length - 1, index + delta))];
    if (agent && agent.id !== selected) select(agent.id);
  } else {
    const list = currentMessages();
    const index = Math.max(
      0,
      list.findIndex((message) => message.id === selectedMessage()?.id),
    );
    const next = Math.max(0, Math.min(list.length - 1, index + delta));
    messageId = list[next]?.id ?? "";
    follow = next === list.length - 1 && runIndex === 0 && newestPage;
  }
}
function open(): void {
  if (region === "roster") {
    region = "timeline";
    return;
  }
  const message = selectedMessage();
  if (!message) return;
  if (hits) {
    const hit = message;
    attention = false;
    selected = hit.agent;
    hits = undefined;
    view = "conversation";
    loadConversation(true);
    runIndex = Math.max(
      0,
      runs.findIndex((run) => run.id === hit.run),
    );
    const run = runs[runIndex];
    newestPage = false;
    messages = [hit];
    more = run !== undefined;
    if (run) {
      try {
        const page = entries(run, hit.revision + 26);
        if (page.messages.some((entry) => entry.id === hit.id)) {
          messages = page.messages;
          more = page.more;
        } else {
          // The byte limit can cut off the hit; anchor a page of preceding entries instead.
          const preceding = entries(run, hit.revision);
          messages = [...preceding.messages.slice(-49), hit];
          more = preceding.more || preceding.messages.length > 49;
        }
      } catch (cause) {
        archiveError = String(cause);
      }
    }
    messageId = hit.id;
    follow = false;
  }
  reader = message;
  readerScroll = 0;
  source = false;
}
function doSearch(): void {
  const text = query?.trim();
  query = undefined;
  if (!text || !overview) return;
  searchTerms = text.toLowerCase().match(/[\p{L}\p{N}_]+/gu) ?? [];
  view = "conversation";
  hits = undefined;
  notice = "";
  try {
    const known: Run[] = [];
    const warnings: string[] = [];
    // Bound synchronous store calls even with a very large retained roster.
    for (const agent of agents.slice(0, 64)) {
      const loaded = runsFor(agent, overview);
      known.push(...loaded.runs);
      if (loaded.warning) warnings.push(`${agent.name}: ${loaded.warning}`);
    }
    const result = search([...new Map(known.map((run) => [run.id, run])).values()], text);
    hits = result.messages;
    notice = `${hits.length} matches${result.more ? " (100-result limit)" : ""}; legacy embedded messages are not indexed.${agents.length > 64 ? " First 64 agents searched." : ""}`;
    archiveError = warnings.join(" ");
    region = "timeline";
    messageId = hits[0]?.id ?? "";
  } catch (cause) {
    archiveError = `Search failed: ${String(cause)}`;
  }
}
function controlKey(char: string): void {
  if (char === "p" && overview)
    void action(overview.paused || overview.status.state === "paused" ? "Resume" : "Pause").catch(
      failed,
    );
  else if (char === "r" && overview && !overview.operation_in_flight) {
    const receipt = overview.receipts.find(
      (item) => !item.settled && item.response?.outcome === "status_unavailable",
    );
    if (receipt) {
      review = { submission: receipt.submission, revision: overview.checkpoint.revision };
      reviewVisible = false;
    }
  } else if (char === "f") {
    const agent = chosen();
    if (agent?.live && agent.pane.startsWith("terminal_"))
      z.command("FocusTerminalPane", [Number(agent.pane.slice(9)), true, false]);
  } else if (char === "y") {
    const message = reader ?? selectedMessage();
    if (message) {
      z.command("CopyToClipboard", clean(message.text));
      notice = "Message copied";
    }
  }
}
function loadJournal(loadOlder: boolean): void {
  void journalPage(loadOlder)
    .then(() => {
      remember(history?.records ?? []);
      reconcile();
    })
    .catch(failed);
}
function changeView(next: typeof view): void {
  reader = undefined;
  if (hits) select(selected);
  view = next;
  hits = undefined;
  messageId = "";
  region = "timeline";
  if (view === "activity") loadJournal(false);
}
function openWarnings(): void {
  const warnings = header();
  reader = {
    id: "supervision:warnings",
    run: "supervision",
    agent: "Mother",
    revision: 0,
    kind: "warnings",
    text: warnings.join("\n\n"),
    time: null,
    raw: { warnings },
  };
  readerScroll = 0;
  source = false;
}
function characterKey(char: string): void {
  switch (char) {
    case "w":
      openWarnings();
      break;
    case "v":
      if (reader) {
        source = !source;
        readerScroll = 0;
      }
      break;
    case "j":
      move(1);
      break;
    case "k":
      move(-1);
      break;
    case "?":
      help = true;
      break;
    case "/":
      reader = undefined;
      query = "";
      break;
    case "a":
      attention = !attention;
      if (!shownAgents().some((agent) => agent.id === selected)) select(shownAgents()[0]?.id ?? "");
      break;
    case "c":
      changeView("conversation");
      break;
    case "d":
      changeView("assignment");
      break;
    case "h":
      changeView("activity");
      break;
    case "n":
      if (view === "activity" && history?.next_cursor != null) loadJournal(true);
      else if (view === "conversation") older();
      break;
    default:
      controlKey(char);
  }
}
function body(message: Readonly<Message>, width: number): string[] {
  const key = JSON.stringify([message.id, message.revision, message.text, width]);
  let result = markdown.get(key);
  if (!result) {
    result = z.ui
      .markdown(clean(message.text))
      .lines(Math.max(1, width))
      .map((line) => line.render());
    if (markdown.size >= 100) markdown.delete(markdown.keys().next().value ?? "");
    markdown.set(key, result);
  }
  return result;
}
const plain = (text: string, width: number): string =>
  z.text.truncate(clean(text).replaceAll("\n", " "), Math.max(0, width));
function header(): string[] {
  if (!overview) return [];
  const result = [
    `Supervision: ${overview.paused ? "paused" : overview.status.state} | ${overview.status.pending} pending | ${overview.status.blockers} need attention`,
  ];
  if (error) result.push(`STALE — ${error}`);
  if (overview.delivery_blocked) result.push(`Delivery held: ${overview.delivery_blocked}`);
  if (!overview.observation_ready)
    result.push("Open Conversation; verify lifecycle hooks and finish startup dialogs.");
  if (overview.gap || history?.retention_gap || overview.recent?.retention_gap)
    result.push("Retention gap: earlier supervision evidence is unavailable.");
  for (const receipt of overview.receipts.filter(
    (item) =>
      !item.settled ||
      (item.response?.outcome === "failed" &&
        overview?.batches.some((batch) => batch.id === item.batch)),
  ))
    result.push(`${receipt.submission}: ${receipt.response?.outcome ?? "operation in progress"}`);
  return result.map((line) => clean(line));
}
function messageLabel(message: Readonly<Message>): string {
  const labels: Readonly<Record<string, string>> = {
    message: "Reply",
    prompt: "Prompt",
    question: "Question",
    plan: "Plan",
    result: "Result",
    tool_call: "Tool call",
    tool_result: "Tool result",
  };
  return labels[message.kind] ?? clean(message.kind).replaceAll("_", " ");
}
function projectLabel(agent: Readonly<Agent>): string {
  const projects = agent.row["projects"];
  return Array.isArray(projects)
    ? projects.map((value) => clean(value)).join(", ") || "Unassigned"
    : clean(projects ?? "Unassigned");
}
function readerLines(message: Readonly<Message>, rows: number, columns: number): string[] {
  const title = plain(
    `${messageLabel(message)} · ${timestamp(message.time)} · ${message.run}`,
    columns,
  );
  const content = source
    ? z.text.wrap(clean(JSON.stringify(message.raw, null, 2)), columns)
    : body(message, columns);
  const height = Math.max(0, rows - 2);
  pageSize = Math.max(1, height);
  readerScroll = Math.min(readerScroll, Math.max(0, content.length - height));
  const result = [title, ...content.slice(readerScroll, readerScroll + height)];
  while (result.length < rows - 1) result.push("");
  result.push(plain("Esc: timeline · v: source · y: copy · ↑↓: scroll", columns));
  return result;
}
function rosterLine(agent: Readonly<Agent>, columns: number): string {
  const state = `${needsAttention(agent) ? "!" : " "}${clean(agent.row["state"] ?? "unknown")}${agent.live ? "" : " (retained)"}`;
  const detail = clean(
    agent.row["assignment"] ?? agent.row["current_activity"] ?? agent.row["detail"] ?? "",
  );
  const nameWidth = Math.min(14, Math.max(1, Math.floor(columns / 3)));
  const label = `${agent.id === selected ? ">" : " "} ${z.text.pad(plain(agent.name, nameWidth), nameWidth)} ${state}`;
  const line =
    columns >= 80
      ? `${z.text.pad(label, 40)} ${z.text.pad(plain(clean(agent.row["kind"] ?? "agent"), 10), 10)} ${detail}`
      : label;
  const styled = z.ui.text(plain(line, columns));
  if (agent.id === selected) styled.emphasis(0);
  if (needsAttention(agent)) styled.error();
  return styled.render();
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Append rows to the frame while recording their hit targets.
function drawRoster(output: string[], height: number, columns: number): void {
  output.push(
    plain(
      `${region === "roster" ? "▶" : "─"} Agents${attention ? " · attention only" : ""} · turn ended ≠ verified completion`,
      columns,
    ),
  );
  const list = shownAgents();
  const index = Math.max(
    0,
    list.findIndex((agent) => agent.id === selected),
  );
  const start = Math.max(0, index - Math.max(1, Math.floor(height / 2)) + 1);
  let project = "";
  const end = output.length + height;
  for (const agent of list.slice(start)) {
    if (output.length >= end) break;
    const group = projectLabel(agent);
    if (columns >= 60 && agent.name !== "Mother" && group !== project && output.length + 1 < end) {
      project = group;
      output.push(plain(`  ${project}`, columns));
    }
    rosterHits.set(output.length, agent.id);
    output.push(rosterLine(agent, columns));
  }
}
function excerpt(message: Readonly<Message>, columns: number): string {
  const text = clean(message.text).replaceAll("\n", " ");
  const lower = text.toLowerCase();
  const term = searchTerms.find((value) => lower.includes(value));
  const start = term ? Math.max(0, lower.indexOf(term) - 20) : 0;
  return plain(`${start > 0 ? "…" : ""}${text.slice(start)}`, columns);
}
function preview(message: Readonly<Message>, columns: number): string[] {
  if (hits) return [excerpt(message, columns)];
  if (message.kind.startsWith("tool")) return [plain(message.text.replaceAll("\n", " "), columns)];
  return body(message, columns).slice(0, 2);
}
function messageWho(message: Readonly<Message>): string {
  return hits
    ? `${agents.find((agent) => agent.id === message.agent)?.name ?? message.agent} · ${message.run} · `
    : "";
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Append rows to the frame while recording their hit targets.
function drawTimeline(output: string[], rows: number, columns: number): void {
  output.push(
    plain(
      `${region === "timeline" ? "▶" : "─"} ${hits ? "Search results" : view} · ${chosen()?.name ?? "Mother"} · ${runs[runIndex]?.id ?? ""}`,
      columns,
    ),
  );
  if (archiveError && view === "conversation") output.push(plain(archiveError, columns));
  if (notice) output.push(plain(notice, columns));
  const content = currentMessages();
  const current = selectedMessage();
  const index = Math.max(
    0,
    content.findIndex((message) => message.id === current?.id),
  );
  pageSize = Math.max(1, Math.floor((rows - output.length - 1) / 3));
  if (content.length === 0)
    output.push(
      plain(
        view === "conversation"
          ? "No retained messages. Give Mother an assignment in Conversation."
          : "No retained entries.",
        columns,
      ),
    );
  for (const message of content.slice(Math.max(0, index - pageSize + 1), index + 1)) {
    if (output.length >= rows - 1) break;
    timelineHits.set(output.length, message.id);
    const who = messageWho(message);
    output.push(
      plain(
        `${message.id === current?.id ? ">" : " "} ${messageLabel(message)} · ${timestamp(message.time)}${who ? ` · ${who}` : ""}`,
        columns,
      ),
    );
    const lines = preview(message, columns);
    for (const line of lines)
      if (output.length < rows - 1) {
        timelineHits.set(output.length, message.id);
        output.push(line);
      }
  }
}
function confirmation(rows: number, columns: number): string[] {
  reviewVisible = rows >= 5 && columns >= 40;
  return [
    `Review submission ${review?.submission}`,
    "Inspect workers and available archives first.",
    "Work may already have occurred. Outcome stays unknown; no replay.",
    reviewVisible
      ? "y: Reviewed; continue supervision"
      : "Enlarge the panel to show the recovery control.",
    "Esc: cancel",
  ]
    .slice(0, rows)
    .map((line) => plain(line, columns));
}
const helpLines = [
  "Mother Overview",
  "Tab: roster / timeline · ↑↓ or j/k: select",
  "PageUp/PageDown or wheel: scroll · Enter: read",
  "Esc: return · v: source · y: copy · w: warnings",
  "c: conversations · d: assignment · h: activity",
  "n: older page / earlier run · End: newest",
  "/: search retained archives · a: attention filter",
  "f: focus live pane · p: pause/resume",
  "r: review missing result · ?: close help",
  "Turn ended does not mean verified task completion.",
  "Pause stops delivery; observation and running turns continue.",
];
function footer(short: boolean): string {
  if (query !== undefined) return `Search: ${query}█ · Enter: search · Esc: cancel`;
  if (short && region === "roster") return "Tab: full-screen history · ?: help";
  if (region === "roster") return "j/k: agent · f: focus · a: attention · Tab: history · ?: help";
  if (hits) return "Enter: context and reader · Esc: timeline · /: search · ?: help";
  if (view === "activity") return "Enter: read · n: next page · c: conversation · ?: help";
  if (view === "assignment") return "Enter: read · c: conversation · h: activity · ?: help";
  return "Tab: roster · Enter: read · n: older · /: search · ?: help";
}
function renderPanel(rows: number, columns: number): string {
  rosterHits = new Map();
  timelineHits = new Map();
  if (rows < 1 || columns < 1) return "";
  if (review) return confirmation(rows, columns).join("\n");
  if (help)
    return helpLines
      .flatMap((line) => z.text.wrap(line, columns))
      .slice(0, rows)
      .join("\n");
  if (!overview) return plain(error || "Loading supervision…", columns);
  const output =
    reader?.id === "supervision:warnings"
      ? []
      : header().flatMap((line) => z.text.wrap(line, columns));
  const headerHeight = Math.max(0, rows - 5);
  if (output.length > headerHeight) {
    output.length = headerHeight;
    if (headerHeight > 0) output[headerHeight - 1] = plain("w: all warnings", columns);
  }
  if (reader)
    return [...output, ...readerLines(reader, rows - output.length, columns)]
      .slice(0, rows)
      .join("\n");
  const short = rows - output.length < 12;
  if (!short || region === "roster")
    drawRoster(
      output,
      Math.max(1, short ? rows - output.length - 2 : Math.floor((rows - output.length - 4) / 2)),
      columns,
    );
  if (!short || region === "timeline") drawTimeline(output, rows, columns);
  while (output.length < rows - 1) output.push("");
  output.push(plain(footer(short), columns));
  return output.slice(0, rows).join("\n");
}
function mouseKey(mouse: ReadonlyWire<Mouse>): void {
  if (review || help || query !== undefined) return;
  if ("ScrollUp" in mouse) move(-3);
  else if ("ScrollDown" in mouse) move(3);
  else if ("LeftClick" in mouse && !reader) {
    const row = Number(mouse.LeftClick[0]);
    const agent = rosterHits.get(row);
    const message = timelineHits.get(row);
    if (agent) {
      region = "roster";
      select(agent);
    }
    if (message) {
      region = "timeline";
      messageId = message;
      follow = false;
    }
  }
}
function modalKey(key: ReadonlyWire<KeyWithModifier["bare_key"]>, char: string): boolean {
  if (review) {
    if (key === "Esc") review = undefined;
    else if (char === "y" && reviewVisible) {
      const current = review;
      review = undefined;
      reviewVisible = false;
      void action({
        Resolve: { submission_id: current.submission, expect_rev: current.revision },
      }).catch(failed);
    }
  } else if (query !== undefined) {
    if (key === "Esc") query = undefined;
    else if (key === "Enter") doSearch();
    else if (key === "Backspace") query = Array.from(query).slice(0, -1).join("");
    else if (char) query += char;
  } else if (help) {
    if (char === "?" || key === "Esc") help = false;
  } else return false;
  return true;
}
function endKey(): void {
  if (!reader && region === "timeline" && view === "conversation") {
    follow = true;
    runIndex = 0;
    loadConversation(true);
  } else move(100000);
}
function escape(): void {
  if (reader) reader = undefined;
  else if (hits) {
    select(selected);
    notice = "";
  } else region = "roster";
}
function handleKey(key: ReadonlyWire<KeyWithModifier["bare_key"]>): void {
  const char = typeof key === "object" && "Char" in key ? key.Char : "";
  if (modalKey(key, char)) return;
  // oxlint-disable-next-line typescript/switch-exhaustiveness-check -- Unbound keys intentionally fall through to character shortcuts.
  switch (key) {
    case "Esc":
      escape();
      break;
    case "Tab":
      if (!reader) region = region === "roster" ? "timeline" : "roster";
      break;
    case "Down":
      move(1);
      break;
    case "Up":
      move(-1);
      break;
    case "PageDown":
      move(pageSize);
      break;
    case "PageUp":
      move(-pageSize);
      break;
    case "Home":
      move(-100000);
      break;
    case "End":
      endKey();
      break;
    case "Enter":
      open();
      break;
    default:
      if (char) characterKey(char);
  }
}
export default definePlugin({
  load() {
    z.subscribe(["Visible", "Key", "Mouse"]);
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
    try {
      if ("Mouse" in event) {
        mouseKey(event.Mouse);
        return true;
      }
      if ("Key" in event) {
        handleKey(event.Key.bare_key);
        z.requestRender();
        return true;
      }
    } catch (cause) {
      failed(cause);
    }
    return false;
  },
  unload() {
    visible = false;
    timer?.cancel();
  },
  render(rows, columns) {
    return renderPanel(rows, columns)
      .split("\n")
      .map((line, index) => `\u001B[${index + 1};1H${line}`)
      .join("");
  },
});
