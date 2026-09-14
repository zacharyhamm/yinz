import type {PermissionType, Event, PipeMessage} from "zellij:plugin";
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
  start(options?: {rows?: number; columns?: number; hooks?: Record<string,string>; backgroundPlugins?: string[]}): void;
  loadPlugin(path: string, options?: {configuration?: Record<string,string>; headless?: boolean}): void;
  key(key: string): void;
  text(text: string): void;
  mouse(event: import("zellij:plugin").MouseEvent): void;
  resize(rows: number, columns: number): void;
  action(kdl: string): void;
  pipe(name: string, payload: string, options?: {plugin?: string; args?: Record<string,string>}): string;
  rpc(plugin: string, name: string, payload?: import("zellij:plugin").Json, timeoutMs?: number): Promise<import("zellij:plugin").Json>;
  reload(path: string): void;
  readonly screen: string;
  readonly events: unknown[];
  readonly panes: import("zellij:plugin").PaneInfo[];
  data(plugin: string, entity: string): import("zellij:plugin").DataRows;
  catalog(): import("zellij:plugin").DataEntityInfo[];
  readonly logs: string;
  waitFor(what: string, predicate: () => boolean | Promise<boolean>, timeoutMs?: number): Promise<void>;
  waitForText(text: string, timeoutMs?: number): Promise<string>;
};

export function renderText(raw: string, rows?: number, columns?: number): string;
