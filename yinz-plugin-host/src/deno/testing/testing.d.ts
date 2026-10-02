import type {PermissionType, Event, PipeMessage} from "yinz:plugin";
export interface TestOptions { grants?: PermissionType[] }
export function test(name: string, run: () => void | Promise<void>): void;
export function test(name: string, options: TestOptions, run: () => void | Promise<void>): void;
export function assert(value: unknown, message?: string): asserts value;
export function assertEquals(actual: unknown, expected: unknown, message?: string): void;
export function assertThrows(run: () => unknown, contains?: string): unknown;
export function assertRejects(run: () => Promise<unknown>, contains?: string): Promise<unknown>;
export interface PluginHarness {
  stub(command: string, reply: unknown): void;
  event(event: Event): Promise<void>;
  pipe(message: Partial<PipeMessage> & {name: string}): Promise<void>;
  advance(milliseconds: number): Promise<void>;
  render(rows?: number, columns?: number): Promise<string>;
  readonly commands: unknown[];
  readonly logs: string[];
  readonly renders: string[];
}
export function loadPlugin(path: string, options?: {configuration?: Record<string,string>; replies?: Record<string, unknown>}): Promise<PluginHarness>;
export const session: {
  fixture(files?: Record<string,string>, commands?: Record<string,string>): string;
  start(options?: {rows?: number; columns?: number; hooks?: Record<string,string>; backgroundPlugins?: (string | {path: string; configuration?: Record<string,string>})[]}): void;
  loadPlugin(path: string, options?: {configuration?: Record<string,string>; headless?: boolean}): void;
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
  reload(path: string): void;
  readonly screen: string;
  readonly events: unknown[];
  readonly panes: import("yinz:plugin").PaneInfo[];
  data(plugin: string, entity: string): import("yinz:plugin").DataRows;
  catalog(): import("yinz:plugin").DataEntityInfo[];
  readonly logs: string;
  waitFor(what: string, predicate: () => boolean | Promise<boolean>, timeoutMs?: number): Promise<void>;
  waitForText(text: string, timeoutMs?: number): Promise<string>;
};

export function renderText(raw: string, rows?: number, columns?: number): string;
