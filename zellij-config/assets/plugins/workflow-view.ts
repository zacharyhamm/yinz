// View of authoritative Swamp history. Execution belongs to the managed Swamp server.
import z, {
  type Event,
  type PipeMessage,
  type FormWidget,
  type FieldDescriptor,
  type MarkdownComponent,
  type WidgetInteraction,
  type Json,
} from "zellij:plugin";
function required<T>(value: T): NonNullable<T> {
  if (value == null) throw new Error("Missing required plugin state");
  return value;
}

type Question = {
  model: string;
  version: number;
  attributes: {
    run: string;
    question: string;
    state: string;
    kind: string;
    prompt: string;
    answers?: string[];
  };
};
type History = {
  id?: string;
  workflowName?: string;
  status: string;
  jobs?: { name: string; steps?: { name: string; status: string }[] }[];
};
type Output = {
  output?: string;
  previewTruncated?: boolean;
  outputTruncated?: boolean;
  logError?: string;
  summaryLength?: number;
  outputComplete?: boolean;
  passed?: boolean;
  pane?: number;
  log?: string;
  outcome?: string;
  exitCode?: number | null;
  findings?: { severity: string; file: string; message: string }[];
  review?: Output;
  test?: Output;
  problem?: string;
  reason?: string;
  stdout?: string;
  stderr?: string;
  evidence?: { source: { commit: string }; target: { commit: string } };
  source?: { commit: string };
  target?: { commit: string };
};
type ReportEntry = {
  model: string;
  name: string;
  version: number;
  createdAt?: string;
  attributes: Output;
};
type Snapshot = {
  activeOperation?: { work: string; key: string } | null;
  run?: string;
  launch?: string;
  root: string;
  history: History;
  children?: History[];
  active?: boolean;
  questions?: { results?: Question[] };
  outputs?: { results?: ReportEntry[] };
  logs?: string;
  subtitle?: string;
};
type Attachment = { directory: string; run?: string | undefined; launch?: string | undefined };
type Block = {
  widget?: MarkdownComponent;
  lines?: string[];
  count: number;
  scroll: number;
  important?: boolean;
  live?: boolean;
};
export const view = {
  status: "No run attached",
  scroll: 0,
  pending: [] as Question[],
  selected: 0,
  active: false,
  cancellable: false,
  submitting: false,
  refreshing: false,
  selecting: false,
  dirty: false,
  formY: 0,
  attachment: undefined as Attachment | undefined,
  snapshot: undefined as Snapshot | undefined,
  notice: undefined as string | undefined,
  noticeError: false,
  refreshError: undefined as string | undefined,
  action: undefined as string | undefined,
  load,
  watch,
  attach,
  refresh,
  pipe,
  update,
  render,
  focusedControl: (): string | null => {
    controls();
    return required(form).focusedId();
  },
};
let form: FormWidget | undefined,
  onFork: (() => void) | undefined,
  onRestart: (() => void) | undefined;
let contentHeight = 0,
  formHasInput = false,
  formHeight = 1,
  formKey: string | undefined,
  questionKey: string | undefined;
const answered = new Map<string, number>();
let generation = 0,
  refreshQueued = false;
let submission: { cancel(): void } | undefined;
let questionReadyAt: number | undefined, renderedQuestion: string | undefined;
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function timing(phase: string, started: number, entry?: Question): void {
  z.log(
    "swamp_timing " +
      JSON.stringify({
        layer: "deno_view",
        phase,
        elapsedMs: z.monotonicMs() - started,
        launch,
        run: entry?.attributes.run ?? view.attachment?.run,
        question: entry?.attributes.question,
      }),
  );
}
type Report = {
  chunks: string[];
  offset: number;
  total: number;
  loading?: boolean | undefined;
  error?: string | undefined;
  width?: number | undefined;
  rendered?: number | undefined;
  lines?: string[] | undefined;
};
const reports = new Map<string, Report>();
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
const reportKey = (entry: ReportEntry): string =>
  JSON.stringify([entry.model, entry.name, entry.version]);
