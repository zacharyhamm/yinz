// Durable delivery adapter. Swamp owns the DAG, evidence, questions and decisions.
import z, {
  definePlugin,
  type Json,
  type DataRecord,
  type Integer,
  type PaneId,
  type PaneTarget,
  type AgentSpawnOptions,
  type AgentEvent,
  type MessageToPlugin,
  type RpcCall,
} from "yinz:plugin";
function required<T>(value: T): NonNullable<T> {
  if (value == null) throw new Error("Missing required plugin state");
  return value;
}

type Owner = { host_id: string; session: string; incarnation: string; work: string; agent: string };
type Identity = Pick<Owner, "host_id" | "session" | "incarnation">;
type Request = {
  method: string;
  work: string;
  key: string;
  agent: string;
  operation: string;
  directory: string;
  run: string;
  launch: string;
  workflow: string;
  prompt: string;
  prompt_sha256?: string;
  waitForExit?: boolean;
  includeOutput?: boolean;
  foreground?: boolean;
  process?: number;
  action: string;
  cancel_active?: boolean;
  spec: AgentSpawnOptions & { source_directory?: string; automatic_firecracker_vm?: boolean };
  argv: string[];
  cwd: string;
  inputs?: Record<string, Json>;
  exactInputs?: boolean;
  payload: Record<string, Json>;
  model: string;
  name: string;
  offset: number;
  version: number;
  capture?: Json;
};
type Attachment = { run: string; work: string; workflow: string; root: string };
type Delivery = {
  request: string;
  key: string;
  work: string;
  outcome: string;
  incarnation: string;
  dispatcher: string;
  owner?: Owner | undefined;
  pane?: number | undefined;
  supervisor?: string | undefined;
  process?: number | undefined;
  transportOwner?: string | null | undefined;
  outstanding?: string | undefined;
  ready?: boolean | undefined;
  processExited?: boolean | undefined;
  accepted?: boolean | undefined;
  dispatched?: boolean | undefined;
  agentSession?: string | null | undefined;
  turn?: string | undefined;
  message?: string | null | undefined;
  hook?: string | undefined;
  target?: PaneTarget | undefined;
  remoteHost?: string | undefined;
  remotePaneId?: string | undefined;
  branch?: string | null | undefined;
  checkout?: string | null | undefined;
  log?: string | undefined;
  error?: string | undefined;
  output?: string | undefined;
  outputTruncated?: boolean | undefined;
  logError?: string | undefined;
  exitCode?: number | null | undefined;
  workflow?: string | undefined;
  launch?: string | undefined;
  view?: number | undefined;
  entries?: Record<string, Attachment> | undefined;
  choice?: string | undefined;
  vm?: string | null | undefined;
  clean?: boolean | undefined;
  transport?: string | undefined;
  requestId?: string | undefined;
  correlation?: string | undefined;
  run?: string | undefined;
  closed?: boolean | undefined;
  cancel_pending?: boolean | undefined;
  cleanup?: boolean | undefined;
  transition?: string | undefined;
  source?: number | undefined;
  current?: number | undefined;
  not_dispatched?: boolean | undefined;
  capture?: Json | undefined;
  executionFinished?: boolean | undefined;
  cancelRequested?: boolean | undefined;
  interruptSent?: boolean | undefined;
  interruptedAt?: number | undefined;
  killSent?: boolean | undefined;
  terminationVerified?: boolean | undefined;
};
type Supervisor = {
  terminal_id: number;
  host_id: string;
  child_pid?: number | undefined;
  attached: boolean;
  session_name: string;
  child_exit_status?: number | null | undefined;
  child_group_terminated?: boolean | undefined;
  owner?: string | null | undefined;
  remote_host?: string | undefined;
  pane_token?: string | undefined;
};
const D = z.data;
let identity: Identity;
type Placement = {
  view: number;
  queue: string[];
  current?: number | undefined;
  detached?: boolean | undefined;
  manual?: boolean | undefined;
  subtitle?: string | undefined;
  endpoint?: string | undefined;
  workflow?: string | undefined;
};
// Placement belongs to an authenticated builtin view, never to a caller-supplied pane.
// It is intentionally not recovered across server/bridge reloads.
const placements = new Map<string, Placement>();
const manualViews = new Map<string, Set<number>>();
// oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
function assert(value: unknown, message: string): asserts value {
  const truthy = Boolean(value);
  if (!truthy) throw new Error(message);
}
function isRecord(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === "object" && !Array.isArray(value);
}
function object(value: unknown): Record<string, unknown> {
  assert(isRecord(value), "Expected a JSON object");
  return value;
}
function string(value: unknown): string {
  assert(typeof value === "string", "Expected a string");
  return value;
}
function parsedObject(source: string): Record<string, unknown> {
  const value: unknown = JSON.parse(source);
  return object(value);
}
// Lua's encoder sorts object keys recursively. Preserve exact request/owner comparisons on reload.
function encode(value: unknown): string {
  return JSON.stringify(value, (_key: string, item: unknown): unknown =>
    isRecord(item)
      ? Object.fromEntries(
          Object.keys(item)
            .sort()
            // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
            .map((key) => [key, item[key]]),
        )
      : item,
  );
}
// Lua decoded null object fields as absent and encoded empty tables as arrays.
// Normalize both sides when comparing a persisted request from either runtime.
function sameRequest(saved: string, request: unknown): boolean {
  const previous: unknown = JSON.parse(saved);
  const current = object(request);
  const forwarded =
    current["method"] === "swamp_dispatch"
      ? "payload"
      : current["method"] === "workflow_run"
        ? "inputs"
        : undefined;
  if (
    forwarded &&
    (!isRecord(previous) || encode(previous[forwarded]) !== encode(current[forwarded]))
  )
    return false;
  const normalize = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map((entry) => normalize(entry));
    if (isRecord(value)) {
      const entries = Object.entries(value)
        .filter(([, v]) => v != null)
        .map(([k, v]) => [k, normalize(v)]);
      return entries.length > 0 ? Object.fromEntries(entries) : [];
    }
    return value;
  };
  return encode(normalize(previous)) === encode(normalize(request));
}
function key(value: string): string {
  assert(
    typeof value === "string" &&
      /^[A-Za-z0-9][A-Za-z0-9_.:-]*$/u.test(value) &&
      value.length <= 200,
    "Invalid correlation key",
  );
  return value;
}
function scope(work: string, operation: string): string {
  return encode([identity.host_id, identity.session, key(work), key(operation)]);
}
function read<T = Delivery>(uid: string): [T | undefined, DataRecord | undefined] {
  const row = D.fetch({ entity: "delivery", filter: D.eq("uid", uid), limit: 1 }).records[0];
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
  return [row ? (JSON.parse(String(row["value"])) as T) : undefined, row];
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function put(uid: string, value: unknown, row?: DataRecord): void {
  D.apply([D.put("delivery", { value: encode(value) }, { uid, expectRev: row?.rev ?? 0 })]);
}
function result(uid: string): Delivery {
  const [value] = read(uid);
  assert(value, "No operation with that identity");
  return value;
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function saveEvent(uid: string, outcome: string, fields: Partial<Delivery> = {}): void {
  const [value, row] = read(uid);
  if (!value || !["pending", "unknown"].includes(value.outcome)) return;
  put(uid, { ...value, outcome, ...fields }, row);
}
const terminal = (id: number): { Terminal: number } => ({ Terminal: id });
const plugin = (id: number): PaneId => ({ Plugin: id });
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function paneSubtitle(pane: PaneId, subtitle?: string): void {
  z.command("SetPaneSubtitle", { pane_id: pane, subtitle: subtitle ?? null });
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function detach(placement: Placement): void {
  if (placement.detached) return;
  placement.detached = true;
  paneSubtitle(plugin(placement.view));
  if (placement.current != null) paneSubtitle(terminal(placement.current));
  placement.subtitle = undefined;
  message({ destination_plugin_id: placement.view, message_name: "workflow-detached" });
}
// History is the authority; delivery records only decide which pane is foregrounded.
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function workflowSubtitle(snapshot: { history: History; children?: History[] }): string {
  const main = snapshot.history;
  // oxlint-disable-next-line eslint/no-shadow, typescript/prefer-readonly-parameter-types -- The nested callback has its own independent operation state. This helper shares mutable plugin state and SDK wire types with its callers.
  const steps = (histories: History[], active: boolean): string[] => {
    const entries: string[] = [];
    for (const history of histories)
      for (const job of history.jobs ?? [])
        for (const step of job.steps ?? []) {
          const running =
            ["running", "suspended"].includes(history.status) &&
            ["running", "waiting_approval"].includes(step.status);
          if (active ? !running : step.status !== "failed") continue;
          let name = `${job.name}/${step.name}`;
          if (history !== main) name = `${history.workflowName ?? history.id}/${name}`;
          if (!active && main.status !== "failed") name += " (failed)";
          entries.push(`${name} · ${active ? step.status : main.status}`);
        }
    return entries.sort();
  };
  let entries = steps(snapshot.children ?? [], true);
  if (entries.length === 0) entries = steps([main], true);
  if (entries.length === 0) {
    entries = steps(snapshot.children ?? [], false);
    if (entries.length === 0) entries = steps([main], false);
  }
  return `${main.workflowName ?? "Workflow"} · ${entries.length > 0 ? entries.join(", ") : main.status}`;
}
const cancelling = new Set<string>();
const preparing = new Map<string, Promise<{ ready: boolean }>>();
const inFlight = new Set<{ work: string | undefined }>();
function lifecycleWork(work: string): string {
  return read<Attachment>(scope(work, "attachment"))[0]?.work ?? work;
}
function closed(work?: string): boolean {
  return !!work && !!read(scope(lifecycleWork(work), "view"))[0]?.closed;
}
function ensureOpen(work?: string): void {
  assert(!closed(work), "Pane workflow lifecycle is closed");
}
function transition(work: string, action: string): void {
  const uid = scope(lifecycleWork(work), "view"),
    [view, row] = read(uid);
  if (view) {
    ensureOpen(work);
    view.transition = action;
    put(uid, view, row);
  }
}
function cancelAfterTransition(work: string): void {
  // oxlint-disable-next-line eslint/no-param-reassign -- Update the caller-owned state or the local retry state in place.
  work = lifecycleWork(work);
  const uid = scope(work, "view"),
    [view, row] = read(uid);
  if (!view) return;
  view.closed = true;
  view.cancel_pending = true;
  put(uid, view, row);
  for (const entry of deliveryRows()) {
    const value = entry.value;
    if (
      !["pending", "unknown"].includes(value.outcome) ||
      !value.request ||
      lifecycleWork(entry.work) !== work
    )
      continue;
    // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
    const request: Request = JSON.parse(value.request);
    if (request.method === "pane_action" && ["fork", "duplicate"].includes(request.action)) {
      value.outcome = "interrupted";
      value.error = "Pane workflow lifecycle is closed";
      put(entry.uid, value, entry.row);
    }
  }
  void cancelLifecycle(work).catch((error: unknown) => {
    z.log(String(error));
  });
}
function closeView(work: string, restoreSource?: number): void {
  const existing = placements.get(work);
  if (existing) detach(existing);
  const [placement] = read(scope(work, "view"));
  if (!placement || placement.incarnation !== identity.incarnation || placement.view == null)
    return;
  const pane = z.command("GetPaneInfo", plugin(placement.view));
  if (pane?.plugin_url === "yinz:agent-exit") {
    if (restoreSource != null && !pane.is_suppressed) {
      z.command("ReplacePaneWithExistingPane", [
        plugin(placement.view),
        terminal(restoreSource),
        true,
      ]);
      z.command("ScrollToBottomInPaneId", terminal(restoreSource));
    }
    z.command("HidePaneWithId", plugin(placement.view));
    z.command("ClosePluginPane", placement.view);
  }
}
function active(work: string): boolean {
  if (closed(work)) return false;
  const works = new Set([work, ...Object.keys(read(scope(work, "runs"))[0]?.entries ?? {})]);
  let cursor: Integer | null = null;
  do {
    const { records, raw } = D.fetch({ entity: "delivery", afterId: cursor });
    for (const row of records) {
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
      const id: string[] = JSON.parse(String(row["uid"]));
      if (id[0] === identity.host_id && id[1] === identity.session && works.has(required(id[2]))) {
        // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
        const operation: Delivery = JSON.parse(String(row["value"]));
        if (["pending", "unknown"].includes(operation.outcome)) {
          // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
          const request: Request = JSON.parse(operation.request);
          // oxlint-disable-next-line eslint/max-depth -- Nested protocol checks preserve the order of validation and state changes.
          if (
            ["command", "agent_prompt"].includes(request.method) ||
            (request.method === "agent_spawn" && request.waitForExit)
          )
            return true;
        }
      }
    }
    cursor = raw.next_cursor;
  } while (cursor != null);
  return false;
}
function launchFailure(
  work: string,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  launch: Delivery,
  logs: string,
  directory?: string,
): {
  launch: string;
  root: string | undefined;
  history: { id: string; workflowName: string; status: string };
  children: History[];
  active: boolean;
  problem: string;
  logs: string;
} {
  return {
    launch: work,
    root: launch.checkout ?? directory,
    history: { id: work, workflowName: launch.workflow ?? "Workflow launch", status: "failed" },
    children: [],
    active: active(work),
    problem: logs,
    logs,
  };
}
function cancellable(request: string): boolean {
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
  const r: Request = JSON.parse(request);
  return (
    r.method === "command" ||
    r.method === "agent_prompt" ||
    (r.method === "agent_spawn" && r.waitForExit === true)
  );
}
function activeOperation(work: string): { work: string; key: string } | undefined {
  const entry = deliveryRows().find(
    ({ work: other, value }) =>
      lifecycleWork(other) === lifecycleWork(work) &&
      ["pending", "unknown"].includes(value.outcome) &&
      !!value.request &&
      cancellable(value.request),
  );
  return entry ? { work: entry.work, key: entry.key } : undefined;
}
const stopping = new Map<string, Promise<void>>();
function stopOperation(uid: string): void {
  if (stopping.has(uid)) return;
  const pending = verifyStop(uid)
    .catch((error: unknown) => {
      z.log(String(error));
    })
    .finally(() => {
      stopping.delete(uid);
    });
  stopping.set(uid, pending);
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. Termination verification keeps its ordered ownership checks together.
async function verifyStop(uid: string): Promise<void> {
  try {
    const deadline = Date.now() + 30000;
    for (;;) {
      let [value, row] = read(uid);
      assert(value?.cancelRequested, "Cancellation no longer owns this operation");
      if (!["pending", "unknown"].includes(value.outcome)) return;
      assert(
        Date.now() < deadline,
        "Termination is uncertain; reconcile the saved operation before retrying",
      );
      // Cancellation can arrive while dispatch is still waiting for its supervisor.
      if (value.pane == null || !value.supervisor) {
        // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
        await z.sleep(100);
        continue;
      }
      const owner = value.owner;
      assert(
        owner?.host_id === identity.host_id &&
          owner.session === identity.session &&
          owner.work === value.work,
        "Wrong cancellation owner",
      );
      // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
      const host = await supervisor(value.pane, value.supervisor);
      assert(
        host.host_id === value.supervisor && host.child_pid === value.process && host.attached,
        "Process ownership changed; reconcile without signalling the replacement",
      );
      if (host.owner)
        assert(host.owner === (value.transportOwner ?? encode(owner)), "Remote ownership changed");
      [value, row] = read(uid);
      assert(value?.cancelRequested, "Cancellation lost its operation");
      if (!["pending", "unknown"].includes(value.outcome)) return;
      assert(
        encode(value.owner) === encode(owner) &&
          value.process === host.child_pid &&
          value.supervisor === host.host_id,
        "Operation ownership changed while checking its process",
      );
      if (host.child_group_terminated === true) {
        saveEvent(uid, "cancelled", {
          exitCode: host.child_exit_status,
          processExited: true,
          terminationVerified: true,
        });
        present(value.work);
        changed(value.work);
        return;
      }
      const signal = {
        terminal_id: host.terminal_id,
        owner: encode(owner),
        ...(value.process == null ? {} : { process: value.process }),
        supervisor: value.supervisor,
      };
      if (!value.interruptSent) {
        value.interruptSent = true;
        value.interruptedAt = Date.now();
        put(uid, value, row);
        assert(z.workflowSignal, "Owned pane signalling is unavailable");
        z.workflowSignal({ ...signal, kill: false });
      } else if (!value.killSent && Date.now() - (value.interruptedAt ?? Date.now()) >= 5000) {
        value.killSent = true;
        put(uid, value, row);
        assert(z.workflowSignal, "Owned pane signalling is unavailable");
        z.workflowSignal({ ...signal, kill: true });
      }
      // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
      await z.sleep(100);
    }
  } catch (error) {
    const current = result(uid);
    if (["pending", "unknown"].includes(current.outcome)) {
      saveEvent(uid, "unknown", { error: String(error), terminationVerified: false });
      changed(current.work);
    }
  }
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together.
function present(work: string): void {
  if (closed(work)) return;
  const placement = placements.get(work);
  if (!placement || placement.detached) return;
  const view = plugin(placement.view);
  const info = z.command("GetPaneInfo", view);
  const current = placement.current == null ? view : terminal(placement.current);
  const currentInfo = z.command("GetPaneInfo", current);
  if (
    (info?.plugin_url !== "yinz:agent-exit" && info?.plugin_url !== "yinz:swamp-workflow") ||
    !currentInfo ||
    currentInfo.is_suppressed
  ) {
    detach(placement);
    return;
  }
  let next: number | undefined;
  while (placement.queue.length > 0) {
    const [operation] = read(required(placement.queue[0]));
    if (
      operation &&
      (operation.outcome === "pending" ||
        (Boolean(operation.capture) &&
          !operation.closed &&
          ["completed", "failed"].includes(operation.outcome)))
    ) {
      next = operation.pane;
      break;
    }
    placement.queue.shift();
  }
  paneSubtitle(view, placement.subtitle);
  if (next === placement.current) {
    if (next != null) paneSubtitle(current, placement.subtitle);
    return;
  }
  const target = next == null ? view : terminal(next);
  if (!z.command("GetPaneInfo", target)?.is_suppressed) {
    detach(placement);
    return;
  }
  if (placement.current == null && next != null) {
    const source = read(scope(work, "source"))[0];
    // Keep the source for pane/VM actions, without restoring it when the view closes.
    if (source?.pane != null) z.command("HidePaneWithId", terminal(source.pane));
  }
  if (placement.current != null) paneSubtitle(current);
  paneSubtitle(target, placement.subtitle);
  const uid = scope(work, "view"),
    [saved, row] = read(uid);
  if (saved) {
    saved.current = next;
    put(uid, saved, row);
  }
  z.command("ReplacePaneWithExistingPane", [current, target, true]);
  // The view keeps its own scroll; a step handing the pane back returns it to the bottom.
  if (next == null && placement.current != null)
    message({ destination_plugin_id: placement.view, message_name: "workflow-returned" });
  placement.current = next;
}
function updateSubtitle(work: string, subtitle: string, workflow?: string): void {
  const placement = placements.get(work);
  if (!placement || placement.detached) return;
  placement.subtitle = subtitle;
  placement.workflow = workflow ?? placement.workflow;
  present(work);
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function unavailable(placement: Placement, work: string): void {
  updateSubtitle(
    work,
    `${placement.endpoint ? `${placement.endpoint} · ` : ""}${placement.workflow ?? "Workflow"} · status unavailable`,
  );
}
function attachManual(work: string, view: number): void {
  if (closed(work)) return;
  if (!manualViews.has(work)) manualViews.set(work, new Set());
  required(manualViews.get(work)).add(view);
  if (read(scope(work, "view"))[0]) return;
  const existing = placements.get(work);
  if (existing) {
    if (z.command("GetPaneInfo", plugin(existing.view))) return;
    detach(existing);
  }
  const placement: Placement = { view, queue: [], manual: true, subtitle: "Workflow · loading" };
  placements.set(work, placement);
  let cursor: Integer | null = null;
  do {
    // Default row-id order preserves delivery order across pages.
    const { records, raw } = D.fetch({ entity: "delivery", afterId: cursor });
    for (const row of records) {
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
      const id: string[] = JSON.parse(String(row["uid"]));
      if (id[0] !== identity.host_id || id[1] !== identity.session || id[2] !== work) continue;
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
      const operation: Delivery = JSON.parse(String(row["value"]));
      if (operation.outcome !== "pending" || operation.pane == null) continue;
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
      const request: Request = JSON.parse(operation.request);
      if (
        (request.method === "command" && request.foreground) ||
        (request.method === "agent_spawn" && request.waitForExit)
      )
        placement.queue.push(String(row["uid"]));
    }
    cursor = raw.next_cursor;
  } while (cursor != null);
  present(work);
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
async function command(argv: string[], cwd = "/"): Promise<string> {
  const r = await z.runCommand({
    command: argv,
    cwd,
    context: { swamp_owner: encode(identity) },
    maxBytesReturned: 1048576,
  });
  const stdout = z.decodeUtf8(r.stdout);
  const stderr = z.decodeUtf8(r.stderr);
  assert(
    r.exitCode === 0,
    `Host command failed: ${encode(argv)}\nDirectory: ${cwd}\nExit status: ${r.exitCode}\nstdout:\n${stdout}\nstderr:\n${stderr}`,
  );
  return stdout;
}
type SwampRequest = {
  allowLimited?: boolean;
  operation: string;
  payload?: Record<string, Json> | undefined;
  id?: string | undefined;
  started?: boolean | undefined;
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  onNotDispatched?: ((error: Json) => void) | undefined;
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  onProgress?: ((payload: Json) => void) | undefined;
};
const tracked = new Map<string, string>(),
  watching = new Set<string>();
const watchIds = new Map<string, { run: string; work: string }>();
const notifications = new Set<string>();
function changed(work: string): void {
  if (closed(work)) return;
  if (notifications.has(work)) return;
  notifications.add(work);
  void z
    .sleep(0)
    .then(() => {
      notifications.delete(work);
      if (closed(work)) return;
      const view = read(scope(work, "view"))[0];
      if (view?.incarnation === identity.incarnation && view.view != null)
        message({ destination_plugin_id: view.view, message_name: "workflow-changed" });
      for (const id of manualViews.get(work) ?? []) {
        let pane;
        try {
          pane = z.command("GetPaneInfo", plugin(id));
        } catch {
          /* A closed view is removed below. */
        }
        if (!pane) required(manualViews.get(work)).delete(id);
        else if (view?.incarnation !== identity.incarnation || id !== view.view)
          message({ destination_plugin_id: id, message_name: "workflow-changed" });
      }
    })
    .catch((error: unknown) => {
      z.log(String(error));
    });
}
function setupDiagnostic(detail: string, directory: string): string {
  const root = "'" + directory.replaceAll("'", "'\"'\"'") + "'";
  const setup = /Module not found[^\n]*\.swamp\/pulled-extensions\/@swamp\/git\//u.test(detail)
    ? `\nMissing @swamp/git in control checkout ${directory}. Run: swamp extension pull @swamp/git --repo-dir ${root}`
    : /(?:@hammz\/yinz|@swamp\/git).*could not be resolved/u.test(detail)
      ? `\nInspect extensions in control checkout ${directory}: swamp doctor extensions --repo-dir ${root} --json`
      : "";
  return detail + setup;
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function, typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers. This helper shares mutable plugin state and SDK wire types with its callers. This lifecycle handler keeps its ordered protocol transitions together. Keep this existing protocol decision tree together for review.
async function swamp(request: SwampRequest, directory: string): Promise<string> {
  const workflow = request.payload?.["workflow"];
  if (
    ["workflow.history.search", "report.search", "report.get"].includes(request.operation) &&
    typeof workflow === "string" &&
    /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/iu.test(workflow)
  ) {
    const definition = parsedObject(
      await swamp(
        { operation: "workflow.get", payload: { workflowIdOrName: workflow } },
        directory,
      ),
    );
    assert(typeof definition["name"] === "string", "Workflow has no canonical name");
    // oxlint-disable-next-line eslint/no-param-reassign -- Normalize the workflow name in this local request before dispatch.
    request = { ...request, payload: { ...request.payload, workflow: definition["name"] } };
  }
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
  const reply = (await z.swampRequest({ ...request, directory })) as {
    error?: { code: string };
    limited?: boolean;
    projected?: { rows?: Json[]; values?: Json[] };
    results?: Json[];
    log?: { lines?: string[] };
  };
  if (reply["error"]) {
    if (reply["error"].code === "not_dispatched") request.onNotDispatched?.(reply["error"]);
    throw new Error(setupDiagnostic(encode(reply["error"]), directory));
  }
  const query = request.payload?.["query"];
  if (
    typeof query === "string" &&
    query &&
    ["workflow.search", "model.search", "workflow.history.search"].includes(request.operation)
  ) {
    const needle = query.toLowerCase();
    // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
    reply["results"] = (reply["results"] ?? []).filter((value: Json) => {
      const row = parsedObject(encode(value));
      const fields =
        request.operation === "workflow.history.search"
          ? [
              row["workflowName"],
              row["runId"],
              row["status"],
              ...Object.entries(parsedObject(encode(row["inputs"] ?? {}))).map(
                ([field, item]) => `${field}=${String(item)}`,
              ),
            ]
          : [
              row["name"],
              row["id"],
              request.operation === "model.search" ? row["type"] : row["description"],
            ];
      return fields.some(
        // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
        (value) => typeof value === "string" && value.toLowerCase().includes(needle),
      );
    });
  }
  if (request.operation === "data.query" && reply["limited"] && !request.allowLimited)
    throw new Error("Swamp data query reached its result limit; narrow the saved-data query");
  if (request.operation === "data.query" && reply["projected"]) {
    reply["results"] = reply["projected"].rows ?? reply["projected"].values ?? [];
    delete reply["projected"];
  } else if (request.operation === "workflow.history.logs" && reply["log"])
    return (reply["log"].lines ?? []).join("\n");
  return encode(reply);
}
async function cancelRun(run: string, root: string, verify = false): Promise<unknown> {
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
  const history: History = JSON.parse(
    await swamp({ operation: "workflow.history.get", payload: { workflowIdOrName: run } }, root),
  );
  if (history.status === "running") {
    const cancelled = await swamp({ operation: "workflow.cancel", payload: { runId: run } }, root);
    if (!verify) return JSON.parse(cancelled);
    const final = parsedObject(
      await swamp({ operation: "workflow.history.get", payload: { workflowIdOrName: run } }, root),
    );
    assert(
      ["succeeded", "suspended", "completed", "failed", "cancelled", "interrupted"].includes(
        String(final["status"]),
      ),
      "Workflow cancellation is not complete; retry cleanup",
    );
  } else
    assert(
      ["succeeded", "suspended", "completed", "failed", "cancelled", "interrupted"].includes(
        history.status,
      ),
      verify
        ? "Workflow termination is uncertain; reconcile before cleanup"
        : "Workflow execution state is uncertain; refresh before cancelling",
    );
  return undefined;
}
function cancelIfClosed(work: string): boolean {
  if (!closed(work)) return false;
  // oxlint-disable-next-line eslint/no-param-reassign -- Update the caller-owned state or the local retry state in place.
  work = lifecycleWork(work);
  const uid = scope(work, "view"),
    [view, row] = read(uid);
  assert(view, "Missing closed lifecycle");
  view.cancel_pending = true;
  put(uid, view, row);
  void cancelLifecycle(work).catch((error: unknown) => {
    z.log(String(error));
  });
  return true;
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
async function workflowRunId(operation: Delivery, request: Request): Promise<string | undefined> {
  if (operation.run) return operation.run;
  if (request.operation === "workflow.resume")
    return typeof request.payload["runId"] === "string" ? request.payload["runId"] : undefined;
  const workflow = operation.workflow ?? request.payload?.["workflowIdOrName"];
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
  const history: {
    results?: { runId: string; workflowName: string; inputs: History["inputs"] }[];
  } = JSON.parse(
    await swamp(
      {
        operation: "workflow.history.search",
        payload: {
          // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
          ...(Boolean(workflow) ? { workflow } : {}),
          filter: `has(tags.yinzOperation) && tags.yinzOperation == ${encode(operation.correlation)}`,
        },
      },
      required(operation.checkout),
    ),
  );
  assert(
    (history.results ?? []).length <= 1,
    "Multiple runs have the same operation identity; reconcile explicitly",
  );
  return history.results?.[0]?.runId;
}
function watchRun(run: string, work: string, root: string): void {
  if (watching.has(run) || [...tracked.values()].some((uid) => read(uid)[0]?.run === run)) return;
  const id = `${z.instanceId}-watch-${run}`;
  watching.add(run);
  watchIds.set(id, { run, work });
  void swamp({ operation: "run.attach", payload: { runId: run, afterSeq: 0 }, id }, root)
    .catch(() => {
      // The later status reconciliation reports this failure.
    })
    .finally(() => {
      watching.delete(run);
      watchIds.delete(id);
    });
}
// oxlint-disable-next-line eslint/max-lines-per-function -- This lifecycle handler keeps its ordered protocol transitions together.
async function dispatch(
  uid: string,
  root: string,
  operation: string,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  payload: Record<string, Json>,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  onProgress?: (payload: Json) => void,
): Promise<Delivery> {
  let [value, row] = read(uid);
  assert(value, "No operation with that identity");
  ensureOpen(value.work);
  Object.assign(value, {
    transport: "serve",
    requestId: z.instanceId + uid,
    checkout: root,
    correlation: uid,
    outcome: "unknown",
  });
  put(uid, value, row);
  tracked.set(required(value.requestId), uid);
  if (operation === "workflow.run") {
    // oxlint-disable-next-line eslint/no-param-reassign -- Update the caller-owned state or the local retry state in place.
    payload = {
      ...payload,
      noSupersede: true,
      runtimeTags: {
        // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
        ...(payload["runtimeTags"] as Record<string, Json>),
        yinzOperation: uid,
      },
    };
  }
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
  const reply: { runId?: string } = JSON.parse(
    await swamp(
      {
        operation,
        payload,
        id: value.requestId,
        started: true,
        onProgress,
        onNotDispatched(error) {
          const [current, currentRow] = read(uid);
          assert(current, "Missing dispatch");
          current.not_dispatched = true;
          if (["pending", "unknown"].includes(current.outcome)) {
            current.outcome = "failed";
            current.error = encode(error);
          }
          put(uid, current, currentRow);
        },
      },
      root,
    ),
  );
  [value, row] = read(uid);
  assert(value, "No operation with that identity");
  if (reply.runId) value.run = reply.runId;
  if (value.outcome === "unknown" && tracked.has(required(value.requestId)))
    value.outcome = "pending";
  put(uid, value, row);
  if (closed(value.work)) {
    if (operation !== "model.method.run") cancelIfClosed(value.work);
    ensureOpen(value.work);
  }
  return value;
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function, typescript/prefer-readonly-parameter-types -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together. This helper shares mutable plugin state and SDK wire types with its callers.
async function serveObserve(uid: string, operation: Delivery, row?: DataRecord): Promise<Delivery> {
  let attachment: { ok: boolean; error?: string } | undefined;
  for (let attempt = 1; attempt <= 8; attempt++) {
    if (!["pending", "unknown"].includes(operation.outcome)) return operation;
    // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
    const request: Request = JSON.parse(operation.request);
    if (request.operation === "model.method.run") {
      if (operation.run) {
        const id = `${z.instanceId}-attach-${uid}`;
        if (![...tracked.values()].includes(uid) && !attachment) {
          tracked.set(id, uid);
          // oxlint-disable-next-line eslint/max-depth -- Nested protocol checks preserve the order of validation and state changes.
          try {
            // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
            await swamp(
              {
                operation: "run.attach",
                payload: { runId: operation.run, afterSeq: 0 },
                id,
                started: true,
              },
              required(operation.checkout),
            );
            attachment = { ok: true };
          } catch (error) {
            tracked.delete(id);
            attachment = { ok: false, error: String(error) };
          }
        }
        if (attachment) {
          operation.outcome = attachment.ok && tracked.has(id) ? "pending" : "unknown";
          // oxlint-disable-next-line eslint/max-depth -- Nested protocol checks preserve the order of validation and state changes.
          if (attachment.ok) operation.dispatcher = z.instanceId;
          else operation.error = attachment.error;
        }
      } else operation.outcome = "unknown";
    } else {
      // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
      operation.run = await workflowRunId(operation, request);
      if (operation.run) {
        // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
        const history: History = JSON.parse(
          // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
          await swamp(
            { operation: "workflow.history.get", payload: { workflowIdOrName: operation.run } },
            required(operation.checkout),
          ),
        );
        if (
          request.operation !== "workflow.resume" ||
          history.inputs?.submission ===
            (request.payload["inputs"] != null &&
            typeof request.payload["inputs"] === "object" &&
            !Array.isArray(request.payload["inputs"])
              ? request.payload["inputs"]["submission"]
              : undefined)
        ) {
          // oxlint-disable-next-line eslint/max-depth -- Nested protocol checks preserve the order of validation and state changes.
          if (history.status === "running") {
            // oxlint-disable-next-line eslint/max-depth -- Nested protocol checks preserve the order of validation and state changes.
            if (operation.outcome === "unknown" || operation.dispatcher !== z.instanceId) {
              // oxlint-disable-next-line eslint/max-depth -- Nested protocol checks preserve the order of validation and state changes.
              try {
                // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
                await swamp(
                  {
                    operation: "run.attach",
                    payload: { runId: operation.run, afterSeq: 0 },
                    started: true,
                  },
                  required(operation.checkout),
                );
                operation.outcome = "pending";
                operation.dispatcher = z.instanceId;
              } catch {
                operation.outcome = "unknown";
                operation.error =
                  "Execution is unavailable; reconcile persisted history before proceeding";
              }
            } else operation.outcome = "pending";
          } else if (["succeeded", "suspended"].includes(history.status)) {
            operation.outcome = "completed";
            operation.exitCode = 0;
          } else if (["failed", "cancelled", "interrupted"].includes(history.status)) {
            operation.outcome = "failed";
            operation.exitCode = 1;
          }
        }
      }
    }
    try {
      put(uid, operation, row);
      return operation;
    } catch (error) {
      if (!D.isConflict(error) || attempt === 8) throw error;
    }
    const [fresh, freshRow] = read(uid);
    assert(fresh, "No operation with that identity");
    // oxlint-disable-next-line eslint/no-param-reassign -- Update the caller-owned state or the local retry state in place.
    operation = fresh;
    // oxlint-disable-next-line eslint/no-param-reassign -- Update the caller-owned state or the local retry state in place.
    row = freshRow;
  }
  return operation;
}
const refreshEvents = new Set([
  "started",
  "step_completed",
  "step_failed",
  "suspended",
  "completed",
  "cancelled",
  "error",
  "report_completed",
  "report_failed",
]);
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function changesSnapshot(frame: { type: string; event?: { kind: string } }): boolean {
  return (
    frame.type === "done" ||
    frame.type === "error" ||
    (!!frame.event && refreshEvents.has(frame.event.kind))
  );
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together.
z.on("CustomMessage", ([name, payload]) => {
  if (name !== "swamp-response") return;
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
  const frame: {
      id: string;
      type: string;
      event?: { kind: string; runId?: string; run?: { status: string }; error?: Json };
      error: { code: string };
    } = JSON.parse(payload),
    watch = watchIds.get(frame.id);
  if (watch) {
    if (changesSnapshot(frame)) changed(watch.work);
    return;
  }
  const uid = tracked.get(frame.id);
  if (!uid) return;
  const [operation, row] = read(uid);
  if (!operation) return;
  let write = false;
  const previousRun = operation.run;
  const pending = ["pending", "unknown"].includes(operation.outcome);
  const method = parsedObject(operation.request)["operation"] === "model.method.run";
  if (frame.event?.runId) {
    operation.run = frame.event.runId;
    if (pending) operation.outcome = "pending";
    write = true;
  }
  if (
    pending &&
    frame.event &&
    (["completed", "cancelled"].includes(frame.event.kind) ||
      (!method && frame.event.kind === "suspended"))
  ) {
    const status = frame.event.run?.status;
    const succeeded =
      frame.event.kind !== "cancelled" &&
      (status === "succeeded" || (!method && status === "suspended"));
    operation.outcome = succeeded ? "completed" : "failed";
    operation.exitCode = succeeded ? 0 : 1;
    if (!succeeded) operation.error = encode(frame.event);
    write = true;
  }
  if (pending && frame.event?.kind === "error") {
    operation.error = encode(frame.event.error);
    operation.outcome = "failed";
    operation.exitCode = 1;
    write = true;
  }
  if (frame.type === "done") {
    if (["pending", "unknown"].includes(operation.outcome)) operation.outcome = "unknown";
    tracked.delete(frame.id);
    write = true;
  } else if (frame.type === "error") {
    if (pending) {
      operation.error = encode(frame.error);
      const attaching = method && frame.id === `${z.instanceId}-attach-${uid}`;
      operation.outcome =
        attaching || ["transport_unknown", "host_error", "not_found"].includes(frame.error.code)
          ? "unknown"
          : "failed";
    }
    tracked.delete(frame.id);
    write = true;
  }
  if (write) put(uid, operation, row);
  if (closed(operation.work)) {
    if (!method && operation.run && operation.run !== previousRun) cancelIfClosed(operation.work);
    return;
  }
  if (write || changesSnapshot(frame))
    changed(read<Attachment>(scope(operation.work, "attachment"))[0]?.work ?? operation.work);
});
async function supervisor(pane: number, saved?: string): Promise<Supervisor> {
  for (const row of z.workflowRemotePanes?.() ?? []) {
    if (saved ? row.host_id === saved : row.terminal_id === pane) {
      const info = z.command("GetPaneInfo", terminal(row.terminal_id));
      // Remote identities have no child PID; their owner token guards the process instead.
      return {
        ...row,
        session_name: identity.session,
        attached: true,
        child_exit_status: info?.exited ? info.exit_status : undefined,
      };
    }
  }
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
  const rows: Supervisor[] = JSON.parse(
    await command([z.serverBinary || "yinz", "pty-host", "list", "--json"]),
  );
  const found = rows.find(
    (row) =>
      row.session_name === identity.session &&
      (saved ? row.host_id === saved : row.terminal_id === pane),
  );
  assert(found, "No supervisor identity for pane; reconcile before sending input");
  return found;
}
async function owned(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  args: { work: string; agent: string },
  running = false,
): Promise<Delivery & { pane: number }> {
  ensureOpen(args.work);
  let agent = result(scope(args.work, args.agent));
  assert(
    agent.owner?.host_id === identity.host_id && agent.owner.session === identity.session,
    "Wrong host or session",
  );
  assert(agent.owner.work === args.work && agent.owner.agent === args.agent, "Wrong work or agent");
  assert(agent.pane != null && agent.supervisor, "Agent dispatch is unknown; reconcile it first");
  const current = await supervisor(agent.pane, agent.supervisor);
  ensureOpen(args.work);
  assert(current.host_id === agent.supervisor, "Pane belongs to a different supervisor");
  assert(current.child_pid === agent.process, "Agent process changed");
  assert(current.attached, "Supervisor is detached; adopt and reconcile before sending input");
  if (current.owner)
    assert(
      current.owner === (agent.transportOwner ?? encode(agent.owner)),
      "Remote ownership changed",
    );
  if (current.terminal_id !== agent.pane) {
    const uid = scope(args.work, args.agent),
      [fresh, row] = read(uid);
    assert(fresh, "Agent disappeared");
    fresh.pane = current.terminal_id;
    put(uid, fresh, row);
    agent = fresh;
  }
  if (running)
    assert(!agent.processExited && current.child_exit_status == null, "Agent process exited");
  return { ...agent, pane: current.terminal_id };
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function dispatchPrompt(agent: Delivery): void {
  if (closed(agent.owner?.work)) return;
  if (!agent.ready || !agent.outstanding) return;
  const [operation, row] = read(agent.outstanding);
  if (!operation || operation.outcome !== "pending" || operation.dispatched) return;
  assert(agent.pane != null, "Missing prompt pane");
  operation.dispatched = true;
  // A crash after this point is an uncertain send.
  put(agent.outstanding, operation, row);
  z.agent.send(terminal(agent.pane), string(parsedObject(operation.request)["prompt"]));
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function claim(args: Partial<Request>): [string, Delivery | undefined] {
  ensureOpen(args.work);
  const uid = scope(required(args.work), required(args.key));
  const [previous] = read(uid);
  const request = encode(args);
  if (previous) {
    assert(sameRequest(previous.request, args), "Conflicting arguments for existing operation");
    return [uid, previous];
  }
  const value = {
    request,
    key: args.key,
    work: args.work,
    outcome: "unknown",
    incarnation: identity.incarnation,
    dispatcher: z.instanceId,
  };
  try {
    put(uid, value);
  } catch (error) {
    if (!D.isConflict(error)) throw error;
    // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
    const previous = result(uid);
    assert(sameRequest(previous.request, args), "Conflicting concurrent operation");
    return [uid, previous];
  }
  return [uid, undefined];
}
// oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
function openPicker(work: string, key: string, action: string, source: number): void {
  message({
    plugin_url: "yinz:hocus-new-agent",
    message_name: "swamp-pane-action",
    plugin_config: {
      caller_terminal_pane_id: String(source),
      [action === "fork" ? "fork_focused" : "duplicate_focused"]: "true",
      swamp_reply_plugin: String(z.getPluginIds().plugin_id),
      swamp_work: work,
      swamp_key: key,
      // A modal overlay, as every other New agent launch is (`Ctrl e n/d/f`, the agents
      // list's D and F). A plain floating pane hands the keys to the tiled layer when it
      // closes, not back to where they came from, and hocus reads a tiled pane taking the
      // keys from over its panel as the user leaving it: a cancelled picker put the panel away.
      overlay: "true",
      modal: "true",
    },
    new_plugin_args: {
      should_float: true,
      should_focus: true,
      pane_id_to_replace: null,
      pane_title: null,
      cwd: null,
      skip_cache: false,
    },
  });
}
function openPickerOperation(work: string, action: string): Delivery | undefined {
  const lifecycle = lifecycleWork(work);
  for (const entry of deliveryRows()) {
    const value = entry.value;
    if (
      !["pending", "unknown"].includes(value.outcome) ||
      !value.request ||
      lifecycleWork(entry.work) !== lifecycle
    )
      continue;
    // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
    const request: Request = JSON.parse(value.request);
    if (request.method === "pane_action" && request.action === action && request.cancel_active)
      return value;
  }
  return undefined;
}
const discovered = new Set<string>();
async function discover(work: string, root: string): Promise<Delivery | undefined> {
  const launch = result(scope(work, "launch"));
  assert(launch.checkout === root, "Launch belongs to a different control checkout");
  if (launch.not_dispatched) return read(scope(work, "runs"))[0];
  const exactInputs = Boolean(parsedObject(launch.request ?? "{}")["exactInputs"]);
  const payload: Record<string, Json> = exactInputs
    ? {
        filter: `has(tags.yinzOperation) && tags.yinzOperation == ${encode(scope(work, "launch"))}`,
      }
    : { inputs: { launch: work } };
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
  const history: {
    results?: { runId: string; workflowName: string; inputs: History["inputs"] }[];
  } = JSON.parse(await swamp({ operation: "workflow.history.search", payload }, root));
  for (const run of history.results ?? [])
    if (exactInputs || run.inputs.session === identity.session) {
      await handle({
        method: "workflow_attach",
        work,
        key: "attachment",
        run: run.runId,
        workflow: run.workflowName,
        directory: root,
      });
    }
  return read(scope(work, "runs"))[0];
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together.
async function handle(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  input: Partial<Request>,
  manualView?: number,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  onProgress?: (payload: Json) => void,
): Promise<unknown> {
  assert(
    input != null && typeof input === "object" && !Array.isArray(input),
    "Expected a typed request",
  );
  // Each method validates the fields it consumes; correlation keys are checked by scope().
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- Each method validates its consumed fields before dispatch; unrelated fields may be absent.
  const args = input as Request;
  if (args.method === "identity") return identity;
  if (args.method === "swamp_read") {
    if (args.operation === "workflow.approve")
      ensureOpen(typeof args.payload["runId"] === "string" ? args.payload["runId"] : undefined);
    assert(
      [
        "workflow.history.get",
        "workflow.history.search",
        "data.query",
        "workflow.approve",
      ].includes(args.operation),
      "Unsupported bridge request",
    );
    return JSON.parse(
      await swamp({ operation: args.operation, payload: args.payload, onProgress }, args.directory),
    );
  }
  if (args.method === "workflow_report") {
    assert(
      Number.isSafeInteger(args.offset) &&
        args.offset >= 0 &&
        Number.isSafeInteger(args.offset + 32768),
      "Invalid report offset",
    );
    assert(Number.isSafeInteger(args.version) && args.version > 0, "Invalid report version");
    assert(
      typeof args.model === "string" && typeof args.name === "string",
      "Invalid report identity",
    );
    const predicate = `modelName == ${encode(args.model)} && name == ${encode(args.name)} && version == ${args.version}`;
    // Stored lengths and substring use UTF-16 offsets; CEL size() counts code points.
    const finish = args.offset + 32768;
    const ending = String.raw`(attributes.summaryLength <= ${finish} ? int(attributes.summaryLength) : (attributes.summary.substring(${finish - 1},${finish}).matches(r"[\uD800-\uDBFF]") ? ${finish - 1} : ${finish}))`;
    const select = `{"text":attributes.summary.substring(${args.offset},${ending}),"next":${ending},"total":attributes.summaryLength}`;
    // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
    const response: { results: Json[] } = JSON.parse(
      await swamp(
        {
          operation: "data.query",
          payload: {
            predicate: `${predicate} && has(attributes.summary) && has(attributes.summaryLength) && attributes.summaryLength >= ${args.offset}`,
            select,
          },
        },
        args.directory,
      ),
    );
    assert(response.results.length === 1, "Saved test report unavailable at this version");
    return response.results[0];
  }
  if (args.method === "workflow_answer" || args.method === "workflow_recover") {
    ensureOpen(typeof args.inputs?.["run"] === "string" ? args.inputs["run"] : undefined);
    let inputs = args.inputs ?? {};
    if (args.method === "workflow_recover") {
      const question = key(string(inputs["question"])),
        run = key(string(inputs["run"]));
      const attachment = read<Attachment>(scope(run, "attachment"))[0];
      const config = z.projectConfig.read(args.directory);
      assert(!config.error, config.error ?? "Invalid project config");
      assert(
        attachment?.root === config.root,
        "Question run belongs to a different control checkout",
      );
      const definition = parsedObject(
        await swamp(
          { operation: "model.get", payload: { modelIdOrName: args.model } },
          config.root,
        ),
      );
      assert(
        definition["type"] === "@hammz/question",
        "Only the workflow question model may recover answers",
      );
      const questions = parsedObject(
        await swamp(
          {
            operation: "data.query",
            payload: {
              predicate: `modelName == ${encode(args.model)} && specName == "question" && attributes.run == ${encode(run)} && attributes.question == ${encode(question)}`,
              select: '{"attributes":attributes}',
            },
          },
          config.root,
        ),
      );
      const results = questions["results"];
      assert(
        Array.isArray(results) && results.length === 1,
        "Question is absent or ambiguous; inspect its saved state",
      );
      const saved = object(object(results[0])["attributes"]);
      assert(
        saved["run"] === run && saved["question"] === question,
        "Question does not belong to this run",
      );
      ensureOpen(run);
      inputs = { run, question };
    }
    // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
    const reply: { run?: { status: string } } = JSON.parse(
      await swamp(
        {
          operation: "model.method.run",
          payload: {
            modelIdOrName: args.model,
            methodName: args.method === "workflow_recover" ? "recover" : "submit",
            inputs,
          },
          onProgress,
        },
        args.directory,
      ),
    );
    if (["failed", "cancelled"].includes(reply.run?.status ?? ""))
      throw new Error(encode(reply.run));
    return reply.run ?? reply;
  }
  if (args.method === "workflow_cancel") {
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
    const snapshot = (await handle({
      method: "workflow_show",
      directory: args.directory,
      run: args.run,
    })) as { root: string; history: History; children: History[] };
    ensureOpen(args.run);
    const operation = await cancelRun(args.run, snapshot.root);
    return operation == null
      ? { operations: [], alreadyFinished: true }
      : { operations: [operation] };
  }
  if (args.method === "workflow_show") {
    const failed = args.launch ? read(scope(args.launch, "launch"))[0] : undefined;
    if (failed?.outcome === "failed" && failed.error)
      return launchFailure(args.launch, failed, failed.error, args.directory);
    assert(args.directory, "Missing control checkout");
    const config = z.projectConfig.read(args.directory);
    assert(!config.error, config.error ?? "Invalid project config");
    let attachment: Attachment | undefined, work: string | undefined;
    if (args.run) {
      [attachment] = read<Attachment>(scope(args.run, "attachment"));
      if (attachment) work = attachment.work;
      else {
        // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
        const history: History = JSON.parse(
          await swamp(
            { operation: "workflow.history.get", payload: { workflowIdOrName: args.run } },
            config.root,
          ),
        );
        assert(history.inputs.session === identity.session, "Run belongs to a different session");
        work = history.inputs.launch;
      }
    } else if (args.launch) work = args.launch;
    if (closed(work))
      return {
        run: args.run,
        launch: work,
        root: config.root,
        history: { id: args.run ?? work, status: "cancelled" },
        children: [],
        active: false,
        questions: { results: [] },
        outputs: { results: [] },
        logs: "Pane workflow lifecycle is closed.",
      };
    if (work && manualView != null) attachManual(work, manualView);
    let runs = work ? read(scope(work, "runs"))[0] : undefined;
    if (work) {
      assert(
        result(scope(work, "launch")).checkout === config.root,
        "Launch belongs to a different control checkout",
      );
      const launch = result(scope(work, "launch"));
      if (
        Boolean(parsedObject(launch.request ?? "{}")["exactInputs"]) ||
        !discovered.has(work) ||
        !runs
      ) {
        runs = await discover(work, config.root);
        discovered.add(work);
      }
    }
    if (args.run) [attachment] = read<Attachment>(scope(args.run, "attachment"));
    else if (runs)
      for (const [id, entry] of Object.entries(runs.entries ?? {})) {
        if (entry.workflow === "@hammz/yinz-agent-exit" || !attachment)
          attachment = { ...entry, run: id };
      }
    if (!attachment && work) {
      let runner = read(scope(work, "swamp-process"))[0];
      if (runner && ["completed", "failed", "process-exit"].includes(runner.outcome)) {
        // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
        runner = (await handle({ method: "observe", work, key: "swamp-process" })) as Delivery;
        let logs = `Workflow runner exited without reporting a run. Exit status: ${runner.exitCode}`;
        if (runner.output)
          logs += `\nRetained command scrollback (may omit older output):\n${runner.output}`;
        if (runner.logError) logs += `\nLog unavailable: ${runner.logError}`;
        return launchFailure(work, result(scope(work, "launch")), logs);
      }
    }
    assert(
      attachment,
      "Launch has no reported run yet; reconcile by launch token without repeating it",
    );
    const run = attachment.run;
    // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
    const history: History = JSON.parse(
      await swamp(
        { operation: "workflow.history.get", payload: { workflowIdOrName: run } },
        config.root,
      ),
    );
    const ids = Object.keys(result(scope(attachment.work, "runs")).entries ?? {});
    const questions: unknown = JSON.parse(
      await swamp(
        {
          operation: "data.query",
          payload: {
            predicate: `specName == "question" && attributes.run in ${encode(ids)}`,
            select: '{"model": modelName, "version": version, "attributes": attributes}',
          },
        },
        config.root,
      ),
    );
    const outputs: unknown = JSON.parse(
      await swamp(
        {
          operation: "data.query",
          payload: {
            predicate: `version >= 0 && has(tags.workflowRunId) && tags.workflowRunId in ${encode(ids)}`,
            select: outputProjection,
          },
        },
        config.root,
      ),
    );
    const children = [];
    if (history.status === "running") watchRun(run, attachment.work, config.root);
    for (const id of ids)
      if (id !== run) {
        // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
        const child: History = JSON.parse(
          await swamp(
            { operation: "workflow.history.get", payload: { workflowIdOrName: id } },
            config.root,
          ),
        );
        children.push(child);
        if (child.status === "running") watchRun(child.id, attachment.work, config.root);
      }
    return {
      run,
      launch: attachment.work,
      root: config.root,
      history,
      children,
      active: active(attachment.work),
      activeOperation: activeOperation(attachment.work) ?? null,
      outputs,
      logs: await swamp(
        { operation: "workflow.history.logs", payload: { runIdOrWorkflow: run, tail: 100 } },
        config.root,
      ),
      questions,
    };
  }
  key(args.work);
  if (args.method === "lookup") return read(scope(args.work, args.key))[0] ?? null;
  if (args.method === "observe") {
    const uid = scope(args.work, args.key),
      [operation, row] = read(uid);
    assert(operation, "No operation with that identity");
    if (operation.transport === "serve") return serveObserve(uid, operation, row);
    if (operation.outcome === "pending" && operation.dispatcher !== z.instanceId) {
      operation.outcome = "unknown";
      put(uid, operation, row);
    }
    if (
      args.includeOutput !== false &&
      // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
      !Boolean(operation.capture) &&
      operation.log &&
      operation.pane != null &&
      operation.supervisor
    ) {
      try {
        const host = await supervisor(operation.pane, operation.supervisor);
        assert(host.host_id === operation.supervisor, "Log belongs to a different supervisor");
        assert(host.child_pid === operation.process, "Log process changed");
        assert(host.attached, "Log supervisor is detached");
        operation.pane = host.terminal_id;
        operation.log = `terminal_${host.terminal_id}`;
        const contents = z.command("GetPaneScrollback", {
          pane_id: terminal(host.terminal_id),
          get_full_scrollback: true,
        });
        operation.output = [
          ...contents.lines_above_viewport,
          ...contents.viewport,
          ...contents.lines_below_viewport,
        ].join("\n");
        operation.outputTruncated = true;
      } catch (error) {
        operation.logError = String(error);
      }
    }
    return operation;
  }
  if (args.method === "command_close") {
    const uid = scope(args.work, args.key);
    const operation = result(uid);
    assert(parsedObject(operation.request)["method"] === "command", "Not a command operation");
    assert(
      operation.owner?.host_id === identity.host_id &&
        operation.owner.session === identity.session &&
        operation.owner.work === args.work &&
        operation.owner.agent === args.key,
      "Wrong command ownership",
    );
    assert(
      ["completed", "failed"].includes(operation.outcome) ||
        (operation.outcome === "cancelled" && operation.terminationVerified === true),
      "Command has not completed",
    );
    assert(
      operation.pane != null && operation.supervisor && operation.process != null,
      "Missing command identity",
    );
    if (operation.closed) return operation;
    const pane = operation.pane;
    const info = z.command("GetPaneInfo", terminal(pane));
    if (info) {
      const host = await supervisor(pane, operation.supervisor);
      assert(
        host.host_id === operation.supervisor && host.terminal_id === pane && host.attached,
        "Command supervisor changed",
      );
      assert(host.child_pid === operation.process, "Command process changed");
      if (host.owner)
        assert(
          host.owner === (operation.transportOwner ?? encode(operation.owner)),
          "Command ownership changed",
        );
      assert(z.command("GetPaneInfo", terminal(pane))?.exited, "Command is still running");
    }
    const placement = placements.get(args.work);
    if (placement) {
      placement.queue = placement.queue.filter((entry) => entry !== uid);
      present(args.work);
      if (placement.current === pane) placement.current = undefined;
    }
    const viewUid = scope(lifecycleWork(args.work), "view"),
      [view, viewRow] = read(viewUid);
    if (view?.current === pane) {
      delete view.current;
      put(viewUid, view, viewRow);
    }
    // Replacement can briefly remove the pane from the manifest; close the verified identity anyway.
    if (info) z.command("CloseTerminalPane", pane);
    const [fresh, row] = read(uid);
    assert(fresh, "Command disappeared");
    fresh.closed = true;
    put(uid, fresh, row);
    return fresh;
  }
  if (args.method === "step_ready") {
    assert(
      !activeOperation(args.work),
      "An owned operation is still active or uncertain; cancel or reconcile it before another step",
    );
    return {
      work: args.work,
      key: args.key,
      request: encode(args),
      incarnation: identity.incarnation,
      outcome: "completed",
    };
  }
  if (args.method === "step_finished") {
    const uid = scope(args.work, args.operation),
      [value, row] = read(uid);
    assert(
      value?.owner &&
        value.process === args.process &&
        value.owner.work === args.work &&
        value.owner.host_id === identity.host_id &&
        value.owner.session === identity.session,
      "Completion belongs to a different process",
    );
    value.executionFinished = true;
    put(uid, value, row);
    return value;
  }
  if (args.method === "cancel_step") {
    ensureOpen(args.work);
    const uid = scope(args.work, args.operation),
      [value, row] = read(uid);
    assert(value?.request, "No operation with that identity");
    assert(cancellable(value.request), "This operation has no cancellable process");
    if (!["pending", "unknown"].includes(value.outcome)) return value;
    if (!value.cancelRequested) {
      value.cancelRequested = true;
      put(uid, value, row);
    }
    stopOperation(uid);
    return result(uid);
  }
  if (args.method === "agent_inspect") return owned(args);
  if (args.method === "workflow_attach") {
    const entry: Attachment = {
      run: args.run,
      work: args.work,
      workflow: args.workflow,
      root: args.directory,
    };
    const uid = scope(args.run, "attachment"),
      [existing, row] = read(uid);
    assert(!existing || encode(existing) === encode(entry), "Run belongs to a different launch");
    if (!existing) put(uid, entry, row);
    const runsUid = scope(args.work, "runs");
    for (let attempt = 1; attempt <= 8; attempt++) {
      const [saved, runsRow] = read<{ entries: Record<string, Attachment> }>(runsUid);
      const runs = saved ?? { entries: {} };
      if (runs.entries[args.run]) break;
      runs.entries[args.run] = entry;
      try {
        put(runsUid, runs, runsRow);
        break;
      } catch (error) {
        if (!D.isConflict(error) || attempt === 8) throw error;
      }
    }
    if (cancelIfClosed(args.work)) ensureOpen(args.work);
    if (!existing) changed(args.work);
    return {
      work: args.work,
      key: args.key,
      request: encode(args),
      incarnation: identity.incarnation,
      outcome: "completed",
    };
  }
  ensureOpen(args.work);
  if (args.method === "reconcile") {
    const uid = scope(args.work, args.operation),
      [operation, row] = read(uid);
    assert(operation, "Operation is absent");
    if (parsedObject(operation.request)["method"] === "workflow_run" && operation.checkout)
      await discover(args.work, operation.checkout);
    if (operation.transport === "serve") return serveObserve(uid, operation, row);
    if (operation.pane != null && operation.supervisor) {
      const host = await supervisor(operation.pane, operation.supervisor);
      assert(
        host.host_id === operation.supervisor,
        "Supervisor mismatch; attachment remains unknown",
      );
      assert(host.child_pid === operation.process, "Supervisor process changed");
      assert(host.attached, "Supervisor is detached; adopt before reconciling");
      if (host.owner)
        assert(
          host.owner === (operation.transportOwner ?? encode(operation.owner)),
          "Remote ownership changed",
        );
      operation.pane = host.terminal_id;
      if (operation.log) operation.log = `terminal_${host.terminal_id}`;
      if (host.child_exit_status === 130) operation.cancelRequested = true;
      if (operation.cancelRequested && host.child_group_terminated !== true) {
        operation.outcome = "unknown";
        operation.terminationVerified = false;
      } else if (host.child_exit_status != null || host.child_group_terminated === true) {
        operation.terminationVerified = host.child_group_terminated === true;
        operation.processExited = true;
        operation.outcome = operation.cancelRequested
          ? "cancelled"
          : parsedObject(operation.request)["method"] === "command"
            ? host.child_exit_status === 0
              ? "completed"
              : "failed"
            : "process-exit";
        operation.exitCode = host.child_exit_status;
      } else if (operation.outcome === "pending" && operation.incarnation !== identity.incarnation)
        operation.outcome = "unknown";
      put(uid, operation, row);
      if (operation.cancelRequested && operation.outcome === "unknown") stopOperation(uid);
    }
    return operation;
  }
  const prompt =
    args.method === "agent_prompt"
      ? args.prompt
      : args.method === "agent_spawn"
        ? args.spec?.prompt
        : undefined;
  if (
    ["command", "agent_spawn", "agent_prompt"].includes(args.method) &&
    !read(scope(args.work, args.key))[0]
  )
    assert(
      !deliveryRows().some(
        (entry) =>
          lifecycleWork(entry.work) === lifecycleWork(args.work) &&
          entry.value.cancelRequested &&
          ["pending", "unknown"].includes(entry.value.outcome),
      ),
      "Cancellation has not finished; reconcile the owned operation before starting another",
    );
  if (args.method === "agent_prompt" || prompt != null)
    assert(
      typeof prompt === "string" &&
        /\S/u.test(prompt) &&
        !prompt.includes("\0") &&
        z.encodeUtf8(prompt).length <= 262144,
      "Prompt must contain 1–262144 UTF-8 bytes (256 KiB), must not be blank or contain NUL",
    );
  if (
    args.method === "pane_action" &&
    args.cancel_active &&
    ["fork", "duplicate"].includes(args.action)
  ) {
    const open = openPickerOperation(args.work, args.action);
    if (open) {
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
      const request: Request = JSON.parse(open.request),
        source = await owned({ work: request.work, agent: "source" });
      assert(
        z.command("GetPaneInfo", terminal(source.pane))?.exited,
        "Source pane is no longer exited",
      );
      openPicker(request.work, request.key, request.action, source.pane);
      return open;
    }
  }
  let row: DataRecord | undefined, uid: string | undefined, value: Delivery | undefined;
  if (args.method === "vm_status")
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
    value = {
      work: args.work,
      key: args.key,
      request: encode(args),
      incarnation: identity.incarnation,
    } as Delivery;
  else {
    const [claimed, previous] = claim(args);
    uid = claimed;
    if (previous) return previous;
    [value, row] = read(uid);
    assert(value, "No operation with that identity");
  }
  if (args.method === "workflow_run") {
    assert(args.directory, "Missing project directory");
    const config = z.projectConfig.read(args.directory);
    assert(!config.error, config.error ?? "Invalid project config");
    const workflow = args.workflow ?? config.config.workflow;
    assert(workflow, "No exit workflow configured. Use Hocus /project-setup.");
    const { name } = parsedObject(
      await swamp(
        { operation: "workflow.get", payload: { workflowIdOrName: workflow }, onProgress },
        config.root,
      ),
    );
    assert(typeof name === "string" && name, "Workflow has no canonical name");
    const validation = await swamp(
      { operation: "workflow.validate", payload: { workflowIdOrName: name }, onProgress },
      config.root,
    );
    ensureOpen(args.work);
    assert(
      parsedObject(validation)["passed"] === true,
      "Workflow validation failed: " + setupDiagnostic(validation, config.root),
    );
    const inputs = { ...args.inputs };
    if (args.exactInputs) {
      // General workflows own their input schema; dispatch carries ownership in runtime tags.
      Object.assign(value, { workflow: name, checkout: config.root, launch: args.work });
      put(required(uid), value, row);
      return dispatch(
        required(uid),
        config.root,
        "workflow.run",
        { workflowIdOrName: name, inputs },
        onProgress,
      );
    }
    inputs["launch"] = args.work;
    inputs["session"] = identity.session;
    inputs["binary"] = z.serverBinary || "yinz";
    inputs["checkout"] ??= (
      await command(["git", "rev-parse", "--show-toplevel"], args.directory)
    ).replace(/\n$/u, "");
    ensureOpen(args.work);
    if (typeof inputs["sourcePane"] === "number" && inputs["sourcePane"] >= 0) {
      const pane = z.command("GetPaneInfo", terminal(inputs["sourcePane"]));
      assert(pane?.exited, "Source pane must be exited");
      const host = await supervisor(inputs["sourcePane"]);
      ensureOpen(args.work);
      put(scope(args.work, "source"), {
        pane: inputs["sourcePane"],
        supervisor: host.host_id,
        process: host.child_pid,
        target: pane.target,
        transportOwner: host.owner,
        owner: { ...identity, work: args.work, agent: "source" },
      });
      inputs["kind"] = pane.agent?.kind ?? inputs["kind"] ?? "";
      const vm =
        typeof pane.target === "object" && "Firecracker" in pane.target
          ? pane.target.Firecracker
          : undefined;
      if (vm) {
        inputs["firecracker"] = vm.vm;
        inputs["firecrackerImage"] = vm.image ?? "";
        // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
      } else assert(!Boolean(inputs["firecracker"]), "Local source cannot select a different VM");
    }
    assert(
      // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
      !Boolean(inputs["firecracker"]) ||
        (typeof inputs["sourcePane"] === "number" && inputs["sourcePane"] >= 0),
      "VM work requires its original pane identity",
    );
    inputs["target"] ??= config.root;
    inputs["gate"] ??= config.config.ci ?? ["cargo", "x", "ci", "check"];
    inputs["targetBranch"] ??= config.config.default_branch ?? "main";
    if (config.config.rebase_prompt != null) inputs["rebasePrompt"] ??= config.config.rebase_prompt;
    for (const [field, setting] of [
      ["commitAgent", "commit_agent"],
      ["reviewAgent", "review_agent"],
      ["reviewFixAgent", "review_fix_agent"],
      ["rebaseAgent", "rebase_agent"],
      ["testFixAgent", "test_fix_agent"],
    ] as const) {
      const selected = config.config[setting] ?? config.config.agent;
      inputs[field] ??= selected
        ? {
            ...(selected.kind == null && inputs["kind"] == null
              ? {}
              : { kind: selected.kind ?? inputs["kind"] ?? "" }),
            ...(selected.model == null ? {} : { model: selected.model }),
            ...(selected.effort == null ? {} : { effort: selected.effort }),
          }
        : {
            ...(inputs["kind"] == null ? {} : { kind: inputs["kind"] }),
            command: inputs["agentCommand"] ?? "",
            ...(inputs["agentArgs"] == null ? {} : { args: inputs["agentArgs"] }),
          };
    }
    inputs["flowVersion"] ??= 3;
    Object.assign(value, { workflow: name, checkout: config.root, launch: args.work });
    put(required(uid), value, row);
    return dispatch(
      required(uid),
      config.root,
      "workflow.run",
      { workflowIdOrName: name, inputs },
      onProgress,
    );
  } else if (args.method === "swamp_dispatch") {
    if (args.operation === "workflow.resume")
      ensureOpen(typeof args.payload["runId"] === "string" ? args.payload["runId"] : undefined);
    return dispatch(
      required(uid),
      args.directory,
      args.operation,
      {
        ...args.payload,
        _yinz_active_callback: true,
      },
      onProgress,
    );
  } else if (args.method === "pane_action") {
    let source: { pane: number } | undefined;
    const saved = read(scope(args.work, "source"))[0];
    const view = read(scope(args.work, "view"))[0];
    if (
      args.action === "restart" &&
      !saved &&
      view?.incarnation === identity.incarnation &&
      view.source != null
    )
      source = { pane: view.source };
    else if (args.action !== "leave" || saved) {
      // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
      const closed =
        args.action === "leave" &&
        saved?.owner?.incarnation === identity.incarnation &&
        saved.pane != null &&
        !z.command("GetPaneInfo", terminal(saved.pane));
      if (closed)
        assert(
          saved.owner?.host_id === identity.host_id &&
            saved.owner.session === identity.session &&
            saved.owner.work === args.work &&
            saved.owner.agent === "source" &&
            saved.supervisor,
          "Wrong source ownership",
        );
      else source = await owned({ work: args.work, agent: "source" });
    }
    if (source) {
      const pane = z.command("GetPaneInfo", terminal(source.pane));
      assert(pane?.exited, "Source pane is no longer exited");
      if (args.action === "restart" && pane.restart_error != null)
        throw new Error(pane.restart_error);
    }
    if (args.action === "leave") {
      transition(args.work, "leave");
      if (source) z.command("CloseTerminalPane", source.pane);
      value.outcome = "completed";
      put(required(uid), value, row);
      closeView(args.work);
      return value;
    }
    assert(source, "Missing source pane");
    if (args.action === "restart") {
      transition(args.work, "restart");
      if (args.cancel_active) cancelAfterTransition(args.work);
      closeView(args.work, source.pane);
      z.command("RerunCommandPane", source.pane);
      value.outcome = "completed";
      put(required(uid), value, row);
    } else {
      assert(args.action === "fork" || args.action === "duplicate", "Unknown pane choice");
      value.outcome = "pending";
      put(required(uid), value, row);
      openPicker(args.work, args.key, args.action, source.pane);
    }
  } else if (args.method === "agent_spawn") {
    assert(args.spec != null, "Missing agent specification");

    const owner = { ...identity, work: args.work, agent: args.key };
    value.owner = owner;
    value.outcome = "pending";
    put(required(uid), value, row);
    // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
    const { source_directory, automatic_firecracker_vm, prompt, ...spec } = args.spec;
    const opts = {
      ...spec,
      ...(prompt == null ? {} : { prompt }),
      ...(source_directory === undefined ? {} : { sourceDirectory: source_directory }),
      ...(automatic_firecracker_vm === undefined
        ? {}
        : { automaticFirecrackerVm: automatic_firecracker_vm }),
      context: { swamp_owner: encode(owner), swamp_operation: args.key },
      startSuppressed: placements.has(args.work) && placements.get(args.work)?.detached !== true,
    };
    const spawned = z.agent.spawnDetails(opts);
    assert("Terminal" in spawned.pane_id, "Expected an agent terminal");
    [value, row] = read(required(uid));
    assert(value, "No operation with that identity");
    value.pane = spawned.pane_id.Terminal;
    value.branch = spawned.branch ?? null;
    value.checkout = spawned.cwd ?? null;
    put(required(uid), value, row);
    if (closed(args.work)) {
      z.command("CloseTerminalPane", value.pane);
      ensureOpen(args.work);
    }
    const info = z.command("GetPaneInfo", terminal(value.pane));
    [value, row] = read(required(uid));
    assert(value, "No operation with that identity");
    value.target = info?.target;
    put(required(uid), value, row);
    let host: Supervisor;
    if (typeof value.target === "object" && "Firecracker" in value.target) {
      const deadline = Date.now() + 150000;
      for (;;) {
        try {
          // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
          host = await supervisor(required(value.pane));
          break;
        } catch {
          ensureOpen(args.work);
          assert(
            Date.now() < deadline,
            "VM dispatch has a pane but no verified transport; reconcile it without repeating spawn",
          );
          // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
          await z.sleep(200);
        }
      }
    } else host = await supervisor(required(value.pane));
    [value, row] = read(required(uid));
    assert(value, "No operation with that identity");
    Object.assign(value, {
      supervisor: host.host_id,
      process: host.child_pid,
      remoteHost: host.remote_host,
      remotePaneId: host.pane_token,
    });
    if (!value.processExited) value.outcome = args.waitForExit ? "pending" : "completed";
    put(required(uid), value, row);
    ensureOpen(args.work);
    const placement = placements.get(args.work);
    if (args.waitForExit && placement && !placement.detached) {
      placement.queue.push(required(uid));
      present(args.work);
    }
  } else if (args.method === "agent_prompt") {
    await owned(args, true);
    const agentUid = scope(args.work, args.agent),
      [agent, agentRow] = read(agentUid);
    assert(agent && agentRow && row, "Agent disappeared");
    assert(!agent.outstanding, "One outstanding prompt is permitted per agent");
    Object.assign(value, {
      owner: agent.owner,
      pane: agent.pane,
      outcome: "pending",
      supervisor: agent.supervisor,
      process: agent.process,
      remoteHost: agent.remoteHost,
      remotePaneId: agent.remotePaneId,
      checkout: agent.checkout,
      branch: agent.branch,
      target: agent.target,
    });
    agent.outstanding = uid;
    // Publish observation and the exclusive prompt slot in one transaction before sending bytes.
    D.apply([
      D.put(
        "delivery",
        { value: encode(value) },
        { uid: required(uid), expectRev: row.rev ?? null },
      ),
      D.put(
        "delivery",
        { value: encode(agent) },
        { uid: agentUid, expectRev: agentRow.rev ?? null },
      ),
    ]);
    const placement = placements.get(args.work);
    if (placement && !placement.detached) {
      placement.queue.push(required(uid));
      present(args.work);
    }
    dispatchPrompt(agent);
  } else if (args.method === "command") {
    assert(
      Array.isArray(args.argv) &&
        args.argv.length > 0 &&
        args.argv.every((arg) => typeof arg === "string"),
      "Missing literal argv",
    );
    const owner = { ...identity, work: args.work, agent: args.key };
    value.owner = owner;
    value.outcome = "pending";
    value.capture = args.capture;
    put(required(uid), value, row);
    const pane = z.command("OpenCommandPaneBackground", [
      { path: required(args.argv[0]), args: args.argv.slice(1), cwd: args.cwd ?? null },
      {
        swamp_owner: encode(owner),
        swamp_operation: args.key,
        swamp_work: read<Attachment>(scope(args.work, "attachment"))[0]?.work ?? args.work,
      },
    ]);
    [value, row] = read(required(uid));
    assert(value, "No operation with that identity");
    assert(
      pane && "Terminal" in pane,
      "Command dispatch was not acknowledged; reconcile the saved operation before any further action",
    );
    value.pane = pane.Terminal;
    value.checkout = args.cwd;
    value.log = `terminal_${value.pane}`;
    put(required(uid), value, row);
    if (closed(args.work)) {
      z.command("CloseTerminalPane", value.pane);
      ensureOpen(args.work);
    }
    const placement = placements.get(args.work);
    if (args.foreground && placement && !placement.detached) {
      placement.queue.push(required(uid));
      present(args.work);
    }
    const host = await supervisor(value.pane);
    [value, row] = read(required(uid));
    assert(value, "No operation with that identity");
    value.supervisor = host.host_id;
    value.process = host.child_pid;
    put(required(uid), value, row);
    ensureOpen(args.work);
  } else if (["source_sync", "vm_inspect", "vm_sync", "vm_status"].includes(args.method)) {
    const agent: Partial<Delivery> =
      args.method === "source_sync"
        ? read(scope(args.work, "source"))[0]
          ? await owned({ work: args.work, agent: "source" })
          : { target: "Local" }
        : await owned(args);
    value.owner = agent.owner;
    value.target = agent.target;
    const target =
      typeof agent.target === "object" && "Firecracker" in agent.target
        ? agent.target.Firecracker
        : undefined;
    if (target) {
      const name = target.vm;
      assert(name, "Missing VM identity");
      let output = "",
        warnings = "";
      if (args.method === "vm_sync" || args.method === "source_sync") {
        const r = await z.runCommand({
          command: [
            z.serverBinary || "yinz",
            "firecracker",
            "sync",
            name,
            "--session",
            identity.session,
          ],
          cwd: "/",
          context: { swamp_owner: encode(identity) },
          maxBytesReturned: 1048576,
        });
        warnings = z.decodeUtf8(r.stderr);
        output = z.decodeUtf8(r.stdout);
        assert(
          r.exitCode === 0,
          `VM sync failed; inspect provider reports before retrying: ${warnings}`,
        );
      }
      const count = await command([
        z.serverBinary || "yinz",
        "firecracker",
        "changes",
        name,
        "--session",
        identity.session,
      ]);
      assert(/^\d+\s*$/u.test(count), "Invalid guest change count");
      Object.assign(value, {
        outcome: "completed",
        vm: name,
        output: output + warnings + count,
        clean: Number(count) === 0 && warnings === "",
      });
    } else {
      Object.assign(value, { outcome: "completed", vm: null, clean: true });
    }
    if (uid) put(uid, value, row);
    else return value;
  } else throw new Error("Unknown bridge method");
  return result(required(uid));
}
type History = {
  id: string;
  workflowId: string;
  workflowName: string;
  status: string;
  inputs: { launch: string; session: string; submission?: Json };
  jobs?: { name: string; steps?: { name: string; status: string }[] }[];
};
// Bound every saved log copy before the host serializes the response; its byte cap would cut JSON.
// CEL substring uses UTF-16 offsets: back off if the cut would split a surrogate pair.
function preview(path: string, field = "output", limit = 16384): string {
  return String.raw`"output":has(VALUE.output) ? (VALUE.output.size() > 16384 ? VALUE.output.substring(0,
    VALUE.output.substring(16383,16384).matches(r"[\uD800-\uDBFF]") ? 16383 : 16384) : VALUE.output) : "",
    "previewTruncated":has(VALUE.output) && VALUE.output.size() > 16384`
    .replaceAll("output", field)
    .replaceAll("16383", String(limit - 1))
    .replaceAll("16384", String(limit))
    .replaceAll("VALUE", path);
}
const streamPreview = (field: string): string =>
  preview("attributes", field, 2048).replace("previewTruncated", `${field}Truncated`);
function testPreview(path: string): string {
  return `{"evidence":${path}.evidence,"outcome":${path}.outcome,"exitCode":${path}.exitCode,"summaryLength":has(${path}.summaryLength) ? ${path}.summaryLength : 0,"outputComplete":has(${path}.outputComplete) && ${path}.outputComplete,"log":${path}.log,"passed":${path}.passed,"outputRef":has(${path}.outputRef) ? ${path}.outputRef : null,${preview(path)}}`;
}
const projection = `{"model":modelName,"name":name,"version":version,"createdAt":createdAt,"attributes":
  has(tags.type) && tags.type == "report" ? {} :
  specName == "claim" ? {"key":attributes.key,"fingerprint":attributes.fingerprint,"outcome":attributes.outcome} :
  specName == "commandOutput" ? {} :
  specName == "rebaseResult" ? {"exitCode":attributes.exitCode,"evidence":attributes.evidence,
    "outputRef":has(attributes.outputRef) ? attributes.outputRef : null,
    "operation":has(attributes.operation) ? {
      "key":attributes.operation.key,"work":attributes.operation.work,"outcome":attributes.operation.outcome,
      "exitCode":has(attributes.operation.exitCode) ? attributes.operation.exitCode : null,
      "log":has(attributes.operation.log) ? attributes.operation.log : "",
      "pane":has(attributes.operation.pane) ? attributes.operation.pane : false} : {},
    ${streamPreview("stdout")},${streamPreview("stderr")}} :
  specName == "diff" ? {"evidence":attributes.evidence,${preview("attributes")}} :
  specName == "test" ? ${testPreview("attributes")} :
  specName == "operation" ? {
    "outcome":attributes.outcome,"pane":has(attributes.pane) ? attributes.pane : false,
    "log":has(attributes.log) ? attributes.log : "", "exitCode":has(attributes.exitCode) ? attributes.exitCode : null,
    "outputTruncated":has(attributes.outputTruncated) && attributes.outputTruncated,
    "logError":has(attributes.logError) ? attributes.logError : false,${preview("attributes")}} :
  specName in ["checks","assessment"] ? {"review":attributes.review,
    "test":attributes.test == null ? attributes.test : ${testPreview("attributes.test")},
    "evidence":has(attributes.evidence) ? attributes.evidence : null,
    "tests":has(attributes.tests) ? attributes.tests : "", "codeReview":has(attributes.codeReview) ? attributes.codeReview : "",
    "reason":has(attributes.reason) ? attributes.reason : null,
    "recommendation":has(attributes.recommendation) ? attributes.recommendation : null,
    "repairReview":has(attributes.repairReview) && attributes.repairReview,
    "repairTests":has(attributes.repairTests) && attributes.repairTests,
    "ready":has(attributes.ready) && attributes.ready} : attributes}`;
// Share the attributes expression to stay within Swamp's 4096-character select limit.
const outputProjection = `[attributes].map(a,${projection.replaceAll("attributes.", "a.")})[0]`;
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function message(options: Partial<MessageToPlugin> & { message_name: string }): void {
  z.command("MessageToPlugin", {
    plugin_url: null,
    destination_plugin_id: null,
    plugin_config: {},
    message_payload: null,
    message_args: {},
    new_plugin_args: null,
    floating_pane_coordinates: null,
    ...options,
  });
}
let timingAttempt = 0;
async function invoke(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  args: Partial<Request>,
  manualView?: number,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  onProgress?: (payload: Json) => void,
): Promise<Json> {
  if (!z.swampTiming) return invokeInner(args, manualView, onProgress);
  timingAttempt += 1;
  const attempt = timingAttempt,
    start = z.monotonicMs();
  let outcome = "error",
    response: Json | undefined;
  try {
    response = await invokeInner(args, manualView, onProgress);
    outcome = "ok";
    return response;
  } finally {
    try {
      z.log(
        "swamp_timing " +
          encode({
            layer: "deno",
            phase: "invoke",
            attempt: `${z.instanceId}:${attempt}`,
            elapsedMs: z.monotonicMs() - start,
            outcome,
            method: args?.method,
            work: args?.work,
            key: args?.key,
            requestId:
              response != null && typeof response === "object" && !Array.isArray(response)
                ? response["requestId"]
                : undefined,
            run:
              response != null && typeof response === "object" && !Array.isArray(response)
                ? response["run"]
                : undefined,
          }),
      );
    } catch {
      /* Keep the operation's result. */
    }
  }
}
// oxlint-disable-next-line eslint/max-lines-per-function, eslint/complexity -- Keep request ownership, completion and error persistence in one ordered lifecycle.
async function invokeInner(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  args: Partial<Request>,
  manualView?: number,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  onProgress?: (payload: Json) => void,
): Promise<Json> {
  const newLaunch =
    args?.method === "workflow_run" && !read(scope(required(args.work), required(args.key)))[0];
  let response: unknown;
  try {
    const run = args.inputs?.["run"] ?? args.payload?.["runId"];
    const flight = {
      work: args.work ?? args.run ?? args.launch ?? (typeof run === "string" ? run : undefined),
    };
    inFlight.add(flight);
    try {
      response = await handle(args, manualView, onProgress);
    } finally {
      inFlight.delete(flight);
      if (
        flight.work &&
        (args.method === "workflow_answer" ||
          args.method === "workflow_recover" ||
          (args.method === "swamp_read" && args.operation === "workflow.approve"))
      )
        cancelIfClosed(flight.work);
    }
  } catch (error) {
    if (args?.method === "workflow_show") {
      for (const [work, placement] of placements)
        if (placement.view === manualView) unavailable(placement, work);
    }
    try {
      if (
        newLaunch &&
        !read(scope(required(args.work), "swamp-process"))[0] &&
        !read(scope(required(args.work), required(args.key)))[0]?.transport
      )
        saveEvent(scope(required(args.work), required(args.key)), "failed", {
          error: String(error),
        });
      const [value] = read(scope(required(args.work), required(args.key)));
      if (value && sameRequest(value.request, args) && value.outcome === "pending") {
        saveEvent(scope(required(args.work), required(args.key)), "unknown", {
          error: String(error),
        });
        present(required(args.work));
      }
    } catch {
      /* Preserve the original dispatch error. */
    }
    throw error;
  }
  if (args.method === "workflow_show") {
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
    const snapshot = response as {
      launch: string;
      root?: string;
      history: History;
      children?: History[];
      subtitle?: string;
      endpoint?: string | undefined;
    };
    snapshot.endpoint = await serverAddress(snapshot.root);
    snapshot.subtitle = `${snapshot.endpoint ? `${snapshot.endpoint} · ` : ""}${workflowSubtitle(snapshot)}`;
    const placement = placements.get(snapshot.launch);
    if (placement && placement.view === manualView) {
      placement.endpoint = snapshot.endpoint;
      updateSubtitle(snapshot.launch, snapshot.subtitle, snapshot.history.workflowName);
    }
  }
  // oxlint-disable-next-line typescript/no-unsafe-return -- This branch forwards the JSON response unchanged to its caller.
  return JSON.parse(encode(response));
}
async function serverAddress(root: string | undefined): Promise<string | undefined> {
  if (!root) return undefined;
  let endpoint: Record<string, unknown>;
  try {
    endpoint = parsedObject(await swamp({ operation: "managed.endpoint", payload: {} }, root));
  } catch {
    return undefined;
  }
  const bind = endpoint["bind"],
    port = endpoint["port"];
  if (
    typeof bind !== "string" ||
    typeof port !== "number" ||
    !Number.isInteger(port) ||
    port <= 0 ||
    port > 65535
  )
    return undefined;
  return `${bind.includes(":") ? `[${bind}]` : bind}:${port}`;
}

// A separate RPC surface: Hocus cannot select arbitrary privileged bridge methods.
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Accept the SDK JSON and RPC wire types.
async function hocusWorkflow(input: Json, call: RpcCall): Promise<Json> {
  const from = call.source?.Plugin;
  assert(
    from != null && z.command("GetPaneInfo", plugin(from))?.plugin_url === "yinz:hocus",
    "Workflow tools require the host-authenticated builtin Hocus",
  );
  return validatedWorkflow(input, call);
}

// Shared by authenticated Hocus RPC and the host-authenticated CLI pipe.
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep the validated operation decision tree together.
async function validatedWorkflow(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- The shared validator accepts the SDK JSON wire type.
  input: Json,
  call: Pick<RpcCall, "callId" | "progress">,
): Promise<Json> {
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
  const request = input as { tool: string; args: Record<string, Json>; directory: string };
  assert(
    request != null && typeof request === "object" && typeof request.directory === "string",
    "Missing active project",
  );
  const a = request.args;
  assert(a != null && typeof a === "object" && !Array.isArray(a), "Expected tool arguments");
  const argument = (name: string): string => {
    assert(
      typeof a[name] === "string" && a[name].trim() !== "" && !a[name].includes("\0"),
      `Missing or invalid ${name}`,
    );
    return a[name];
  };
  const config = z.projectConfig.read(request.directory);
  assert(!config.error, config.error ?? "Invalid project configuration");
  const root = config.root;
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  const progress = (update: Json): void => {
    call.progress(update);
  };
  const query = async <T extends Json = Json>(
    operation: string,
    // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Query forwards nested SDK wire values without modifying them.
    payload: Readonly<Record<string, Json>> = {},
  ): Promise<T> =>
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
    JSON.parse(await swamp({ operation, payload, onProgress: progress }, root)) as T;
  const reports: Record<string, Json>[] = [
    { name: "@swamp/workflow-summary", workflow: a["workflow"] ?? null, run: a["run"] ?? null },
  ];
  // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
  if (Boolean(a["model"]))
    reports.push({
      name: "@swamp/method-summary",
      model: required(a["model"]),
      run: a["run"] ?? null,
    });
  let launch: string | undefined;
  try {
    switch (request.tool) {
      case "workflow_search":
        return await query("workflow.search", { query: a["query"] ?? "" });
      case "workflow_get":
        return await query("workflow.get", { workflowIdOrName: argument("workflow") });
      case "workflow_schema":
        return await query(
          "workflow.schema",
          // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
          Boolean(a["workflow"]) ? { workflowIdOrName: argument("workflow") } : {},
        );
      case "workflow_create": {
        const name = argument("name");
        await query("workflow.schema");
        const existing = await query<{ results?: { name: string }[] }>("workflow.search", {
          query: name,
        });
        assert(
          // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
          !(existing.results ?? []).some((w: { name: string }) => w.name === name),
          "A workflow already has that name; inspect it before editing",
        );
        const created = await query<{ id: string }>("workflow.create", { name });
        return {
          created,
          definition: await query("workflow.get", { workflowIdOrName: created.id }),
        };
      }
      case "workflow_edit":
      case "workflow_delete": {
        const workflowIdOrName = argument("workflow");
        assert(
          a["expected"] != null &&
            typeof a["expected"] === "object" &&
            !Array.isArray(a["expected"]),
          "expected must be the complete workflow_get result",
        );
        if (request.tool === "workflow_edit") {
          await query("workflow.schema");
          assert(
            a["definition"] != null &&
              typeof a["definition"] === "object" &&
              !Array.isArray(a["definition"]),
            "definition must match the workflow schema",
          );
          assert(
            a["definition"]["id"] === a["expected"]["id"],
            "Preserve the inspected workflow ID",
          );
        }
        // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
        const result = await query(
          request.tool === "workflow_edit" ? "workflow.edit" : "workflow.delete",
          {
            workflowIdOrName,
            expected: a["expected"],
            ...(request.tool === "workflow_edit" ? { content: encode(a["definition"]) } : {}),
          },
        );
        return request.tool === "workflow_delete"
          ? result
          : {
              result,
              definition: await query("workflow.get", {
                workflowIdOrName: required(a["expected"]["id"]),
              }),
            };
      }
      case "workflow_validate":
        return await query("workflow.validate", { workflowIdOrName: argument("workflow") });
      case "workflow_evaluate":
        return await query("workflow.evaluate", {
          workflowIdOrName: argument("workflow"),
          inputs: a["inputs"] ?? {},
        });
      case "workflow_active":
        return await query("run.history", { active: true });
      case "workflow_history":
        return await query("workflow.history.search", {
          query: a["query"] ?? "",
          // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
          ...(Boolean(a["workflow"]) ? { workflow: a["workflow"] } : {}),
          // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
          ...(Boolean(a["filter"]) ? { filter: a["filter"] } : {}),
        });
      case "workflow_approvals":
        return await query("workflow.approvals");
      case "workflow_reports":
        return await query("report.search", { ...a });
      case "workflow_report": {
        const { name: _name, ...options } = a;
        return await query("report.get", { ...options, reportName: argument("name") });
      }
      case "workflow_data": {
        argument("predicate");
        assert(
          a["limit"] == null ||
            (typeof a["limit"] === "number" &&
              Number.isInteger(a["limit"]) &&
              a["limit"] > 0 &&
              a["limit"] <= 1000),
          "Invalid data limit",
        );
        // oxlint-disable-next-line typescript/no-unsafe-return -- This branch forwards the JSON response unchanged to its caller.
        return JSON.parse(
          await swamp(
            {
              operation: "data.query",
              payload: { ...a },
              allowLimited: true,
              onProgress: progress,
            },
            root,
          ),
        );
      }
      case "workflow_models":
        return await query("model.search", { query: a["query"] ?? "" });
      case "workflow_model":
        return await query("model.get", { modelIdOrName: argument("model") });
      case "workflow_model_types":
        return await query("model.type.search", { query: a["query"] ?? "" });
      case "workflow_model_type":
        return await query("model.type.describe", { typeArg: argument("type") });
      case "workflow_model_method":
        return await query("model.method.describe", {
          modelIdOrName: argument("model"),
          methodName: argument("method"),
        });
      case "workflow_launch": {
        const workflow = argument("workflow");
        const inputs = a["inputs"] ?? {};
        assert(typeof inputs === "object" && !Array.isArray(inputs), "inputs must be an object");
        const supplied = inputs["launch"];
        launch =
          inputs["session"] === identity.session &&
          typeof supplied === "string" &&
          /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,199}$/u.test(supplied)
            ? supplied
            : key(`${z.instanceId}-hocus-${call.callId}`);
        // oxlint-disable-next-line eslint/no-shadow, typescript/no-unsafe-type-assertion -- The nested callback has its own independent operation state. This conversion restores the known host or stored wire type after JSON transport.
        const result = (await invoke(
          {
            method: "workflow_run",
            directory: request.directory,
            workflow,
            inputs,
            exactInputs: true,
            work: launch,
            key: "launch",
          },
          undefined,
          progress,
        )) as unknown as Delivery;
        if (result.run)
          await handle({
            method: "workflow_attach",
            work: launch,
            key: `attach-${result.run}`,
            run: result.run,
            workflow: required(result.workflow),
            directory: root,
          });
        // oxlint-disable-next-line typescript/no-unsafe-return -- This branch forwards the JSON response unchanged to its caller.
        return JSON.parse(encode({ ...result, reports }));
      }
      case "workflow_run_get":
      case "workflow_logs":
      case "workflow_resume":
      case "workflow_approve":
      case "workflow_reject":
      case "workflow_answer":
      case "workflow_cancel":
      case "workflow_cancel_step":
        break;
      default:
        throw new Error("Unsupported Hocus workflow tool");
    }
    const run = argument("run");
    assert(
      /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/iu.test(run),
      "An exact run UUID is required",
    );
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
    const history = (await query("workflow.history.get", { workflowIdOrName: run })) as History;
    assert(history.id === run, "Run identity mismatch");
    required(reports[0])["workflow"] = history.workflowName;
    const attachment = read<Attachment>(scope(run, "attachment"))[0];
    if (attachment) assert(attachment.root === root, "Run belongs to a different control checkout");
    let work = attachment?.work;
    const candidate = history.inputs?.launch;
    if (
      !work &&
      history.inputs?.session === identity.session &&
      typeof candidate === "string" &&
      /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,199}$/u.test(candidate)
    ) {
      const saved = read(scope(candidate, "launch"))[0];
      if (saved?.checkout === root && saved.workflow === history.workflowName) work = candidate;
    }
    if (request.tool === "workflow_run_get") {
      if (attachment) return await invoke({ method: "workflow_show", directory: root, run });
      // A run started outside Yinz has no bridge ownership. Its Swamp evidence is still readable.
      return {
        // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
        history: JSON.parse(encode(history)),
        children: [],
        reports,
        outputs: await query("data.query", {
          predicate: `version >= 0 && has(tags.workflowRunId) && tags.workflowRunId == ${encode(run)}`,
          select: outputProjection,
        }),
      };
    }
    if (request.tool === "workflow_logs") {
      assert(
        a["tail"] == null ||
          (typeof a["tail"] === "number" &&
            Number.isInteger(a["tail"]) &&
            a["tail"] > 0 &&
            a["tail"] <= 10000),
        "Invalid log tail",
      );
      const logs = await swamp(
        {
          operation: "workflow.history.logs",
          payload: { runIdOrWorkflow: run, tail: a["tail"] ?? 100 },
          onProgress: progress,
        },
        root,
      );
      return {
        run,
        // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
        logs: Boolean(a["contains"])
          ? logs
              .split("\n")
              .filter((line) => line.includes(argument("contains")))
              .join("\n")
          : logs,
      };
    }
    ensureOpen(work ?? run);
    if (request.tool === "workflow_resume") {
      assert(
        a["inputs"] == null || (typeof a["inputs"] === "object" && !Array.isArray(a["inputs"])),
        "inputs must be an object",
      );
      const ownerInputs =
        work &&
        // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
        (!Boolean(parsedObject(read(scope(work, "launch"))[0]?.request ?? "{}")["exactInputs"]) ||
          (history.inputs?.launch === work && history.inputs?.session === identity.session));
      for (const field of ["launch", "session"] as const)
        if (Boolean(ownerInputs) && Object.hasOwn(a["inputs"] ?? {}, field))
          assert(a["inputs"]?.[field] === history.inputs?.[field], `Cannot change owned ${field}`);
      const validation = await query<{ passed: Json }>("workflow.validate", {
        workflowIdOrName: history.workflowId,
      });
      if (validation.passed !== true)
        return {
          error: { code: "validation_failed", message: "Workflow validation failed" },
          validation,
          reports,
        };
      ensureOpen(work ?? run);
      return await invoke(
        {
          method: "swamp_dispatch",
          work: work ?? run,
          key: key(`hocus-${call.callId}`),
          directory: root,
          operation: "workflow.resume",
          payload: {
            workflowIdOrName: history.workflowId,
            runId: run,
            inputs: a["inputs"] ?? {},
            // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
            ...(Boolean(a["from"]) ? { from: argument("from") } : {}),
          },
        },
        undefined,
        progress,
      );
    }
    if (request.tool === "workflow_approve" || request.tool === "workflow_reject") {
      const payload = {
        workflowIdOrName: history.workflowId,
        runId: run,
        stepName: argument("step"),
        // oxlint-disable-next-line eslint/no-extra-boolean-cast -- Explicit conversion narrows a JSON value while preserving legacy truthiness.
        ...(Boolean(a["reason"]) ? { reason: argument("reason") } : {}),
      };
      if (request.tool === "workflow_reject") return await query("workflow.reject", payload);
      return await invoke(
        {
          method: "swamp_read",
          work: work ?? run,
          run,
          directory: root,
          operation: "workflow.approve",
          payload,
        },
        undefined,
        progress,
      );
    }
    if (request.tool === "workflow_answer") {
      const model = argument("model"),
        question = argument("question");
      const definition = await query<{ type: string }>("model.get", { modelIdOrName: model });
      assert(
        definition.type === "@hammz/question",
        "Only the workflow question model may receive answers",
      );
      const validation = await query<{ passed: Json }>("workflow.validate", {
        workflowIdOrName: history.workflowId,
      });
      if (validation.passed !== true)
        return {
          error: { code: "validation_failed", message: "Workflow validation failed" },
          validation,
          reports,
        };
      ensureOpen(work ?? run);
      // Only the question model which produced this exact run/question may receive submit.
      const questions = await query<{ results?: Json[] }>("data.query", {
        predicate: `modelName == ${encode(model)} && specName == "question" && attributes.run == ${encode(run)} && attributes.question == ${encode(question)}`,
        select: '{"attributes":attributes}',
      });
      assert(
        questions.results?.length === 1,
        "Question is absent or ambiguous; inspect its saved state",
      );
      return await invoke(
        {
          method: "workflow_answer",
          directory: root,
          model,
          inputs: {
            run,
            question,
            answer: argument("answer"),
            submission: key(`hocus-${call.callId}`),
          },
        },
        undefined,
        progress,
      );
    }
    if (request.tool === "workflow_cancel") {
      if (attachment) return await invoke({ method: "workflow_cancel", directory: root, run });
      assert(history.status === "running", "Only active executions can be cancelled");
      return await query("workflow.cancel", { runId: run });
    }
    assert(attachment, "Step cancellation requires a bridge-owned run");
    const operationKey = argument("key"),
      operationWork = argument("work");
    const ownedOperation = read(scope(operationWork, operationKey))[0];
    assert(
      lifecycleWork(operationWork) === attachment.work &&
        ownedOperation?.owner?.host_id === identity.host_id &&
        ownedOperation.owner.session === identity.session &&
        ownedOperation.owner.work === operationWork,
      "Step does not belong to the inspected run lifecycle",
    );
    return await invoke({ method: "cancel_step", work: operationWork, operation: operationKey });
  } catch (error) {
    let detail: Json = { code: "workflow_tool_failed", message: String(error) };
    try {
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- JSON error details are forwarded unchanged.
      detail = JSON.parse(error instanceof Error ? error.message : String(error));
    } catch {
      /* Keep the original diagnostic. */
    }
    return {
      error: detail,
      reports,
      ...(launch ? { launch } : {}),
      recovery:
        "Inspect persisted definitions, run history, reports and submission state before retrying a mutation.",
    };
  }
}

export default definePlugin({
  // oxlint-disable-next-line eslint/max-lines-per-function -- This lifecycle handler keeps its ordered protocol transitions together.
  load() {
    identity = {
      host_id: z.getPluginIds().host_id,
      session: z.serverSession,
      incarnation: z.serverIncarnation,
    };
    assert(identity.session !== "", "Bridge requires a server session");
    D.define("delivery", { value: { kind: "Text", required: true } }, { version: 1 });
    z.subscribe(["AgentEvent", "CommandPaneExited", "PaneUpdate", "PaneClosed"]);
    for (const entry of deliveryRows()) {
      if (entry.key === "view" && entry.value.closed && entry.value.cancel_pending)
        void cancelLifecycle(entry.work).catch((error: unknown) => {
          z.log(String(error));
        });
      else if (
        entry.value.cancelRequested &&
        ["pending", "unknown"].includes(entry.value.outcome) &&
        !closed(entry.work)
      )
        stopOperation(entry.uid);
    }
    z.rpc.handle("hocus_workflow", hocusWorkflow);
    // oxlint-disable-next-line eslint/max-lines-per-function -- Authentication and lifecycle ownership must precede privileged dispatch.
    z.rpc.handle("swamp", async (payload, call) => {
      const from = call.source?.Plugin;
      assert(from != null, "Workflow RPC requires a host-authenticated plugin source");
      const pane = z.command("GetPaneInfo", plugin(from));
      assert(
        pane?.plugin_url === "yinz:agent-exit" || pane?.plugin_url === "yinz:swamp-workflow",
        "Only builtin workflow views may use the privileged bridge RPC",
      );
      // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- This conversion restores the known host or stored wire type after JSON transport.
      const args = payload as Partial<Request>;
      if (args.method === "cleanup_prepare")
        assert(pane.plugin_url === "yinz:agent-exit", "Only an exit pane may prepare cleanup");
      if (
        ["workflow_run", "cleanup_prepare"].includes(args.method ?? "") &&
        pane.plugin_url === "yinz:agent-exit"
      ) {
        const uid = scope(required(args.work), "view"),
          [existing] = read(uid);
        if (existing)
          assert(
            existing.view === from && existing.incarnation === identity.incarnation,
            "Launch already belongs to another hook instance",
          );
        else {
          put(uid, {
            view: from,
            source: args.inputs?.["sourcePane"],
            incarnation: identity.incarnation,
          });
          placements.set(required(args.work), {
            view: from,
            queue: [],
            subtitle: "Workflow · loading",
          });
          present(required(args.work));
        }
      }
      if (args.method === "cleanup_prepare") {
        const work = required(args.work);
        let pending = preparing.get(work);
        if (!pending) {
          pending = prepareCleanup(work).finally(() => {
            preparing.delete(work);
          });
          preparing.set(work, pending);
        }
        return pending;
      }
      return invoke(args, args.method === "workflow_show" ? from : undefined, (progress) => {
        call.progress(progress);
      });
    });
  },
  // oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together.
  async pipe(messageIn) {
    if (messageIn.name === "workflow_action") {
      if (
        !messageIn.payload ||
        typeof messageIn.source !== "object" ||
        !("Cli" in messageIn.source)
      )
        return;
      const pipe = messageIn.source.Cli;
      z.command("BlockCliPipeInput", pipe);
      try {
        // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- JSON is checked by the shared operation validator.
        const input = JSON.parse(messageIn.payload) as Json;
        const data = await validatedWorkflow(input, {
          callId: pipe,
          progress: () => {
            // The CLI emits a single final reply.
          },
        });
        const error =
          data != null && typeof data === "object" && !Array.isArray(data)
            ? data["error"]
            : undefined;
        z.command("CliPipeOutput", [
          pipe,
          encode(error == null ? { ok: true, data } : { ok: false, error: encode(data) }),
        ]);
      } catch (error) {
        z.command("CliPipeOutput", [pipe, encode({ ok: false, error: String(error) })]);
      } finally {
        z.command("UnblockCliPipeInput", pipe);
      }
      return;
    }
    if (messageIn.name === "workflow-refresh-failed") {
      const from =
        typeof messageIn.source === "object" && "Plugin" in messageIn.source
          ? messageIn.source.Plugin
          : undefined;
      const launch = messageIn.payload ?? "";
      for (const [work, placement] of placements) {
        if (placement.view === from && (launch === "" || launch === work))
          unavailable(placement, work);
      }
      return;
    }
    if (messageIn.name === "swamp-pane-result" && messageIn.payload) {
      const from =
        typeof messageIn.source === "object" && "Plugin" in messageIn.source
          ? messageIn.source.Plugin
          : undefined;
      if (from == null) return;
      const pane = z.command("GetPaneInfo", plugin(from));
      if (!pane) return;
      assert(
        pane.plugin_url === "yinz:hocus-new-agent",
        "Pane result must come from the agent picker",
      );
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
      const reply: { work: string; key: string; choice: string; pane_id?: { Terminal: number } } =
        JSON.parse(messageIn.payload);
      assert(reply.choice === "started" || reply.choice === "cancelled", "Invalid picker result");
      if (reply.choice === "started")
        assert(
          reply.pane_id &&
            Object.keys(reply.pane_id).length === 1 &&
            Number.isInteger(reply.pane_id.Terminal) &&
            reply.pane_id.Terminal >= 0 &&
            reply.pane_id.Terminal <= 4294967295,
          "Started picker result requires a terminal pane ID",
        );
      const uid = scope(reply.work, reply.key),
        [current] = read(uid);
      assert(
        current && parsedObject(current.request)["method"] === "pane_action",
        "No picker operation",
      );
      if (current.outcome === "completed") {
        assert(current.choice === reply.choice, "Picker result already differs");
        message({ destination_plugin_id: from, message_name: "swamp-pane-ack" });
        return;
      }
      const view = read(scope(lifecycleWork(reply.work), "view"))[0];
      if (closed(reply.work) || (reply.choice === "started" && view?.transition)) {
        if (reply.choice === "started")
          z.command("CloseTerminalPane", required(reply.pane_id).Terminal);
        z.command("ClosePluginPane", from);
        return;
      }
      assert(
        ["pending", "unknown"].includes(current.outcome),
        "Picker dispatch cannot accept a result",
      );
      saveEvent(uid, "completed", { choice: reply.choice });
      if (reply.choice === "started") {
        transition(reply.work, "fork");
        if (parsedObject(current.request)["cancel_active"] === true)
          cancelAfterTransition(reply.work);
        closeView(
          reply.work,
          parsedObject(current.request)["action"] === "fork"
            ? required(reply.pane_id).Terminal
            : undefined,
        );
      }
      message({ destination_plugin_id: from, message_name: "swamp-pane-ack" });
      return;
    }
    if (
      messageIn.name !== "swamp" ||
      !messageIn.payload ||
      typeof messageIn.source !== "object" ||
      !("Cli" in messageIn.source)
    )
      return;
    const pipe = messageIn.source.Cli;
    z.command("BlockCliPipeInput", pipe);
    try {
      let reply;
      try {
        // oxlint-disable-next-line typescript/no-unsafe-argument -- The receiving handler validates the consumed request fields before dispatch.
        reply = { result: await invoke(JSON.parse(messageIn.payload)) };
      } catch (error) {
        reply = { error: String(error) };
      }
      z.command("CliPipeOutput", [pipe, encode(reply)]);
    } finally {
      z.command("UnblockCliPipeInput", pipe);
    }
  },
});
z.on("PaneUpdate", () => {
  for (const [work, placement] of placements) {
    if (placement.detached) continue;
    const source = read(scope(work, "source"))[0];
    const original =
      source?.pane == null ? undefined : z.command("GetPaneInfo", terminal(source.pane));
    // A rerun source means the agent restarted: the workflow no longer owns its location.
    if (original?.exited === false) detach(placement);
    else present(work);
  }
});
function deliveryRows(): {
  uid: string;
  row: DataRecord;
  work: string;
  key: string;
  value: Delivery;
}[] {
  const all: { uid: string; row: DataRecord; work: string; key: string; value: Delivery }[] = [];
  let cursor: Integer | null = null;
  do {
    const { records, raw } = D.fetch({ entity: "delivery", afterId: cursor });
    for (const row of records) {
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
      const id: string[] = JSON.parse(String(row["uid"]));
      if (id[0] === identity.host_id && id[1] === identity.session)
        all.push({
          uid: String(row["uid"]),
          row,
          work: required(id[2]),
          key: required(id[3]),
          // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
          value: JSON.parse(String(row["value"])),
        });
    }
    cursor = raw.next_cursor;
  } while (cursor != null);
  return all;
}
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together.
async function cancelLifecycle(work: string): Promise<void> {
  if (cancelling.has(work)) return;
  cancelling.add(work);
  let done = false;
  try {
    const uid = scope(work, "view"),
      [view, row] = read(uid);
    if (!view?.closed || !view.cancel_pending) return;
    const runs = new Map<string, string>();
    let unresolved = false;
    for (const entry of deliveryRows()) {
      if (lifecycleWork(entry.work) !== work) continue;
      const value = entry.value;
      if (entry.key === "runs") {
        for (const [id, run] of Object.entries(value.entries ?? {})) runs.set(id, run.root);
      } else if (value.checkout && value.request) {
        // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
        const request: Request = JSON.parse(value.request);
        if (
          request.method !== "workflow_run" &&
          !["workflow.run", "workflow.resume"].includes(request.operation)
        )
          continue;
        let run = value.run;
        if (!run && value.transport && !value.not_dispatched) {
          // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
          run = await workflowRunId(value, request);
          // oxlint-disable-next-line eslint/max-depth -- Nested protocol checks preserve the order of validation and state changes.
          if (run) {
            const [current, currentRow] = read(entry.uid);
            assert(current, "Missing dispatch");
            current.run ??= run;
            put(entry.uid, current, currentRow);
          } else unresolved = true;
        }
        if (run) runs.set(run, value.checkout);
      }
    }
    // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
    for (const [run, root] of runs) await cancelRun(run, root, view.cleanup === true);
    if (!unresolved) {
      view.cancel_pending = false;
      delete view.error;
      // Late attachments change this revision, requiring another cancellation pass.
      put(uid, view, row);
      done = true;
    }
  } catch (error) {
    const uid = scope(work, "view"),
      [view, row] = read(uid);
    if (view) {
      view.error = String(error);
      put(uid, view, row);
    }
    z.log("Workflow cancellation pending: " + String(error));
  } finally {
    cancelling.delete(work);
    if (!done && read(scope(work, "view"))[0]?.cancel_pending)
      void z
        .sleep(1000)
        .then(async () => cancelLifecycle(work))
        .catch((error: unknown) => {
          z.log(String(error));
        });
  }
}
// The closed marker blocks new dispatch; in-flight calls must publish ownership first.
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Cancellation and termination checks must all finish before readiness.
async function prepareCleanup(work: string): Promise<{ ready: boolean }> {
  const uid = scope(work, "view"),
    [view, row] = read(uid);
  assert(view, "Missing cleanup lifecycle");
  if (!view.cleanup) {
    view.cleanup = true;
    view.closed = true;
    view.cancel_pending = true;
    put(uid, view, row);
    const placement = placements.get(work);
    if (placement) placement.detached = true;
  }
  if ([...inFlight].some((flight) => flight.work != null && lifecycleWork(flight.work) === work)) {
    void cancelLifecycle(work).catch((error: unknown) => {
      z.log(String(error));
    });
    return { ready: false };
  }
  if (cancelling.has(work)) return { ready: false };
  await cancelLifecycle(work);
  const current = result(uid);
  assert(!current.error || !current.cancel_pending, current.error ?? "Cancellation failed");
  if (current.cancel_pending) return { ready: false };
  for (const entry of deliveryRows()) {
    if (lifecycleWork(entry.work) !== work || !entry.value.request) continue;
    const request = parsedObject(entry.value.request);
    const value = entry.value;
    if (request["operation"] === "model.method.run" && !value.not_dispatched) {
      assert(
        value.outcome !== "unknown",
        "Owned Swamp method termination is uncertain; reconcile before cleanup",
      );
      if (value.outcome === "pending") return { ready: false };
    }
    if (!["command", "agent_spawn", "agent_prompt"].includes(String(request["method"]))) continue;
    // These dispatch paths persist the owner before creating or prompting a process.
    if (!value.owner || value.terminationVerified) continue;
    assert(
      value.pane != null && value.supervisor,
      "Operation termination is uncertain; reconcile the saved operation and retry cleanup",
    );
    value.cancelRequested = true;
    value.outcome = "unknown";
    put(entry.uid, value, entry.row);
    stopOperation(entry.uid);
    // oxlint-disable-next-line eslint/no-await-in-loop -- Verify each owned process before declaring cleanup ready.
    await stopping.get(entry.uid);
    const stopped = result(entry.uid);
    assert(stopped.terminationVerified, stopped.error ?? "Operation termination is uncertain");
  }
  // Late run attachments request another cancellation pass.
  if (result(uid).cancel_pending) return { ready: false };
  const panes = ownedPanes(work);
  if (view.source != null) panes.delete(view.source);
  for (const pane of panes) z.command("CloseTerminalPane", pane);
  return { ready: true };
}
function ownedPanes(work: string): Set<number> {
  const panes = new Set<number>();
  for (const entry of deliveryRows()) {
    if (lifecycleWork(entry.work) !== work) continue;
    const value = entry.value;
    if (
      value.owner?.host_id === identity.host_id &&
      value.owner.session === identity.session &&
      value.owner.incarnation === identity.incarnation &&
      value.owner.work === entry.work &&
      value.pane != null
    )
      panes.add(value.pane);
  }
  return panes;
}
function closeLifecycle(work: string): void {
  const uid = scope(work, "view"),
    [view, row] = read(uid);
  if (!view || view.closed || view.transition || view.incarnation !== identity.incarnation) return;
  view.closed = true;
  view.cancel_pending = true;
  // Durable closure precedes resource disposal and asynchronous cancellation.
  put(uid, view, row);
  const placement = placements.get(work);
  if (placement) placement.detached = true;
  const panes = ownedPanes(work);
  if (view.source != null) panes.add(view.source);
  for (const entry of deliveryRows()) {
    if (lifecycleWork(entry.work) !== work) continue;
    const value = entry.value;
    if (["pending", "unknown"].includes(value.outcome)) {
      value.outcome = "interrupted";
      value.error = "Pane workflow lifecycle is closed";
      put(entry.uid, value, entry.row);
    }
  }
  for (const pane of panes) z.command("CloseTerminalPane", pane);
  z.command("ClosePluginPane", required(view.view));
  void cancelLifecycle(work).catch((error: unknown) => {
    z.log(String(error));
  });
}
z.on("PaneClosed", (pane) => {
  const affected = new Set<string>();
  for (const entry of deliveryRows()) {
    const value = entry.value;
    if (entry.key === "view" && value.incarnation === identity.incarnation) {
      if (
        ("Plugin" in pane && value.view === pane.Plugin) ||
        ("Terminal" in pane && (value.source === pane.Terminal || value.current === pane.Terminal))
      )
        affected.add(entry.work);
    } else if (
      "Terminal" in pane &&
      entry.key === "source" &&
      value.pane === pane.Terminal &&
      value.owner?.host_id === identity.host_id &&
      value.owner.session === identity.session &&
      value.owner.incarnation === identity.incarnation &&
      value.owner.work === entry.work
    )
      affected.add(lifecycleWork(entry.work));
  }
  for (const work of affected) closeLifecycle(work);
});
// oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep this existing event or protocol decision tree together for review. This lifecycle handler keeps its ordered protocol transitions together.
z.on("AgentEvent", (event) => {
  let raw: { _yinz_workflow_owner?: string; prompt_sha256?: string; turn_id?: string };
  try {
    // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
    raw = JSON.parse(event.raw || "");
  } catch {
    return;
  }
  // oxlint-disable-next-line eslint/no-underscore-dangle -- This field name is part of the existing agent hook wire protocol.
  if (!raw._yinz_workflow_owner) return;
  // oxlint-disable-next-line eslint/no-underscore-dangle, typescript/no-unsafe-assignment -- This field name is part of the existing agent hook wire protocol. The producer supplies this documented wire shape; the handler checks fields before dispatch.
  const owner: Owner = JSON.parse(raw._yinz_workflow_owner);
  if (
    owner.host_id !== identity.host_id ||
    owner.session !== identity.session ||
    closed(owner.work)
  )
    return;
  const uid = scope(owner.work, owner.agent),
    [agent] = read(uid);
  if (
    !agent ||
    (agent.pane != null && agent.pane !== event.pane_id) ||
    encode(agent.owner) !== encode(owner)
  )
    return;
  if (event.hook === "SessionStart") {
    const [current, row] = read(uid);
    assert(current, "No operation with that identity");
    current.ready = true;
    put(uid, current, row);
    if (current.outstanding && current.supervisor)
      void owned({ work: owner.work, agent: owner.agent }, true)
        .then(dispatchPrompt)
        .catch((error: unknown) => {
          z.log(String(error));
        });
  }
  const promptUid = outstandingPrompt(uid, agent);
  if (event.hook === "UserPromptSubmit" && promptUid) {
    const [value, row] = read(promptUid);
    if (value?.outcome === "pending") {
      // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
      const request: Request = JSON.parse(value.request);
      const matches =
        request.prompt_sha256 && raw.prompt_sha256
          ? request.prompt_sha256 === raw.prompt_sha256
          : (request.method === "agent_spawn" ? request.spec.prompt : request.prompt) ===
            event.prompt;
      if (matches && !value.accepted) {
        value.accepted = true;
        value.agentSession = event.session_id;
        value.turn = raw.turn_id;
        put(promptUid, value, row);
      }
    }
  }
  finishAgent(owner, event, raw);
});
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function finishAgent(owner: Owner, event: AgentEvent, raw: { turn_id?: string }): void {
  const uid = scope(owner.work, owner.agent),
    [agent, row] = read(uid);
  if (!agent || encode(agent.owner) !== encode(owner)) return;
  const promptUid = outstandingPrompt(uid, agent);
  if (!promptUid) return;
  const outcome = (
    { Stop: "completed", StopFailure: "failed", Interrupt: "interrupted" } as Record<string, string>
  )[event.hook];
  if (!outcome) return;
  const operation = result(promptUid);
  if (
    !operation.accepted ||
    operation.agentSession !== event.session_id ||
    (operation.turn && operation.turn !== raw.turn_id)
  )
    return;
  if (operation.cancelRequested || parsedObject(operation.request)["waitForExit"] === true) {
    const [current, currentRow] = read(promptUid);
    assert(current, "No operation with that identity");
    current.message = event.last_message ?? null;
    current.hook = event.hook;
    put(promptUid, current, currentRow);
    return;
  }
  saveEvent(promptUid, outcome, {
    message: event.last_message ?? null,
    hook: event.hook,
    agentSession: event.session_id ?? null,
  });
  delete agent.outstanding;
  put(uid, agent, row);
  present(owner.work);
}
// A spawn that waits for exit owns its startup prompt the way agent_prompt owns a later one.
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function outstandingPrompt(uid: string, agent: Delivery): string | undefined {
  if (agent.outstanding) return agent.outstanding;
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
  const request: Partial<Request> = agent.request ? JSON.parse(agent.request) : {};
  return request.method === "agent_spawn" && request.waitForExit && request.spec?.prompt
    ? uid
    : undefined;
}
function recordExit(uid: string, exitCode: number | null): void {
  const [value, row] = read(uid);
  if (!value || !["pending", "unknown"].includes(value.outcome)) return;
  value.exitCode = exitCode;
  value.processExited = true;
  // Wrapper exit says nothing about descendants which closed their pty handles.
  value.terminationVerified = false;
  if (value.cancelRequested || exitCode === 130) {
    value.cancelRequested = true;
    value.outcome = "unknown";
    put(uid, value, row);
    stopOperation(uid);
  } else {
    value.outcome =
      parsedObject(value.request)["method"] === "command"
        ? exitCode === 0
          ? "completed"
          : "failed"
        : "process-exit";
    put(uid, value, row);
  }
}
z.on("CommandPaneExited", ([pane, exitCode, context]) => {
  if (!context["swamp_owner"]) return;
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- The producer supplies this documented wire shape; the handler checks fields before dispatch.
  const owner: Owner = JSON.parse(context["swamp_owner"]);
  if (
    owner.host_id !== identity.host_id ||
    owner.session !== identity.session ||
    closed(owner.work)
  )
    return;
  // oxlint-disable-next-line typescript/prefer-nullish-coalescing -- An empty operation key needs the agent fallback too.
  const uid = scope(owner.work, context["swamp_operation"] || owner.agent),
    [value] = read(uid);
  if (
    !value ||
    encode(value.owner) !== encode(owner) ||
    (value.pane != null && value.pane !== pane)
  )
    return;
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- This builtin wrote the serialized record under its private data schema.
  const request: Request = JSON.parse(value.request);
  if (request.method === "command" && !["pending", "unknown"].includes(value.outcome)) return;
  recordExit(uid, exitCode);
  if (request.method === "agent_spawn") {
    const [current, row] = read(uid);
    assert(current, "No operation with that identity");
    current.processExited = true;
    put(uid, current, row);
  }
  if (value.outstanding) recordExit(value.outstanding, exitCode);
  present(owner.work);
  changed(owner.work);
});
