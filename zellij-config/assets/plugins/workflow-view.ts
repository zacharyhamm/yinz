// Detachable view of authoritative Swamp history. The runner lives in another pane.
import z, {type Event, type PipeMessage, type FormWidget, type FieldDescriptor,
  type MarkdownComponent, type WidgetInteraction, type Json} from 'zellij:plugin';

type Question = {model: string; version: number; attributes: {run: string; question: string; state: string; kind: string; prompt: string; answers?: string[]}};
type History = {id?: string; workflowName?: string; status: string; jobs?: {name: string; steps?: {name: string; status: string}[]}[]};
type Output = {output?: string; previewTruncated?: boolean; outputTruncated?: boolean; logError?: string;
  pane?: number; log?: string; outcome?: string; exitCode?: number | null;
  findings?: {severity: string; file: string; message: string}[]; review?: Output; test?: Output;
  evidence?: {source: {commit: string}; target: {commit: string}}; source?: {commit: string}; target?: {commit: string}};
type Snapshot = {run?: string; launch?: string; root: string; history: History; children?: History[]; active?: boolean;
  questions?: {results?: Question[]}; outputs?: {results?: {model: string; name: string; version: number; attributes: Output}[]}; logs?: string};
type Attachment = {directory: string; run?: string; launch?: string};
type Block = {widget?: MarkdownComponent; lines?: string[]; count: number; scroll: number; important?: boolean; live?: boolean};
export const view = {
  status: 'No run attached', scroll: 0, pending: [] as Question[], selected: 0, active: false,
  submitting: false, refreshing: false, selecting: false, dirty: false, formY: 0,
  attachment: undefined as Attachment | undefined, snapshot: undefined as Snapshot | undefined,
  notice: undefined as string | undefined, noticeError: false, refreshError: undefined as string | undefined,
  load, attach, refresh, pipe, update, render, focusedControl: () => {controls(); return form!.focusedId();},
};
let form: FormWidget | undefined, onRestart: (() => void) | undefined;
let formKey: string | undefined, questionKey: string | undefined, formHeight = 1, formHasInput = false, contentHeight = 0;
const answered = new Map<string, number>();
let generation = 0;
type Stream = {operation: string; source: string; raw: Uint8Array; value: string; omitted: boolean};
let streams: Stream[] = [], latestStream: Stream | undefined, launch: string | undefined;
let followLive = true;
function watch(value?: string) {
  if (value !== launch) { streams = []; latestStream = undefined; followLive = true; }
  launch = value;
  z.command('SubscribePaneOutput', value ? {swamp_work: value} : null);
}
function output(operation: string, source: string, data: Uint8Array) {
  let stream = streams.find(entry => entry.operation === operation && entry.source === source);
  if (!stream) { stream = {operation, source, raw: new Uint8Array(), value: '', omitted: false}; streams.push(stream); }
  latestStream = stream;
  const size = stream.raw.length + data.length, raw = new Uint8Array(Math.min(size, 65536));
  const retained = Math.max(0, raw.length - data.length);
  raw.set(stream.raw.subarray(stream.raw.length - retained));
  raw.set(data.subarray(Math.max(0, data.length - raw.length)), retained);
  let start = 0, end = raw.length;
  if (size > 65536) {
    stream.omitted = true;
    while (start < end && raw[start] >= 128 && raw[start] < 192) start++;
  }
  stream.raw = raw.slice(start);
  // Keep incomplete UTF-8 and escapes in raw until the next chunk completes them.
  let last = end - 1;
  while (last >= start && raw[last] >= 128 && raw[last] < 192) last--;
  const first = raw[last], need = first >= 240 ? 4 : first >= 224 ? 3 : first >= 194 ? 2 : 1;
  if (last >= start && end - last < need) end = last;
  stream.value = text(z.decodeUtf8(raw.subarray(start, end))
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, '')
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/\x1b(?:\][^\x07\x1b]*(?:\x1b)?|\[[0-?]*[ -/]*)?$/g, '')
    .replace(/\x1b[@-_]/g, ''));
  repaint();
}
const text = (value: unknown) => z.text.sanitize(String(value ?? ''));
const quote = (value: string) => "'" + value.replaceAll("'", "'\"'\"'") + "'";
const identity = (entry?: Question) => entry ? JSON.stringify([entry.attributes.run, entry.model, entry.attributes.question]) : undefined;
function controls() {
  const entry = view.pending[view.selected], q = entry?.attributes, key = identity(entry);
  const signature = JSON.stringify([key ?? '', q?.kind ?? '', q?.answers ?? [], view.pending.length > 1, view.active]);
  if (signature === formKey) return;
  const focus = key === questionKey ? form?.focusedId() : undefined;
  const hasInput = !!q && !(q.answers?.length);
  const value = key === questionKey && formHasInput && hasInput ? String(form?.value('answer') ?? '') : '';
  const fields: FieldDescriptor[] = [];
  if (q) {
    if (!hasInput) q.answers!.forEach((answer, index) => fields.push({kind: 'button', id: `choice_${index + 1}`,
      label: text(answer.replaceAll('-', ' ').replace(/^[a-z]/, c => c.toUpperCase()))}));
    else fields.push({kind: 'textInput', id: 'answer', label: 'Answer', value}, {kind: 'button', id: 'submit', label: 'Submit'});
  }
  if (view.pending.length > 1) fields.push({kind: 'button', id: 'next', label: 'Next question'});
  if (view.active) fields.push({kind: 'button', id: 'cancel_run', label: 'Cancel run'});
  if (onRestart && !view.active && !view.pending.length) fields.push({kind: 'button', id: 'restart', label: 'Restart agent'});
  fields.push({kind: 'button', id: 'detach', label: 'Detach'});
  form = z.ui.form(fields);
  if (focus && fields.some(field => field.id === focus)) form.focus(focus);
  formKey = signature; questionKey = key; formHasInput = hasInput; formHeight = hasInput ? 2 : 1;
}
function repaint() { if (view.selecting) view.dirty = true; else z.requestRender(); }
async function refresh() {
  if (!view.attachment || view.refreshing || view.submitting) return;
  view.refreshing = true;
  const revision = generation, before = JSON.stringify([view.snapshot ?? {}, view.status, view.refreshError ?? '']);
  try {
    const snapshot = await z.rpc.call({pluginUrl: 'zellij:swamp-bridge', name: 'swamp', timeoutMs: 30000,
      payload: JSON.parse(JSON.stringify({method: 'workflow_show', ...view.attachment}))}) as unknown as Snapshot;
    if (revision !== generation) return;
    view.refreshError = undefined; view.snapshot = snapshot;
    view.active = !!snapshot.active || [snapshot.history, ...(snapshot.children ?? [])].some(h => ['running', 'suspended'].includes(h.status));
    view.attachment.run = snapshot.run;
    if (snapshot.launch && snapshot.launch !== launch) watch(snapshot.launch);
    view.status = `Run ${snapshot.run ?? snapshot.launch}: ${snapshot.history.status}`;
    const selected = identity(view.pending[view.selected]), latest = new Map<string, Question>();
    for (const entry of snapshot.questions?.results ?? []) {
      const key = identity(entry)!;
      if (!latest.has(key) || entry.version > latest.get(key)!.version) latest.set(key, entry);
    }
    view.pending = [...latest.entries()].filter(([key, entry]) => entry.attributes.state === 'pending' && entry.version > (answered.get(key) ?? -Infinity))
      .sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([, entry]) => entry);
    view.selected = Math.min(view.selected, Math.max(0, view.pending.length - 1));
    const index = view.pending.findIndex(entry => identity(entry) === selected);
    if (index >= 0) view.selected = index;
    controls();
  } catch (error) { if (revision === generation) view.refreshError = String(error); }
  finally {
    view.refreshing = false;
    if (before !== JSON.stringify([view.snapshot ?? {}, view.status, view.refreshError ?? ''])) repaint();
    if (revision !== generation) void refresh();
  }
}
function attach(attachment: Attachment) {
  generation++; view.submitting = false; streams = []; latestStream = undefined; followLive = true;
  view.selecting = false; view.dirty = false; formKey = undefined; questionKey = undefined;
  watch(attachment.launch); view.attachment = {...attachment}; view.snapshot = undefined; view.pending = []; view.selected = 0; view.scroll = 0;
  view.notice = undefined; view.noticeError = false; view.refreshError = undefined; view.active = true; view.status = 'Loading workflow...';
  void refresh();
}
function load(options?: {onRestart?: () => void}) {
  onRestart = options?.onRestart; formKey = undefined;
  z.subscribe(['Key', 'Mouse', 'PastedText', 'PaneOutput', 'CommandPaneExited']); z.command('SetSelfMouseSelectionSupport', true);
  controls(); z.every(2000, refresh);
}
function pipe(message: PipeMessage) {
  if (message.name !== 'show' || !message.payload) return;
  attach(JSON.parse(message.payload)); z.command('ShowSelf', true);
  if (typeof message.source === 'object' && 'Cli' in message.source) {
    z.command('CliPipeOutput', [message.source.Cli, JSON.stringify({attached: true})]); z.command('UnblockCliPipeInput', message.source.Cli);
  }
}
function update(event: Event): boolean {
  controls();
  if (typeof event !== 'object') return false;
  if ('PaneOutput' in event) {
    const {context, pane_id, data} = event.PaneOutput;
    if (launch && context.swamp_work === launch) output(context.swamp_operation || 'command', `pane ${pane_id}`, new Uint8Array(data));
    return false;
  }
  if ('CommandPaneExited' in event) {
    if (launch && event.CommandPaneExited[2].swamp_work === launch) void refresh();
    return false;
  }
  if ('Key' in event && event.Key.bare_key === 'End' && form!.focusedId() !== 'answer') { followLive = true; view.scroll = Infinity; return true; }
  if ('Key' in event && event.Key.bare_key === 'PageUp') { followLive = false; view.scroll = Math.max(0, view.scroll - contentHeight); return true; }
  if ('Key' in event && event.Key.bare_key === 'PageDown') { view.scroll += contentHeight; return true; }
  if ('Mouse' in event) {
    const mouse = event.Mouse;
    if ('ScrollUp' in mouse) { followLive = false; view.scroll = Math.max(0, view.scroll - Math.min(Number(mouse.ScrollUp), contentHeight)); return true; }
    if ('ScrollDown' in mouse) { view.scroll += Math.min(Number(mouse.ScrollDown), contentHeight); return true; }
    if ('LeftClick' in mouse) view.selecting = Number(mouse.LeftClick[0]) < view.formY;
    if (view.selecting) {
      if ('Release' in mouse) { view.selecting = false; if (view.dirty) { view.dirty = false; repaint(); } }
      return false;
    }
    const position = Object.values(mouse)[0];
    if (!('Release' in mouse) && (!Array.isArray(position) || Number(position[0]) < view.formY)) return false;
  }
  let action: WidgetInteraction;
  if ('Key' in event) action = form!.handleKey(event.Key);
  else if ('Mouse' in event) action = form!.handleMouse(event.Mouse);
  else if ('PastedText' in event) action = form!.insertPastedText(event.PastedText);
  else return false;
  if (action.kind === 'cancel' || ('id' in action && action.id === 'detach')) { generation++; view.attachment = undefined; view.submitting = false; watch(); z.command('CloseSelf'); return true; }
  if ('id' in action && action.id === 'next') { view.selected = (view.selected + 1) % view.pending.length; controls(); return true; }
  if (action.kind !== 'buttonPressed' || view.submitting) return action.kind !== 'ignored';
  if (action.id === 'restart' && onRestart && !view.active && !view.pending.length) { onRestart(); return true; }
  const entry = view.pending[view.selected], choice = /^choice_(\d+)$/.exec(action.id ?? '');
  let command: string[], success: string;
  if (entry && (action.id === 'submit' || choice)) {
    if (entry.version <= (answered.get(identity(entry)!) ?? -Infinity)) return false;
    const q = entry.attributes, answer = choice ? q.answers![Number(choice[1]) - 1] : String(form!.value('answer'));
    command = [z.serverBinary || 'zellij', 'workflow', 'answer', q.run, entry.model, q.question, answer]; success = 'Answer recorded';
  } else if (action.id === 'cancel_run' && view.snapshot && view.attachment?.run) {
    command = [z.serverBinary || 'zellij', 'workflow', 'cancel', view.attachment.run]; success = 'Cancellation requested; resources preserved';
  } else return true;
  generation++; view.submitting = true; view.noticeError = false;
  view.notice = success === 'Answer recorded' ? 'Submitting answer...' : 'Requesting cancellation...';
  void submit(command, success, view.attachment!, view.snapshot!.root, entry);
  return true;
}
async function submit(command: string[], success: string, attachment: Attachment, cwd: string, entry?: Question) {
  const operation = `${command[2] === 'answer' ? identity(entry) : command[3]}: ${command[2] === 'answer' ? command[6] : 'cancel_run'}`;
  try {
    const result = await z.runCommand({cwd, context: {swamp_owner: 'view'}, command, maxBytesReturned: 8192,
      onOutput: event => { if (view.attachment === attachment) output(operation, event.stderr ? 'stderr' : 'stdout', event.data); }});
    if (view.attachment !== attachment) return;
    const accepted = result.exitCode === 0;
    view.noticeError = !accepted;
    view.notice = accepted ? success : z.decodeUtf8(result.stderr) || z.decodeUtf8(result.stdout) || 'Command failed';
    if (accepted && command[2] === 'answer' && entry) {
      answered.set(identity(entry)!, entry.version);
      view.pending = view.pending.filter(pending => identity(pending) !== identity(entry));
      view.selected = Math.min(view.selected, Math.max(0, view.pending.length - 1)); controls();
    }
  } catch (error) { if (view.attachment === attachment) { view.noticeError = true; view.notice = String(error); } }
  finally {
    if (view.attachment === attachment) {
      if (view.noticeError) output(operation, 'submission error', z.encodeUtf8(view.notice!));
      view.submitting = false; repaint(); void refresh();
    }
  }
}
let markdownCache: {source: string; dim: boolean; widget: MarkdownComponent}[] = [];
const failed = (status?: string) => status === 'failed' || status === 'error';
function render(rows: number, cols: number): string {
  controls();
  const blocks: Block[] = [], cache: typeof markdownCache = [];
  function markdown(source: string, dim: boolean): Block {
    const previous = markdownCache[cache.length];
    const widget = previous?.source === source && previous.dim === dim ? previous.widget : z.ui.markdown(source, {dim});
    cache.push({source, dim, widget});
    return {widget, count: widget.render({x: 0, y: 0, width: cols, height: 0}).totalRows, scroll: 0};
  }
  function add(source: unknown, important = false, isMarkdown = false) {
    if (isMarkdown) blocks.push(markdown(String(source ?? ''), !important));
    else { const lines = z.text.wrap(text(source), Math.max(1, cols)); blocks.push({lines, count: lines.length, important, scroll: 0}); }
  }
  const entry = view.pending[view.selected], question = entry ? markdown(entry.attributes.prompt, false) : undefined;
  add(view.status, failed(view.snapshot?.history.status));
  if (view.notice) add(view.notice, view.noticeError);
  if (view.refreshError) add(view.refreshError, true);
  if (!entry && view.active) add('Waiting for the workflow. No question pending.');
  const snapshot = view.snapshot;
  if (snapshot) {
    for (const history of [snapshot.history, ...(snapshot.children ?? [])]) {
      add(`${text(history.workflowName)} / ${text(history.id)}: ${text(history.status)}`, failed(history.status));
      for (const job of history.jobs ?? []) for (const step of job.steps ?? []) add(`${job.name}/${step.name}: ${step.status}`, failed(step.status));
    }
    for (const entry of snapshot.outputs?.results ?? []) {
      add(`${entry.model}/${entry.name} v${entry.version}`);
      const attrs = entry.attributes ?? {}, review = attrs.review ?? attrs;
      for (const finding of review.findings ?? []) add(`${finding.severity} ${finding.file}: ${finding.message}`, true);
      if (attrs.pane != null) add(`Pane ${attrs.pane} / ${text(attrs.log)} / ${text(attrs.outcome)}`, failed(attrs.outcome));
      function showOutput(value: Output, path: string) {
        if (value.output == null) return;
        if (value.previewTruncated) {
          add('Output preview (up to 16,384 characters). Retrieve the complete saved version:');
          const predicate = `modelName == ${JSON.stringify(entry.model)} && name == ${JSON.stringify(entry.name)} && version == ${entry.version}`;
          add(`swamp data query ${quote(predicate)} --select ${path} --json --repo-dir ${quote(snapshot!.root)}`);
        }
        if (value.outputTruncated) add('Retained command scrollback (may omit older output):');
        add(value.output, false, true);
      }
      showOutput(attrs, 'attributes.output');
      if (attrs.logError) add(`Log unavailable: ${text(attrs.logError)}`, true);
      if (attrs.test) {
        add(`Gate ${text(attrs.test.outcome)} / exit ${text(attrs.test.exitCode)} / log ${text(attrs.test.log)}`,
          failed(attrs.test.outcome) || (typeof attrs.test.exitCode === 'number' && attrs.test.exitCode !== 0));
        showOutput(attrs.test, 'attributes.test.output');
      }
      const evidence = attrs.evidence ?? (attrs.source ? attrs : undefined);
      if (evidence) add(`Source ${text(evidence.source?.commit)} / target ${text(evidence.target?.commit)}`);
    }
    add('Workflow log (last 100 lines):'); add(snapshot.logs ?? '', false, true);
  }
  for (const stream of streams) {
    add(stream.value);
    add(`Live ${stream.operation} / ${stream.source}${stream.omitted ? ' [older output omitted]' : ''} (End: live)`);
    blocks[blocks.length - 1].live = stream === latestStream;
  }
  markdownCache = cache;
  const height = Math.min(formHeight, Math.max(0, rows - (rows >= 2 ? 1 : 0)));
  const footerHeight = question ? Math.min(question.count, Math.floor((rows - height) / 2)) : 0;
  if (question && question.count > footerHeight) blocks.unshift({...question, count: question.count - footerHeight, scroll: footerHeight});
  const separator = footerHeight > 0 && rows - height - footerHeight >= 2 ? 1 : 0;
  const page = Math.max(0, rows - height - footerHeight - separator); contentHeight = page; view.formY = rows - height;
  const total = blocks.reduce((sum, block) => sum + block.count, 0);
  if (streams.length && followLive) {
    let liveEnd = 0;
    for (const block of blocks) { liveEnd += block.count; if (block.live) break; }
    view.scroll = Math.max(0, liveEnd - page);
  }
  view.scroll = Math.max(0, Math.min(view.scroll, Math.max(0, total - page)));
  const frame: string[] = [];
  function draw(block: Block, y: number, skip: number, count: number) {
    if (block.widget) frame.push(block.widget.render({x: 0, y, width: cols, height: count, scroll: block.scroll + skip}).text);
    else for (let i = 0; i < count; i++) {
      const line = z.ui.text(block.lines![skip + i]);
      frame.push((block.important ? line.error() : line.dim()).render({x: 0, y: y + i, width: cols, height: 1}));
    }
  }
  let skip = view.scroll, y = 0;
  for (const block of blocks) {
    const count = Math.min(Math.max(0, block.count - skip), page - y);
    if (count > 0) { draw(block, y, skip, count); y += count; }
    skip = Math.max(0, skip - block.count);
  }
  if (question && footerHeight > 0) draw(question, page + separator, 0, footerHeight);
  return frame.join('') + form!.render({x: 0, y: view.formY, width: cols, height});
}