function reportEntries(): ReportEntry[] {
  return (view.snapshot?.outputs?.results ?? [])
    .filter((entry) => (entry.attributes.summaryLength ?? 0) > 0)
    .sort(
      (a, b) =>
        (b.createdAt ?? "").localeCompare(a.createdAt ?? "") ||
        reportKey(b).localeCompare(reportKey(a)),
    );
}
function loadReports(revision: number): void {
  for (const entry of reportEntries()) {
    const key = reportKey(entry);
    let report = reports.get(key);
    if (!report) {
      report = { chunks: [], offset: 0, total: required(entry.attributes.summaryLength) };
      reports.set(key, report);
      followLive = false;
      view.scroll = 0;
    }
    if (!report.loading && report.offset < report.total)
      void loadReport(entry, report, revision, required(view.snapshot).root).catch(
        (error: unknown) => {
          z.log(String(error));
        },
      );
  }
}
async function loadReport(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  entry: ReportEntry,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  report: Report,
  revision: number,
  root: string,
): Promise<void> {
  report.loading = true;
  try {
    // oxlint-disable-next-line eslint/no-unmodified-loop-condition -- The awaited callback changes this shared state while the loop is suspended.
    while (report.offset < report.total && revision === generation) {
      // oxlint-disable-next-line eslint/no-await-in-loop, typescript/no-unsafe-type-assertion -- These operations must complete in order before the next state transition. This conversion restores the known host or stored wire type after JSON transport.
      const chunk = (await z.rpc.call({
        pluginUrl: "zellij:swamp-bridge",
        name: "swamp",
        timeoutMs: 30000,
        payload: {
          method: "workflow_report",
          directory: root,
          model: entry.model,
          name: entry.name,
          version: entry.version,
          offset: report.offset,
        },
      })) as { text: string; next: number; total: number };
      if (revision !== generation) return;
      if (
        typeof chunk.text !== "string" ||
        !Number.isSafeInteger(chunk.next) ||
        chunk.next <= report.offset ||
        chunk.next > report.total ||
        chunk.total !== report.total ||
        chunk.text.length !== chunk.next - report.offset
      )
        throw new Error("Invalid saved report chunk");
      report.chunks.push(chunk.text);
      report.offset = chunk.next;
      report.error = undefined;
      repaint();
    }
  } catch (error) {
    if (revision === generation) {
      report.error = String(error);
      repaint();
    }
  } finally {
    report.loading = false;
  }
}
type Stream = {
  operation: string;
  source: string;
  raw: Uint8Array;
  value: string;
  omitted: boolean;
};
let streams: Stream[] = [];
let latestStream: Stream | undefined;
let launch: string | undefined;
let detached = false,
  followLive = true,
  scrollLimit = 0;
