import type {PermissionType, Event, PipeMessage, PaletteContext, PaletteAction, PaletteField} from "yinz:plugin";
export interface TestOptions { grants?: PermissionType[] }
export function test(name: string, run: () => void | Promise<void>): void;
export function test(name: string, options: TestOptions, run: () => void | Promise<void>): void;
export function assert(value: unknown, message?: string): asserts value;
export function assertEquals(actual: unknown, expected: unknown, message?: string): void;
export function assertThrows(run: () => unknown, contains?: string): unknown;
export function assertRejects(run: () => Promise<unknown>, contains?: string): Promise<unknown>;
/** Display-only wire protocol. Callbacks stay inside the SDK. */
export interface PaletteTestScope { registration: string; opening: number; view: number; revision: number }
export type PaletteTestMessage =
  | {type: "discover"; scope: PaletteTestScope; context: PaletteContext; query: string; source: string | null}
  | {type: "execute"; scope: PaletteTestScope; context: PaletteContext; handle: string; values: Record<string,string> | null}
  | {type: "cancel"; scope: PaletteTestScope}
  | {type: "release"; handles: string[]}
  | {type: "dismiss"; opening: number}
  | {type: "disconnect"};
export type PaletteTestAction = Required<Omit<PaletteAction, "run">> & {handle: string};
export type PaletteTestField =
  | Required<Extract<PaletteField, {type: "text" | "directory"}>>
  | (Omit<Extract<PaletteField, {type: "choice"}>, "initialValue"> & {initialValue: string | null});
export type PaletteTestView =
  | {type: "actions"; title: string; initialQuery?: string; actions: PaletteTestAction[]; source: string | null}
  | {type: "form"; title: string; fields: PaletteTestField[]; submitLabel: string; handle: string; actions: PaletteTestAction[]; confirmation: boolean}
  | {type: "text"; title: string; text: string};
export type PaletteTestReply =
  | {type: "fields"; values: Record<string, string>}
  | {type: "actions"; actions: PaletteTestAction[]}
  | {type: "view"; view: PaletteTestView}
  | {type: "complete"; notification: string; view: PaletteTestView | null}
  | {type: "close"};
export type PaletteTestCommand =
  | {type: "suspend"; scope: PaletteTestScope; suspended: boolean}
  | {type: "register"; id: string; title: string; registration: string; staticActions: boolean}
  | {type: "unregister"; registration: string}
  | {type: "reply"; scope: PaletteTestScope; result: {Ok: PaletteTestReply} | {Err: string}}
  | {type: "close"; scope: PaletteTestScope};
export interface PluginHarness {
  readonly pickRequests: { request_id: string; options: import("yinz:plugin").PickOptions }[];
  completePick(requestId: string, result: string | null): Promise<void>;
  failPick(requestId: string, message: string): Promise<void>;
  stub(command: string, reply: unknown): void;
  event(event: Event): Promise<void>;
  pipe(message: Partial<PipeMessage> & {name: string}): Promise<void>;
  /** Send a bridge message with an explicit u16 client ID matching context.clientId.
   * Stub CommandPalette: null explicitly. Callback errors appear in reply.result.Err.
   * Bridge or stub failures reject. Use event(), pipe(), or advance() to complete pending callbacks.
   * Send lifecycle messages explicitly. Static inventories do not use fuzzy ranking here.
   */
  palette(message: PaletteTestMessage, clientId: number): Promise<void>;
  /** Captured command history, not an active-provider registry. */
  readonly paletteCommands: readonly PaletteTestCommand[];
  /** Advance frozen test-worker Date.now() (initially zero) and SDK timers.
   * Call z.sleep() inside plugin callbacks, then advance from the test.
   * Sleep in the test body rejects because it has no plugin client context.
   */
  advance(milliseconds: number): Promise<void>;
  render(rows?: number, columns?: number): Promise<string>;
  readonly commands: unknown[];
  readonly logs: string[];
  readonly renders: string[];
}
/** Load one mocked TypeScript instance per test. Its instanceId is always "test".
 * Production instances use UUIDs. Host operations without default mock replies require explicit stubs.
 * @example
 * const plugin = await loadPlugin("./palette.ts", {
 *   replies: {RequestPluginPermissions: null, CommandPalette: null},
 * });
 * await plugin.event({PermissionRequestResult: "Granted"});
 * const registration = plugin.paletteCommands.find(c => c.type === "register" && c.id === "example");
 * assert(registration?.type === "register");
 * const context = {clientId: 1, paneId: {Terminal: 7}, tabId: 1, currentProject: null};
 * const scope = {registration: registration.registration, opening: 1, view: 0, revision: 1};
 * await plugin.palette({type: "discover", scope, context, query: "open", source: null}, 1);
 * const reply = plugin.paletteCommands.at(-1);
 * assert(reply?.type === "reply" && "Ok" in reply.result && reply.result.Ok.type === "actions");
 * const action = reply.result.Ok.actions.find(a => a.id === "open");
 * assert(action);
 * await plugin.palette({type: "execute", scope: {...scope, revision: 2}, context, handle: action.handle, values: null}, 1);
 */
