// View of authoritative Swamp history. Execution belongs to the managed Swamp server.
import z, {
  type Event,
  type PipeMessage,
  type FormWidget,
  type FieldDescriptor,
  type MarkdownComponent,
  type WidgetInteraction,
  type Json,
} from "yinz:plugin";
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
    answer?: string | null;
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
  recommendation?: string;
  tests?: string;
  codeReview?: string;
  ready?: boolean;
  integrated?: boolean;
  removed?: boolean;
  stdout?: string;
  stderr?: string;
  evidence?: { source: Revision; target: Revision };
  source?: Revision;
  target?: Revision;
};
type Revision = { commit: string; branch?: string; checkout?: string };
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
  endpoint?: string;
  problem?: string;
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
  reconciling: false,
  refreshing: false,
  selecting: false,
  dirty: false,
  formY: 0,
  attachment: undefined as Attachment | undefined,
  snapshot: undefined as Snapshot | undefined,
  notice: undefined as string | undefined,
  noticeError: false,
  details: false,
  readingQuestion: false,
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
let uncertainQuestion: string | undefined;
let questionReadyAt: number | undefined, renderedQuestion: string | undefined;
let controlLabels = new Map<string, string>();
function histories(): History[] {
  return view.snapshot
    ? [
        { ...view.snapshot.history, id: view.snapshot.history.id ?? view.snapshot.run ?? "" },
        ...(view.snapshot.children ?? []),
      ]
    : [];
}
function running(): History[] {
  return histories().filter(
    (history) =>
      history.status === "running" && typeof history.id === "string" && history.id.length > 0,
  );
}
function newestOutputs(): ReportEntry[] {
  return [...(view.snapshot?.outputs?.results ?? [])].sort(
    (a, b) => (b.createdAt ?? "").localeCompare(a.createdAt ?? "") || b.version - a.version,
  );
}
function checks(): Output | undefined {
  return newestOutputs().find(
    (entry) =>
      typeof entry.attributes.codeReview === "string" && typeof entry.attributes.tests === "string",
  )?.attributes;
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Reads the shared history wire type without modifying it.
function activity(history: History): string {
  for (const job of history.jobs ?? [])
    for (const step of job.steps ?? [])
      if (["running", "waiting_approval"].includes(step.status))
        return step.name.replaceAll("-", " ");
  return history.workflowName ?? "Workflow";
}
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
  const entries = (view.snapshot?.outputs?.results ?? [])
    .filter((entry) => (entry.attributes.summaryLength ?? 0) > 0)
    .sort(
      (a, b) =>
        (b.createdAt ?? "").localeCompare(a.createdAt ?? "") ||
        reportKey(b).localeCompare(reportKey(a)),
    );
  return view.details ? entries : entries.slice(0, 1);
}
// A view opened on a run starts at its reports; one that was already showing the run stays at the
// bottom, which is where a workflow step that produced the report hands it back.
function loadReports(revision: number, initial: boolean): void {
  for (const entry of reportEntries()) {
    const key = reportKey(entry);
    let report = reports.get(key);
    if (!report) {
      report = { chunks: [], offset: 0, total: required(entry.attributes.summaryLength) };
      reports.set(key, report);
      if (initial) {
        followLive = false;
        view.scroll = 0;
      }
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
        pluginUrl: "yinz:swamp-bridge",
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
// pinBottom keeps a returned view at the bottom while reports still loading grow above it, until
// the developer scrolls.
let detached = false,
  followLive = true,
  pinBottom = false,
  scrollLimit = 0;
let outputPosition = { scroll: 0, follow: true };
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
  const activeRuns = running();
  const mergeBlocked = checks()?.ready === false;
  const answerBlocked =
    view.reconciling ||
    (uncertainQuestion != null && uncertainQuestion === key && q?.state === "pending");
  const signature = JSON.stringify([
    key ?? "",
    q?.kind ?? "",
    q?.answers ?? [],
    q?.state,
    view.pending.length > 1,
    view.active,
    view.cancellable,
    view.snapshot?.activeOperation,
    activeRuns.map((history) => [history.id, activity(history)]),
    mergeBlocked,
    view.details,
    view.readingQuestion,
    answerBlocked,
  ]);
  if (signature === formKey) return;
  const focus = key === questionKey ? form?.focusedId() : undefined;
  const hasInput = !!q && q.state !== "recorded" && !q.answers?.length;
  const value =
    key === questionKey && formHasInput && hasInput ? String(form?.value("answer") ?? "") : "";
  const fields: FieldDescriptor[] = [];
  if (q) {
    if (q.state === "recorded") {
      fields.push({
        kind: "button",
        id: "recover",
        label: "Recover saved answer",
        disabled: answerBlocked,
      });
    } else if (hasInput) {
      fields.push(
        { kind: "textInput", id: "answer", label: "Answer", value },
        { kind: "button", id: "submit", label: "Submit", disabled: answerBlocked },
      );
    } else {
      for (const [index, answer] of required(q.answers).entries()) {
        fields.push({
          kind: "button",
          id: `choice_${index + 1}`,
          label:
            answerLabels[answer] ??
            text(answer.replaceAll("-", " ").replace(/^[a-z]/u, (c) => c.toUpperCase())),
          disabled: answerBlocked || (answer === "merge" && mergeBlocked),
        });
      }
    }
  }
  if (view.pending.length > 1) fields.push({ kind: "button", id: "next", label: "Next question" });
  if (view.snapshot?.activeOperation)
    fields.push({ kind: "button", id: "cancel_step", label: "Cancel step" });
  for (const history of activeRuns)
    fields.push({
      kind: "button",
      id: `cancel_run_${history.id}`,
      label:
        activeRuns.length === 1
          ? "Cancel run"
          : `Cancel ${activity(history)} (${(history.id ?? "").slice(0, 8)})`,
    });
  if (onRestart && !q?.answers?.includes("restart"))
    fields.push({ kind: "button", id: "restart", label: "Restart agent" });
  if (onFork && !q?.answers?.includes("fork"))
    fields.push({ kind: "button", id: "fork", label: "Fork agent" });
  if (q)
    fields.push({
      kind: "button",
      id: "question",
      label: view.readingQuestion ? "Back to output" : "Read question",
    });
  if (view.snapshot)
    fields.push({
      kind: "button",
      id: "details",
      label: view.details ? "Hide details" : "Details",
    });
  controlLabels = new Map(fields.map((field) => [field.id, field.label ?? field.id]));
  form = z.ui.form(fields);
  if (focus && fields.some((field) => field.id === focus)) form.focus(focus);
  else if (focus?.startsWith("cancel_run_") && fields.some((field) => field.id === "details"))
    form.focus("details");
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
      pluginUrl: "yinz:swamp-bridge",
      name: "swamp",
      timeoutMs: 30000,
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
      payload: JSON.parse(JSON.stringify({ method: "workflow_show", ...view.attachment })),
    })) as unknown as Snapshot;
    if (revision !== generation) return;
    view.refreshError = undefined;
    const initial = view.snapshot === undefined;
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
          ["pending", "recorded"].includes(entry.attributes.state) &&
          entry.version > (answered.get(key) ?? -Infinity),
      )
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([, entry]) => entry);
    view.reconciling = false;
    if (uncertainQuestion) {
      const saved = latest.get(uncertainQuestion);
      if (saved && saved.attributes.state !== "pending") uncertainQuestion = undefined;
      else {
        view.noticeError = true;
        view.notice =
          "Answer delivery is uncertain. Waiting for saved state; do not submit another answer.";
      }
    }
    view.selected = Math.min(view.selected, Math.max(0, view.pending.length - 1));
    const index = view.pending.findIndex((entry) => identity(entry) === selected);
    if (index >= 0) view.selected = index;
    if (identity(view.pending[view.selected]) !== selected) view.readingQuestion = false;
    questionReadyAt = z.swampTiming ? z.monotonicMs() : undefined;
    controls();
    loadReports(revision, initial);
  } catch (error) {
    if (revision === generation) view.refreshError = String(error);
  } finally {
    if (revision === generation) {
      if (view.refreshError) {
        const name = view.snapshot?.history.workflowName;
        const endpoint = view.snapshot?.endpoint;
        setSubtitle(
          `${endpoint ? `${endpoint} · ` : ""}${name ?? "Workflow"} · status unavailable`,
        );
        z.command("MessageToPlugin", {
          plugin_url: "yinz:swamp-bridge",
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
  view.reconciling = false;
  uncertainQuestion = undefined;
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
  view.details = false;
  view.readingQuestion = false;
  pinBottom = false;
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
  if (message.name === "workflow-returned") {
    // A workflow step handed the pane back: show the tail, following it when End would.
    view.scroll = Infinity;
    followLive = latestStream === streams.at(-1);
    pinBottom = true;
    repaint();
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
    pinBottom = false;
    followLive = true;
    view.scroll = Infinity;
    return true;
  }
  if ("Key" in event && event.Key.bare_key === "Home" && required(form).focusedId() !== "answer") {
    pinBottom = false;
    followLive = false;
    view.scroll = 0;
    return true;
  }
  if ("Key" in event && event.Key.bare_key === "PageUp") {
    pinBottom = false;
    followLive = false;
    view.scroll = Math.max(0, view.scroll - contentHeight);
    return true;
  }
  if ("Key" in event && event.Key.bare_key === "PageDown") {
    pinBottom = false;
    view.scroll += contentHeight;
    followLive = view.scroll >= scrollLimit && latestStream === streams.at(-1);
    return true;
  }
  if ("Mouse" in event) {
    const mouse = event.Mouse;
    if ("ScrollUp" in mouse) {
      pinBottom = false;
      followLive = false;
      view.scroll = Math.max(0, view.scroll - Math.min(Number(mouse.ScrollUp), contentHeight));
      return true;
    }
    if ("ScrollDown" in mouse) {
      pinBottom = false;
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
  if (action.kind === "buttonPressed" && action.id === "details") {
    view.details = !view.details;
    view.readingQuestion = false;
    view.scroll = 0;
    followLive = false;
    pinBottom = false;
    controls();
    loadReports(generation, false);
    return true;
  }
  if (action.kind === "buttonPressed" && action.id === "question") {
    if (!view.readingQuestion) outputPosition = { scroll: view.scroll, follow: followLive };
    view.readingQuestion = !view.readingQuestion;
    view.scroll = view.readingQuestion ? 0 : outputPosition.scroll;
    followLive = view.readingQuestion ? false : outputPosition.follow;
    pinBottom = false;
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
    if (view.reconciling || (uncertainQuestion != null && uncertainQuestion === identity(entry)))
      return false;
    if (entry.version <= (answered.get(required(identity(entry))) ?? -Infinity)) return false;
    const q = entry.attributes;
    const answer = choice
      ? required(required(q.answers)[Number(choice[1]) - 1])
      : String(required(form).value("answer"));
    if (!answer.trim() || (answer === "merge" && checks()?.ready === false)) return false;
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
  } else if (entry?.attributes.state === "recorded" && action.id === "recover") {
    if (view.reconciling) return false;
    request = {
      method: "workflow_recover",
      directory: required(view.snapshot).root,
      model: entry.model,
      inputs: { run: entry.attributes.run, question: entry.attributes.question },
    };
    success = "Recorded answer delivered";
  } else if (action.id === "cancel_step" && view.snapshot?.activeOperation) {
    const active = view.snapshot.activeOperation;
    request = { method: "cancel_step", work: active.work, operation: active.key };
    success = "Step cancellation requested; waiting for verified termination";
  } else if (action.id?.startsWith("cancel_run_") && view.snapshot) {
    const activeRuns = running();
    const target = activeRuns.find((history) => action.id === `cancel_run_${history.id}`);
    if (!target?.id) return false;
    request = {
      method: "workflow_cancel",
      directory: view.snapshot.root,
      run: target.id,
    };
    success = "Cancellation requested; resources preserved";
  } else return true;
  generation += 1;
  view.submitting = true;
  view.noticeError = false;
  view.action = choice
    ? required(required(entry).attributes.answers)[Number(choice[1]) - 1]
    : (controlLabels.get(action.id ?? "") ?? action.id);
  view.notice =
    success === "Answer recorded"
      ? "Submitting answer..."
      : action.id === "recover"
        ? "Recovering recorded answer..."
        : "Requesting cancellation...";
  void submit(request, success, required(view.attachment), entry).catch((error: unknown) => {
    z.log(String(error));
  });
  return true;
}
// oxlint-disable-next-line eslint/max-lines-per-function, eslint/complexity -- Keep answer admission, completion and error recovery in one lifecycle.
async function submit(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  request: Record<string, Json>,
  success: string,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  attachment: Attachment,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  entry?: Question,
): Promise<void> {
  const answering =
    request["method"] === "workflow_answer" || request["method"] === "workflow_recover";
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
    pluginUrl: "yinz:swamp-bridge",
    name: "swamp",
    payload: request,
    timeoutMs: answering ? undefined : 90000,
    onProgress: answering
      ? (payload): void => {
          if (view.attachment !== attachment) return;
          const status =
            payload != null && typeof payload === "object" && !Array.isArray(payload)
              ? payload["status"]
              : undefined;
          if (status === "waiting" || status === "reloading") {
            timer?.cancel();
            timer = undefined;
            view.notice =
              status === "waiting"
                ? "Waiting for active workflows before loading updated extensions..."
                : "Loading updated extensions...";
          } else if (status === "ready") {
            startDeadline();
            view.notice = "Delivering answer...";
          }
          repaint();
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
    const result = await pending;
    if (view.attachment !== attachment) return;
    view.noticeError = false;
    view.notice =
      result != null &&
      typeof result === "object" &&
      !Array.isArray(result) &&
      result["alreadyFinished"] === true
        ? "Run is no longer running; refreshed status"
        : success;
    if (answering && entry) {
      answered.set(required(identity(entry)), entry.version);
      // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
      view.pending = view.pending.filter((pending) => identity(pending) !== identity(entry));
      view.selected = Math.min(view.selected, Math.max(0, view.pending.length - 1));
      controls();
    }
  } catch (error) {
    if (view.attachment === attachment) {
      view.reconciling = answering;
      if (
        answering &&
        entry &&
        (timedOut || (error instanceof Error && error.name === "TimeoutError"))
      )
        uncertainQuestion = required(identity(entry));
      view.noticeError = true;
      view.notice = timedOut
        ? "Request timed out. Checking saved state before another action; a recorded answer can be recovered."
        : String(error);
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
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep the ordered rendering of summary, reports and execution details together.
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
  const currentChecks = checks();
  const currentOutputs = newestOutputs();
  const cleanup = currentOutputs.find(
    (saved) => typeof saved.attributes.removed === "boolean",
  )?.attributes;
  const integrated = currentOutputs.find(
    (saved) => typeof saved.attributes.integrated === "boolean",
  )?.attributes;
  const afterIntegration =
    !!cleanup || integrated?.integrated === true || entry?.attributes.kind === "cleanup";
  const currentEvidence =
    currentChecks?.evidence ??
    currentOutputs
      .map(
        (saved) =>
          saved.attributes.evidence ?? (saved.attributes.source ? saved.attributes : undefined),
      )
      .find((value) => value?.source);
  const activeRuns = running();
  const snapshot = view.snapshot;
  const header = snapshot
    ? currentEvidence?.source && currentEvidence.target
      ? // oxlint-disable-next-line typescript/prefer-nullish-coalescing -- Detached checkouts have an empty branch and must display the revision instead.
        `${currentEvidence.source.branch || currentEvidence.source.commit.slice(0, 8)} → ${currentEvidence.target.branch || currentEvidence.target.commit.slice(0, 8)}`
      : (snapshot.history.workflowName ?? "Workflow")
    : view.status;
  const summary = [header];
  if (snapshot) {
    summary.push(
      snapshot.problem
        ? `Error: ${snapshot.problem}`
        : failed(snapshot.history.status)
          ? `Attached run: ${snapshot.history.status}`
          : activeRuns.length > 0
            ? `Running: ${activeRuns.map((history) => activity(history)).join(", ")}`
            : entry?.attributes.state === "recorded"
              ? "Answer recorded; delivery needs recovery"
              : entry
                ? entry.attributes.kind === "cleanup"
                  ? "Integrated locally; choose whether to keep the worktree"
                  : "Waiting for your answer"
                : cleanup
                  ? cleanup.removed
                    ? "Worktree removed; branch and VM retained"
                    : "Worktree retained; cleanup did not complete"
                  : integrated?.integrated
                    ? "Integrated locally; worktree retained"
                    : `Attached run: ${snapshot.history.status}`,
    );
    if (!entry && activeRuns.length === 0 && !afterIntegration && snapshot.children?.length)
      summary.push(
        `Related runs: ${[...new Set(snapshot.children.map((history) => history.status))].join(", ")}`,
      );
    if (currentChecks && !afterIntegration) {
      summary.push(`Review: ${currentChecks.codeReview} · Tests: ${currentChecks.tests}`);
      if (
        entry &&
        !["merge", "cleanup"].includes(entry.attributes.kind) &&
        (currentChecks.recommendation || currentChecks.reason)
      )
        summary.push(currentChecks.recommendation ?? required(currentChecks.reason));
    }
  }
  if (view.refreshError) add(view.refreshError, true);
  if (snapshot?.problem) add(snapshot.problem, true);
  if (entry?.attributes.state === "recorded")
    add(
      `Saved answer: ${entry.attributes.answer ?? ""}. Recover this answer to finish its delivery without submitting again.`,
    );
  if (!afterIntegration && currentChecks?.ready === false && currentChecks.reason)
    add(`Merge unavailable: ${currentChecks.reason}`);
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
    if (view.details) add("Execution details:");
    for (const history of view.details ? histories() : []) {
      add(
        `${text(history.workflowName)} / ${text(history.id)}: ${text(history.status)}`,
        failed(history.status),
      );
      for (const job of history.jobs ?? [])
        for (const step of job.steps ?? [])
          add(`${job.name}/${step.name}: ${step.status}`, failed(step.status));
    }
    const recent = currentOutputs.find((saved) => {
      const a = saved.attributes;
      return (
        typeof a.removed === "boolean" ||
        typeof a.integrated === "boolean" ||
        [a.output, a.test, a.review, a.findings, a.problem, a.reason, a.stderr, a.stdout].some(
          Boolean,
        )
      );
    });
    // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
    for (const entry of view.details ? (snapshot.outputs?.results ?? []) : recent ? [recent] : []) {
      if (view.details) add(`${entry.model}/${entry.name} v${entry.version}`);
      const attrs = entry.attributes ?? {},
        review = attrs.review ?? attrs;
      if (typeof attrs.removed === "boolean")
        add(
          attrs.removed
            ? "Worktree removed. Branch and VM retained."
            : "Worktree retained. Inspect cleanup diagnostics before retrying.",
        );
      if (typeof attrs.integrated === "boolean")
        add(
          attrs.integrated
            ? "Changes integrated into the local target."
            : "Changes were not integrated.",
        );
      if (attrs.problem) add(attrs.problem, true);
      if (attrs.reason) add(attrs.reason, true);
      if (attrs.stderr) add(attrs.stderr, true);
      if (attrs.stdout) add(attrs.stdout);
      for (const finding of review.findings ?? [])
        add(`${finding.severity} ${finding.file}: ${finding.message}`, true);
      if (view.details && attrs.pane != null)
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
      if (view.details && evidence)
        add(`Source ${text(evidence.source?.commit)} / target ${text(evidence.target?.commit)}`);
    }
    if (view.details) {
      add("Workflow log (last 100 lines):");
      add(snapshot.logs ?? "", false, true);
    }
  }
  for (const stream of streams) {
    add(stream.value);
    add(
      `Live ${stream.operation} / ${stream.source}${stream.omitted ? " [older output omitted]" : ""} (End: live)`,
    );
    required(blocks.at(-1)).live = stream === latestStream;
  }
  if (view.readingQuestion && question) {
    blocks.splice(0, blocks.length, question);
  }
  markdownCache = cache;
  const noticeHeight = view.notice && rows >= 3 ? 1 : 0;
  const height = Math.min(formHeight, Math.max(0, rows - noticeHeight - (rows >= 2 ? 1 : 0)));
  const summaryHeight = Math.min(
    summary.length,
    rows >= 10 ? 4 : 2,
    Math.max(0, rows - height - noticeHeight - 1),
  );
  const hintHeight = rows >= 8 ? (cols < 60 ? 2 : 1) : 0;
  const available = Math.max(0, rows - height - noticeHeight - summaryHeight - hintHeight);
  const footerHeight =
    question && !view.readingQuestion ? Math.min(question.count, Math.floor(available / 2)) : 0;
  const separator = footerHeight > 0 && available - footerHeight >= 2 ? 1 : 0;
  let page = Math.max(0, available - footerHeight - separator);
  view.formY = rows - height;
  const total = blocks.reduce((sum, block) => sum + block.count, 0);
  const indicatorHeight = total > page && page >= 2 && cols > 0 ? 1 : 0;
  page -= indicatorHeight;
  contentHeight = page;
  scrollLimit = Math.max(0, total - page);
  if (followLive && !view.readingQuestion) {
    let end = 0;
    for (const block of blocks) {
      end += block.count;
      if (block.live) break;
    }
    view.scroll = Math.max(0, end - page);
  } else if (pinBottom && !view.readingQuestion) view.scroll = scrollLimit;
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
  for (let i = 0; i < summaryHeight; i++)
    frame.push(
      z.ui
        .text(z.text.truncate(text(required(summary[i])), cols))
        .render({ x: 0, y: i, width: cols, height: 1 }),
    );
  let skip = view.scroll,
    y = 0;
  for (const block of blocks) {
    const count = Math.min(Math.max(0, block.count - skip), page - y);
    if (count > 0) {
      draw(block, y + summaryHeight, skip, count);
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
        .render({ x: 0, y: page + summaryHeight, width: cols, height: 1 }),
    );
  }
  if (question && footerHeight > 0)
    draw(question, page + summaryHeight + indicatorHeight + separator, 0, footerHeight);
  const focus = required(form).focusedId();
  const labels = [...controlLabels.keys()];
  const position = focus ? labels.indexOf(focus) + 1 : 0;
  const selectedLabel = focus ? (controlLabels.get(focus) ?? "") : "";
  const questionHint =
    entry && view.pending.length > 1
      ? `Question ${view.selected + 1}/${view.pending.length} · `
      : "";
  const hint = `${questionHint}${position}/${labels.length} ${selectedLabel}`;
  const keys = view.readingQuestion
    ? "PgUp/PgDn: question · Tab/↔: actions · Enter: choose"
    : question && question.count > footerHeight
      ? "Tab/↔: actions · Enter: choose · Read question: full prompt"
      : "Tab/↔: actions · Enter: choose · PgUp/PgDn: output";
  for (let i = 0; i < hintHeight; i++) {
    const label =
      hintHeight === 1 ? `${hint} · ${keys}` : i === 0 ? hint : "Tab/↔ actions · Enter choose";
    frame.push(
      z.ui
        .text(z.text.truncate(text(label), cols))
        .dim()
        .render({ x: 0, y: view.formY - noticeHeight - hintHeight + i, width: cols, height: 1 }),
    );
  }
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
