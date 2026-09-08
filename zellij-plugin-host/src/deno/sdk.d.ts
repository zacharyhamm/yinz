/** JSON values use the Rust PluginCommand/Event externally tagged representation. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type Permission =
  | "ReadApplicationState" | "ChangeApplicationState" | "OpenFiles" | "RunCommands"
  | "OpenTerminalsOrPlugins" | "WriteToStdin" | "WebAccess" | "ReadCliPipes"
  | "MessageAndLaunchOtherPlugins" | "Reconfigure" | "FullHdAccess" | "InterceptInput"
  | "ReadPaneContents" | "RunActionsAsUser" | "WriteToClipboard"
  | "ReadSessionEnvironmentVariables" | "PluginData" | "SharedPluginData"
  | "ReadFiles" | "WriteFiles" | "AuthorPlugins";
export type PermissionType = Permission;
export type PermissionStatus = "Granted" | "Denied";
export type PaneId = { Terminal: number } | { Plugin: number };
export type PipeSource = { Cli: string } | { Plugin: number } | "Keybind";
export interface EventPayloads {
  ModeUpdate: Json; TabUpdate: Json[]; PaneUpdate: Json;
  Key: { bare_key: Json; key_modifiers: string[] };
  Mouse: Json; Timer: number; CopyToClipboard: "Command" | "Primary" | "System";
  Visible: boolean; CustomMessage: [string, string];
  FileSystemCreate: Json[]; FileSystemRead: Json[]; FileSystemUpdate: Json[]; FileSystemDelete: Json[];
  PermissionRequestResult: PermissionStatus;
  SessionUpdate: [Json[], Json[]];
  RunCommandResult: [number | null, number[], number[], Record<string, string>];
  WebRequestResult: [number, Record<string, string>, number[], Record<string, string>];
  WebRequestStreamChunk: { stream_id: string; chunk: number[]; done: boolean; error: string | null };
  CommandPaneOpened: [number, Record<string, string>];
  CommandPaneExited: [number, number | null, Record<string, string>];
  PaneClosed: PaneId; EditPaneOpened: [number, Record<string, string>];
  EditPaneExited: [number, number | null, Record<string, string>];
  CommandPaneReRun: [number, Record<string, string>];
  FailedToWriteConfigToDisk: string | null; ListClients: Json[];
  HostFolderChanged: string; FailedToChangeHostFolder: string | null; PastedText: string;
  InterceptedKeyPress: EventPayloads["Key"]; UserAction: Json[]; PaneRenderReport: Json;
  ActionComplete: Json[]; CwdChanged: Json[]; CommandChanged: Json[]; AvailableLayoutInfo: Json[];
  PluginConfigurationChanged: Record<string, string>; HighlightClicked: Json;
  ContextMenuRequested: Json; ContextMenuItemSelected: Json; InitialKeybinds: Json;
  HostTerminalThemeChanged: Json; HintText: Json; ActivePaneScroll: [number, number] | null;
  PluginDataChanged: Json; QueueJobStarted: Json; QueueJobFinished: Json; QueueChanged: Json;
  Hook: { point: string; payload: Json }; AgentEvent: Json;
  ModifiersHeld: { modifiers: string[]; is_live: boolean };
}
export type UnitEvent = "SystemClipboardFailure" | "InputReceived" | "ConfigWasWrittenToDisk" | "BeforeClose";
export type EventType = keyof EventPayloads | UnitEvent;
export type Event = UnitEvent | { [K in keyof EventPayloads]: { [P in K]: EventPayloads[K] } }[keyof EventPayloads];
export type EventPayload<K extends EventType> = K extends keyof EventPayloads ? EventPayloads[K] : undefined;
export interface PluginIds {
  plugin_id: number; zellij_pid: number; initial_cwd: string; client_id: number;
  hostname: string; host_id: string;
}
export interface PipeMessage {
  source: PipeSource;
  name: string;
  payload: string | null;
  args: Record<string, string>;
  is_private: boolean;
  pane_id: PaneId | null;
}
export interface Plugin {
  load?(configuration: Record<string, string>): void | Promise<void>;
  update?(event: Event): boolean | void | Promise<boolean | void>;
  pipe?(message: PipeMessage): boolean | void | Promise<boolean | void>;
  render?(rows: number, columns: number): string | null | void | Promise<string | null | void>;
}
export interface Zellij {
  /** Commands retain their Rust spelling, e.g. command("RenameTab", [0, "logs"]). */
  command(name: string, payload?: Json): Json | Uint8Array;
  command(command: { [variant: string]: Json }): Json | Uint8Array;
  definePlugin<T extends Plugin>(plugin: T): T;
  subscribe(events: EventType[]): void;
  unsubscribe(events: EventType[]): void;
  /** Queues a permission request without waiting for its result. */
  requestPermission(permissions: Permission[]): void;
  requestRender(): void;
  on<K extends EventType>(type: K, handler: (payload: EventPayload<K>, event: Event) => boolean | void | Promise<boolean | void>): () => void;
  sleep(milliseconds: number): Promise<void>;
  getPluginIds(): PluginIds;
  log(...values: unknown[]): void;
  /** Waits for a matching future event, excluding the event currently being delivered. */
  waitForEvent<K extends EventType>(type: K, predicate?: (payload: EventPayload<K>, event: Event) => boolean): Promise<EventPayload<K>>;
  /** Queues permissions with earlier grants and rejects on denial or dispatch failure. */
  waitForPermissions(permissions: Permission[]): Promise<void>;
}
export const zellij: Zellij;
export const definePlugin: Zellij["definePlugin"];
export default zellij;
declare global { const zellij: Zellij; }