export function loadPlugin(path: string, options?: {configuration?: Record<string,string>; replies?: Record<string, unknown>}): Promise<PluginHarness>;
/** Real disposable session controls, available only in integration tests.
 * Real plugins use normal clocks. Builtins retain production trust. Ordinary plugins
 * receive exactly the test's declared grants. Executable fixtures require RunCommands.
 * For real palette input, start a background fixture, wait for its registration marker,
 * then key("Ctrl Alt p"), text("unique fixture action"), waitForText(), key("Enter").
 */
export const session: {
  fixture(files?: Record<string,string>, commands?: Record<string,string>): string;
  start(options?: {autoGrantPermissions?: boolean; rows?: number; columns?: number; hooks?: Record<string,string>; backgroundPlugins?: (string | {path: string; configuration?: Record<string,string>})[]}): void;
  /** Dispatch a shipped yinz: Wasm/Deno builtin or a confined local script.
   * Wait for readiness with existing screen or log waits. Missing artifacts follow normal host loading errors.
   * Preserve literal filenames with escaped specifiers, also for reload/pipe/data:
   * const target = import.meta.resolve("./reader%20%23%3F%2523.ts"); // reader #?%23.ts
   * session.loadPlugin(target); session.reload(target);
   */
  loadPlugin(path: string, options?: {configuration?: Record<string,string>; headless?: boolean; autoGrantPermissions?: boolean}): PluginLifecycle | null;
  key(key: string): void;
  text(text: string): void;
  mouse(event: import("yinz:plugin").MouseEvent): void;
  resize(rows: number, columns: number): void;
  action(kdl: string): void;
  /** Real agent control in the disposable session. Requires RunCommands. */
  agent(request: import("yinz:plugin").AgentRequest): Promise<Omit<import("yinz:plugin").AgentResponse, "spawned"> & {spawned: import("yinz:plugin").SpawnedAgent | null}>;
  /** Deliver a normalized hook through the real report/acknowledgement route. Requires RunCommands. */
  report(pane: import("yinz:plugin").PaneId, kind: string, hook: {hook_event_name: string; session_id: string; [key: string]: import("yinz:plugin").Json}): void;
  pipe(name: string, payload: string, options?: {plugin?: string; args?: Record<string,string>}): string;
  rpc(plugin: string, name: string, payload?: import("yinz:plugin").Json, timeoutMs?: number): Promise<import("yinz:plugin").Json>;
  reload(path: string, options?: {configuration?: Record<string,string>}): PluginLifecycle;
  stop(path: string, options?: {configuration?: Record<string,string>}): PluginLifecycle;
  status(path: string, options?: {configuration?: Record<string,string>}): PluginLifecycle;
  readonly screen: string;
  readonly events: unknown[];
  readonly panes: import("yinz:plugin").PaneInfo[];
  data(plugin: string, entity: string): import("yinz:plugin").DataRows;
  catalog(): import("yinz:plugin").DataEntityInfo[];
  readonly logs: string;
  /** Timeout uses host monotonic elapsed time, independently of frozen guest Date.now(). */
  waitFor(what: string, predicate: () => boolean | Promise<boolean>, timeoutMs?: number): Promise<void>;
  waitForText(text: string, timeoutMs?: number): Promise<string>;
};

export function renderText(raw: string, rows?: number, columns?: number): string;

export interface PluginLifecycle {
  plugins: {id: number; location: string; configuration: Record<string,string>; background: boolean; shared: boolean; clients: number[]; state: "loading" | "awaiting_permission" | "running" | "failed" | "stopped"; pending_permissions: string[]; latest_error: string | null}[];
}