function setSubtitle(subtitle?: string): void {
  z.command("SetPaneSubtitle", {
    pane_id: { Plugin: z.getPluginIds().plugin_id },
    subtitle: !detached && subtitle ? subtitle : null,
  });
}
function watch(value?: string): void {
  if (value !== launch) {
    streams = [];
    latestStream = undefined;
    followLive = true;
  }
  launch = value;
  z.command("SubscribePaneOutput", value ? { swamp_work: value } : null);
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function output(operation: string, source: string, data: Uint8Array): void {
  let stream = streams.find((entry) => entry.operation === operation && entry.source === source);
  if (!stream) {
    stream = { operation, source, raw: new Uint8Array(), value: "", omitted: false };
    streams.push(stream);
  }
  latestStream = stream;
  const size = stream.raw.length + data.length;
  const raw = new Uint8Array(Math.min(size, 65536));
  const retained = Math.max(0, raw.length - data.length);
  raw.set(stream.raw.subarray(stream.raw.length - retained));
  raw.set(data.subarray(Math.max(0, data.length - raw.length)), retained);
  let start = 0;
  let end = raw.length;
  if (size > 65536) {
    stream.omitted = true;
    while (start < end && (raw[start] ?? 0) >= 128 && (raw[start] ?? 0) < 192) start += 1;
  }
  stream.raw = raw.slice(start);
  // Keep incomplete UTF-8 and escapes in raw until the next chunk completes them.
  let last = end - 1;
  while (last >= start && (raw[last] ?? 0) >= 128 && (raw[last] ?? 0) < 192) last -= 1;
  const first = raw[last] ?? 0,
    need = first >= 240 ? 4 : first >= 224 ? 3 : first >= 194 ? 2 : 1;
  if (last >= start && end - last < need) end = last;
  stream.value = text(
    z
      .decodeUtf8(raw.subarray(start, end))
      // oxlint-disable-next-line eslint/no-control-regex -- Terminal output contains ANSI control bytes that must be removed before display.
      .replaceAll(/\u001B\][^\u0007\u001B]*(?:\u0007|\u001B\\)/gu, "")
      // oxlint-disable-next-line eslint/no-control-regex -- Terminal output contains ANSI control bytes that must be removed before display.
      .replaceAll(/\u001B\[[0-?]*[ -/]*[@-~]/gu, "")
      // oxlint-disable-next-line eslint/no-control-regex -- Terminal output contains ANSI control bytes that must be removed before display.
      .replaceAll(/\u001B(?:\][^\u0007\u001B]*(?:\u001B)?|\[[0-?]*[ -/]*)?$/gu, "")
      // oxlint-disable-next-line eslint/no-control-regex -- Terminal output contains ANSI control bytes that must be removed before display.
      .replaceAll(/\u001B[@-_]/gu, ""),
  );
  repaint();
}
const text = (value: unknown): string =>
  z.text.sanitize(typeof value === "string" ? value : JSON.stringify(value ?? ""));
const quote = (value: string): string => "'" + value.replaceAll("'", "'\"'\"'") + "'";
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
const identity = (entry?: Question): string | undefined =>
  entry
    ? JSON.stringify([entry.attributes.run, entry.model, entry.attributes.question])
    : undefined;
const answerLabels: Record<string, string | undefined> = {
  "commit-and-rebase": "Commit & rebase",
  "fix-review-with-agent": "Fix review with agent",
  "fix-tests-with-agent": "Fix tests with agent",
};
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. Controls are listed in their on-screen order.
function controls(): void {
  const entry = view.pending[view.selected];
  const q = entry?.attributes;
  const key = identity(entry);
  const signature = JSON.stringify([
    key ?? "",
    q?.kind ?? "",
    q?.answers ?? [],
    view.pending.length > 1,
    view.active,
    view.cancellable,
    view.snapshot?.activeOperation,
  ]);
  if (signature === formKey) return;
  const focus = key === questionKey ? form?.focusedId() : undefined;
  const hasInput = !!q && !q.answers?.length;
  const value =
    key === questionKey && formHasInput && hasInput ? String(form?.value("answer") ?? "") : "";
  const fields: FieldDescriptor[] = [];
  if (q) {
    if (hasInput) {
      fields.push(
        { kind: "textInput", id: "answer", label: "Answer", value },
        { kind: "button", id: "submit", label: "Submit" },
      );
    } else {
      for (const [index, answer] of required(q.answers).entries()) {
        fields.push({
          kind: "button",
          id: `choice_${index + 1}`,
          label:
            answerLabels[answer] ??
            text(answer.replaceAll("-", " ").replace(/^[a-z]/u, (c) => c.toUpperCase())),
        });
      }
    }
  }
  if (view.pending.length > 1) fields.push({ kind: "button", id: "next", label: "Next question" });
  if (view.snapshot?.activeOperation)
    fields.push({ kind: "button", id: "cancel_step", label: "Cancel step" });
  if (view.cancellable) fields.push({ kind: "button", id: "cancel_run", label: "Cancel run" });
  if (onRestart && !q?.answers?.includes("restart"))
    fields.push({ kind: "button", id: "restart", label: "Restart agent" });
  if (onFork && !q?.answers?.includes("fork"))
    fields.push({ kind: "button", id: "fork", label: "Fork agent" });
  form = z.ui.form(fields);
  if (focus && fields.some((field) => field.id === focus)) form.focus(focus);
  formKey = signature;
  questionKey = key;
  formHasInput = hasInput;
  formHeight = hasInput ? 2 : fields.length > 0 ? 1 : 0;
}
function repaint(): void {
  if (view.selecting) view.dirty = true;
  else z.requestRender();
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together.
async function refresh(changed = true): Promise<void> {
  if (!view.attachment) return;
  if (view.refreshing || view.submitting) {
    if (changed) refreshQueued = true;
    return;
  }
  refreshQueued = false;
  view.refreshing = true;
  const started = z.swampTiming ? z.monotonicMs() : undefined;
  const revision = generation;
  const before = JSON.stringify([view.snapshot ?? {}, view.status, view.refreshError ?? ""]);
  try {
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
    const snapshot = (await z.rpc.call({
      pluginUrl: "zellij:swamp-bridge",
      name: "swamp",
      timeoutMs: 30000,
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
      payload: JSON.parse(JSON.stringify({ method: "workflow_show", ...view.attachment })),
    })) as unknown as Snapshot;
    if (revision !== generation) return;
    view.refreshError = undefined;
    view.snapshot = snapshot;
    view.cancellable = [snapshot.history, ...(snapshot.children ?? [])].some(
      (h) => h.status === "running",
    );
    view.active =
      !!snapshot.active ||
      [snapshot.history, ...(snapshot.children ?? [])].some((h) =>
        ["running", "suspended"].includes(h.status),
      );
    view.attachment.run = snapshot.run;
    if (snapshot.launch && snapshot.launch !== launch) watch(snapshot.launch);
    view.status = `Run ${snapshot.run ?? snapshot.launch}: ${snapshot.history.status}`;
    const selected = identity(view.pending[view.selected]);
    const latest = new Map<string, Question>();
    for (const entry of snapshot.questions?.results ?? []) {
      const key = required(identity(entry));
      if (!latest.has(key) || entry.version > required(latest.get(key)).version)
        latest.set(key, entry);
    }
    view.pending = [...latest.entries()]
      .filter(
        ([key, entry]) =>
          entry.attributes.state === "pending" && entry.version > (answered.get(key) ?? -Infinity),
      )
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([, entry]) => entry);
    view.selected = Math.min(view.selected, Math.max(0, view.pending.length - 1));
    const index = view.pending.findIndex((entry) => identity(entry) === selected);
    if (index >= 0) view.selected = index;
    questionReadyAt = z.swampTiming ? z.monotonicMs() : undefined;
    controls();
    loadReports(revision);
  } catch (error) {
    if (revision === generation) view.refreshError = String(error);
  } finally {
    if (revision === generation) {
      if (view.refreshError) {
        const name = view.snapshot?.history.workflowName;
        setSubtitle(name ? `${name} · status unavailable` : "Workflow · status unavailable");
        z.command("MessageToPlugin", {
          plugin_url: "zellij:swamp-bridge",
          destination_plugin_id: null,
          plugin_config: {},
          message_name: "workflow-refresh-failed",
          message_payload: launch ?? "",
          message_args: {},
          new_plugin_args: null,
          floating_pane_coordinates: null,
        });
      } else if (view.snapshot) setSubtitle(view.snapshot.subtitle);
    }
    view.refreshing = false;
    if (started != null) timing("refresh", started);
    if (before !== JSON.stringify([view.snapshot ?? {}, view.status, view.refreshError ?? ""]))
      repaint();
    if (revision !== generation || refreshQueued)
      void refresh().catch((error: unknown) => {
        z.log(String(error));
      });
  }
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function attach(attachment: Attachment): void {
  const current = view.attachment;
  if (
    current &&
    attachment.directory === current.directory &&
    (attachment.run || attachment.launch) &&
    (attachment.run === undefined || attachment.run === current.run) &&
    (attachment.launch === undefined || attachment.launch === (current.launch ?? launch))
  ) {
    current.launch ??= launch;
    detached = false;
    setSubtitle(view.snapshot?.subtitle ?? "Workflow · loading");
    void refresh().catch((error: unknown) => {
      z.log(String(error));
    });
    return;
  }
  submission?.cancel();
  submission = undefined;
  generation += 1;
  reports.clear();
  view.submitting = false;
  view.selecting = false;
  view.dirty = false;
  formKey = undefined;
  questionKey = undefined;
  watch(attachment.launch);
  view.attachment = { ...attachment };
  view.snapshot = undefined;
  view.pending = [];
  view.selected = 0;
  view.scroll = 0;
  view.action = undefined;
  view.notice = undefined;
  view.noticeError = false;
  view.refreshError = undefined;
  view.active = true;
  view.status = "Loading workflow...";
  detached = false;
  setSubtitle("Workflow · loading");
  void refresh().catch((error: unknown) => {
    z.log(String(error));
  });
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function load(options?: { onRestart?: () => void; onFork?: () => void }): void {
  onRestart = options?.onRestart;
  onFork = options?.onFork;
  formKey = undefined;
  z.subscribe(["Key", "Mouse", "PastedText", "PaneOutput", "CommandPaneExited"]);
  z.command("SetSelfMouseSelectionSupport", true);
  controls();
  z.every(2000, async () => refresh(false));
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function pipe(message: PipeMessage): void {
  if (message.name === "workflow-detached") {
    detached = true;
    setSubtitle();
    return;
  }
  if (message.name === "workflow-changed") {
    void refresh().catch((error: unknown) => {
      z.log(String(error));
    });
    return;
  }
  if (message.name !== "show" || !message.payload) return;
  const value: unknown = JSON.parse(message.payload);
  if (
    value == null ||
    typeof value !== "object" ||
    !("directory" in value) ||
    typeof value.directory !== "string"
  )
    throw new Error("Invalid workflow attachment");
  const run = "run" in value ? (value.run ?? undefined) : undefined;
  // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
  const launch = "launch" in value ? (value.launch ?? undefined) : undefined;
  if (
    (run !== undefined && typeof run !== "string") ||
    (launch !== undefined && typeof launch !== "string")
  )
    throw new Error("Invalid workflow identity");
  attach({ directory: value.directory, run, launch });
  z.command("ShowSelf", true);
  if (typeof message.source === "object" && "Cli" in message.source) {
    z.command("CliPipeOutput", [message.source.Cli, JSON.stringify({ attached: true })]);
    z.command("UnblockCliPipeInput", message.source.Cli);
  }
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function, typescript/prefer-readonly-parameter-types -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together. This helper shares mutable plugin state and SDK wire types with its callers.
function update(event: Event): boolean {
  controls();
  if (typeof event !== "object") return false;
  if ("PaneOutput" in event) {
    const { context, pane_id, data } = event.PaneOutput;
    if (launch && context["swamp_work"] === launch)
      // oxlint-disable-next-line typescript/prefer-nullish-coalescing -- An empty wire string needs the same display fallback as an absent value.
      output(context["swamp_operation"] || "command", `pane ${pane_id}`, data);
    return false;
  }
  if ("CommandPaneExited" in event) {
    if (launch && event.CommandPaneExited[2]["swamp_work"] === launch)
      void refresh().catch((error: unknown) => {
        z.log(String(error));
      });
    return false;
  }
  if ("Key" in event && event.Key.bare_key === "End" && required(form).focusedId() !== "answer") {
    followLive = true;
    view.scroll = Infinity;
    return true;
  }
  if ("Key" in event && event.Key.bare_key === "Home" && required(form).focusedId() !== "answer") {
    followLive = false;
    view.scroll = 0;
    return true;
  }
  if ("Key" in event && event.Key.bare_key === "PageUp") {
    followLive = false;
    view.scroll = Math.max(0, view.scroll - contentHeight);
    return true;
  }
  if ("Key" in event && event.Key.bare_key === "PageDown") {
    view.scroll += contentHeight;
    followLive = view.scroll >= scrollLimit && latestStream === streams.at(-1);
    return true;
  }
  if ("Mouse" in event) {
    const mouse = event.Mouse;
    if ("ScrollUp" in mouse) {
      followLive = false;
      view.scroll = Math.max(0, view.scroll - Math.min(Number(mouse.ScrollUp), contentHeight));
      return true;
    }
    if ("ScrollDown" in mouse) {
      view.scroll += Math.min(Number(mouse.ScrollDown), contentHeight);
      followLive = view.scroll >= scrollLimit && latestStream === streams.at(-1);
      return true;
    }
    if ("LeftClick" in mouse) view.selecting = Number(mouse.LeftClick[0]) < view.formY;
    if (view.selecting) {
      if ("Release" in mouse) {
        view.selecting = false;
        if (view.dirty) {
          view.dirty = false;
          repaint();
        }
      }
      return false;
    }
    const position: unknown = Object.values(mouse)[0];
    if (!("Release" in mouse) && (!Array.isArray(position) || Number(position[0]) < view.formY))
      return false;
  }
  let action: WidgetInteraction;
  if ("Key" in event) action = required(form).handleKey(event.Key);
  else if ("Mouse" in event) action = required(form).handleMouse(event.Mouse);
  else if ("PastedText" in event) action = required(form).insertPastedText(event.PastedText);
  else return false;
  if ("id" in action && action.id === "next") {
    view.selected = (view.selected + 1) % view.pending.length;
    controls();
    return true;
  }
  if (action.kind !== "buttonPressed" || view.submitting) return action.kind !== "ignored";
  if (action.id === "restart" && onRestart) {
    onRestart();
    return true;
  }
  if (action.id === "fork" && onFork) {
    onFork();
    return true;
  }
  const entry = view.pending[view.selected];
  const choice = /^choice_(\d+)$/u.exec(action.id ?? "");
  let request: Record<string, Json>, success: string;
  if (entry && (action.id === "submit" || choice)) {
    if (entry.version <= (answered.get(required(identity(entry))) ?? -Infinity)) return false;
    const q = entry.attributes;
    const answer = choice
      ? required(required(q.answers)[Number(choice[1]) - 1])
      : String(required(form).value("answer"));
    request = {
      method: "workflow_answer",
      directory: required(view.snapshot).root,
      model: entry.model,
      inputs: {
        run: q.run,
        question: q.question,
        answer,
        submission: `${z.instanceId}-${generation + 1}`,
      },
    };
    success = "Answer recorded";
  } else if (action.id === "cancel_step" && view.snapshot?.activeOperation) {
    const active = view.snapshot.activeOperation;
    request = { method: "cancel_step", work: active.work, operation: active.key };
    success = "Step cancellation requested; waiting for verified termination";
  } else if (action.id === "cancel_run" && view.snapshot && view.attachment?.run) {
    request = {
      method: "workflow_cancel",
      directory: view.snapshot.root,
      run: view.attachment.run,
    };
    success = "Cancellation requested; resources preserved";
  } else return true;
  generation += 1;
  view.submitting = true;
  view.noticeError = false;
  view.action = choice
    ? required(required(entry).attributes.answers)[Number(choice[1]) - 1]
    : action.id;
  view.notice =
    success === "Answer recorded" ? "Submitting answer..." : "Requesting cancellation...";
  void submit(request, success, required(view.attachment), entry).catch((error: unknown) => {
    z.log(String(error));
  });
  return true;
}
// oxlint-disable-next-line eslint/max-lines-per-function -- This lifecycle handler keeps its ordered protocol transitions together.
async function submit(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  request: Record<string, Json>,
  success: string,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  attachment: Attachment,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  entry?: Question,
): Promise<void> {
  const answering = request["method"] === "workflow_answer";
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
  const operation = `${text(answering ? identity(entry) : request["run"])}: ${text(answering ? (request["inputs"] as Record<string, Json>)["answer"] : "cancel_run")}`;
  let timedOut = false,
    timer: { cancel(): void } | undefined;
  const startDeadline = (): void => {
    timer?.cancel();
    timer = z.after(90000, () => {
      timedOut = true;
      pending.cancel();
    });
  };
  const pending = z.rpc.call({
    pluginUrl: "zellij:swamp-bridge",
    name: "swamp",
    payload: request,
    timeoutMs: answering ? undefined : 90000,
    onProgress: answering
      ? (payload): void => {
          const status =
            payload != null && typeof payload === "object" && !Array.isArray(payload)
              ? payload["status"]
              : undefined;
          if (status === "waiting" || status === "reloading") {
            timer?.cancel();
            timer = undefined;
          } else if (status === "ready") startDeadline();
        }
      : undefined,
  });
  const activeSubmission = {
    cancel(): void {
      timer?.cancel();
      pending.cancel();
    },
  };
  submission = activeSubmission;
  try {
    if (answering) startDeadline();
    await pending;
    if (view.attachment !== attachment) return;
    view.noticeError = false;
    view.notice = success;
    if (answering && entry) {
      answered.set(required(identity(entry)), entry.version);
      // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
      view.pending = view.pending.filter((pending) => identity(pending) !== identity(entry));
      view.selected = Math.min(view.selected, Math.max(0, view.pending.length - 1));
      controls();
    }
  } catch (error) {
    if (view.attachment === attachment) {
      view.noticeError = true;
      view.notice = timedOut ? "TimeoutError: RPC timed out: swamp" : String(error);
    }
  } finally {
    timer?.cancel();
    if (submission === activeSubmission) submission = undefined;
    if (view.attachment === attachment) {
      if (view.noticeError)
        output(operation, "submission error", z.encodeUtf8(required(view.notice)));
      view.submitting = false;
      repaint();
      void refresh().catch((error: unknown) => {
        z.log(String(error));
      });
    }
  }
}
let markdownCache: { source: string; dim: boolean; widget: MarkdownComponent }[] = [];
const failed = (status?: string): boolean => status === "failed" || status === "error";
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together.
function render(rows: number, cols: number): string {
  const started = z.swampTiming ? z.monotonicMs() : undefined;
  controls();
  const blocks: Block[] = [],
    cache: typeof markdownCache = [];
  function markdown(source: string, dim: boolean): Block {
    const previous = markdownCache[cache.length];
    const widget =
      previous?.source === source && previous.dim === dim
        ? previous.widget
        : z.ui.markdown(source, { dim });
    cache.push({ source, dim, widget });
    return {
      widget,
      count: widget.render({ x: 0, y: 0, width: cols, height: 0 }).totalRows,
      scroll: 0,
    };
  }
  function add(source: unknown, important = false, isMarkdown = false): void {
    if (isMarkdown)
      blocks.push(
        markdown(typeof source === "string" ? source : JSON.stringify(source ?? ""), !important),
      );
    else {
      const lines = z.text.wrap(text(source), Math.max(1, cols));
      blocks.push({ lines, count: lines.length, important, scroll: 0 });
    }
  }
  const entry = view.pending[view.selected],
    question = entry ? markdown(entry.attributes.prompt, false) : undefined;
  add(view.status, failed(view.snapshot?.history.status));
  if (view.notice) add(view.notice, view.noticeError);
  if (view.refreshError) add(view.refreshError, true);
  if (!entry && view.active) add("Waiting for the workflow. No question pending.");
  const snapshot = view.snapshot;
  if (snapshot) {
    // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
    for (const entry of reportEntries()) {
      const report = reports.get(reportKey(entry));
      if (!report) continue;
      // Test output is plain text: preserve failure blocks without interpreting UI markup.
      if (report.width !== cols || report.rendered !== report.offset) {
        report.lines = z.text.wrap(text(report.chunks.join("")), Math.max(1, cols));
        report.width = cols;
        report.rendered = report.offset;
      }
      blocks.push({
        lines: required(report.lines),
        count: required(report.lines).length,
        important: entry.attributes.passed === false,
        scroll: 0,
      });
      if (report.error) add("Report unavailable: " + report.error, true);
      else if (report.offset < report.total) add("Loading complete test report...");
    }
    add("Execution details:");
    for (const history of [snapshot.history, ...(snapshot.children ?? [])]) {
      add(
        `${text(history.workflowName)} / ${text(history.id)}: ${text(history.status)}`,
        failed(history.status),
      );
      for (const job of history.jobs ?? [])
        for (const step of job.steps ?? [])
          add(`${job.name}/${step.name}: ${step.status}`, failed(step.status));
    }
    // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
    for (const entry of snapshot.outputs?.results ?? []) {
      add(`${entry.model}/${entry.name} v${entry.version}`);
      const attrs = entry.attributes ?? {},
        review = attrs.review ?? attrs;
      if (attrs.problem) add(attrs.problem, true);
      if (attrs.reason) add(attrs.reason, true);
      if (attrs.stderr) add(attrs.stderr, true);
      if (attrs.stdout) add(attrs.stdout);
      for (const finding of review.findings ?? [])
        add(`${finding.severity} ${finding.file}: ${finding.message}`, true);
      if (attrs.pane != null)
        add(
          `Pane ${attrs.pane} / ${text(attrs.log)} / ${text(attrs.outcome)}`,
          failed(attrs.outcome),
        );
      // oxlint-disable-next-line eslint/no-inner-declarations, typescript/prefer-readonly-parameter-types -- This traversal uses the render call's local list and visited set. This helper shares mutable plugin state and SDK wire types with its callers.
      function showOutput(value: Output, path: string): void {
        if (value.output == null) return;
        if (value.previewTruncated) {
          add("Output preview (up to 16,384 characters). Retrieve the complete saved version:");
          const predicate = `modelName == ${JSON.stringify(entry.model)} && name == ${JSON.stringify(entry.name)} && version == ${entry.version}`;
          add(
            `swamp data query ${quote(predicate)} --select ${path} --json --repo-dir ${quote(required(snapshot).root)}`,
          );
        }
        if (value.outputTruncated) add("Retained command scrollback (may omit older output):");
        add(value.output, false, true);
      }
      showOutput(attrs, "attributes.output");
      if (attrs.logError) add(`Log unavailable: ${text(attrs.logError)}`, true);
      if (attrs.test) {
        add(
          `Gate ${text(attrs.test.outcome)} / exit ${text(attrs.test.exitCode)} / log ${text(attrs.test.log)}`,
          failed(attrs.test.outcome) ||
            (typeof attrs.test.exitCode === "number" && attrs.test.exitCode !== 0),
        );
        showOutput(attrs.test, "attributes.test.output");
      }
      const evidence = attrs.evidence ?? (attrs.source ? attrs : undefined);
      if (evidence)
        add(`Source ${text(evidence.source?.commit)} / target ${text(evidence.target?.commit)}`);
    }
    add("Workflow log (last 100 lines):");
    add(snapshot.logs ?? "", false, true);
  }
  for (const stream of streams) {
    add(stream.value);
    add(
      `Live ${stream.operation} / ${stream.source}${stream.omitted ? " [older output omitted]" : ""} (End: live)`,
    );
    required(blocks.at(-1)).live = stream === latestStream;
  }
  markdownCache = cache;
  const noticeHeight = view.notice && rows >= 3 ? 1 : 0;
  const height = Math.min(formHeight, Math.max(0, rows - noticeHeight - (rows >= 2 ? 1 : 0)));
  const footerHeight = question
    ? Math.min(question.count, Math.floor((rows - height - noticeHeight) / 2))
    : 0;
  if (question && question.count > footerHeight)
    blocks.unshift({ ...question, count: question.count - footerHeight, scroll: footerHeight });
  const separator = footerHeight > 0 && rows - height - noticeHeight - footerHeight >= 2 ? 1 : 0;
  let page = Math.max(0, rows - height - noticeHeight - footerHeight - separator);
  view.formY = rows - height;
  const total = blocks.reduce((sum, block) => sum + block.count, 0);
  const indicatorHeight = total > page && page >= 2 && cols > 0 ? 1 : 0;
  page -= indicatorHeight;
  contentHeight = page;
  scrollLimit = Math.max(0, total - page);
  if (followLive) {
    let end = 0;
    for (const block of blocks) {
      end += block.count;
      if (block.live) break;
    }
    view.scroll = Math.max(0, end - page);
  }
  view.scroll = Math.max(0, Math.min(view.scroll, scrollLimit));
  const frame: string[] = [];
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  function draw(block: Block, y: number, skip: number, count: number): void {
    if (block.widget)
      frame.push(
        block.widget.render({ x: 0, y, width: cols, height: count, scroll: block.scroll + skip })
          .text,
      );
    else
      for (let i = 0; i < count; i++) {
        const line = z.ui.text(required(block.lines)[skip + i]);
        frame.push(
          (block.important ? line.error() : line.dim()).render({
            x: 0,
            y: y + i,
            width: cols,
            height: 1,
          }),
        );
      }
  }
  let skip = view.scroll,
    y = 0;
  for (const block of blocks) {
    const count = Math.min(Math.max(0, block.count - skip), page - y);
    if (count > 0) {
      draw(block, y, skip, count);
      y += count;
    }
    skip = Math.max(0, skip - block.count);
  }
  if (indicatorHeight) {
    const above = view.scroll > 0 ? `↑ ${view.scroll} more` : "";
    const below = total - view.scroll - page;
    const label = [above, below > 0 ? `↓ ${below} more` : ""].filter(Boolean).join("  ");
    frame.push(
      z.ui
        .text(z.text.truncate(label, cols))
        .dim()
        .render({ x: 0, y: page, width: cols, height: 1 }),
    );
  }
  if (question && footerHeight > 0)
    draw(question, page + indicatorHeight + separator, 0, footerHeight);
  if (noticeHeight) {
    const notice = z.ui.text(
      z.text.truncate(text(`${view.action ? view.action + ": " : ""}${view.notice}`), cols),
    );
    frame.push(
      (view.noticeError ? notice.error() : notice).render({
        x: 0,
        y: view.formY - noticeHeight,
        width: cols,
        height: 1,
      }),
    );
  }
  const rendered =
    frame.join("") + required(form).render({ x: 0, y: view.formY, width: cols, height });
  const key = started != null && entry ? identity(entry) + ":" + entry.version : undefined;
  if (started != null && key && key !== renderedQuestion) {
    timing("question_render", started, entry);
    timing("question_ready_to_render", questionReadyAt ?? started, entry);
  }
  renderedQuestion = key;
  return rendered;
}
