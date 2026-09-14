// ---- section: sdk_core
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
  ModeUpdate: ModeInfo; TabUpdate: TabInfo[]; PaneUpdate: PaneManifest; Key: KeyWithModifier;
  Mouse: Mouse; Timer: number; CopyToClipboard: 'Command' | 'Primary' | 'System'; Visible: boolean; CustomMessage: [string, string];
  FileSystemCreate: [string, FileMetadata | null][]; FileSystemRead: [string, FileMetadata | null][];
  FileSystemUpdate: [string, FileMetadata | null][]; FileSystemDelete: [string, FileMetadata | null][];
  PermissionRequestResult: PermissionStatus; SessionUpdate: [SessionInfo[], [string, Duration][]];
  RunCommandResult: [number | null, number[], number[], StringMap]; WebRequestResult: [number, StringMap, number[], StringMap];
  RunCommandOutput: {data: Uint8Array; stderr: boolean; context: StringMap};
  PaneOutput: {pane_id: number; data: Uint8Array; context: StringMap};
  WebRequestStreamChunk: {stream_id: string; chunk: number[]; done: boolean; error: string | null};
  CommandPaneOpened: [number, StringMap]; CommandPaneExited: [number, number | null, StringMap]; PaneClosed: PaneId;
  EditPaneOpened: [number, StringMap]; EditPaneExited: [number, number | null, StringMap]; CommandPaneReRun: [number, StringMap];
  FailedToWriteConfigToDisk: string | null; ListClients: ClientInfo[]; HostFolderChanged: string; FailedToChangeHostFolder: string | null;
  PastedText: string; InterceptedKeyPress: KeyWithModifier; UserAction: [Action, number, number | null, number | null];
  PaneRenderReport: Record<`terminal_${number}` | `plugin_${number}`, PaneContents>; ActionComplete: [Action, PaneId | null, StringMap];
  CwdChanged: [PaneId, string, number[]]; CommandChanged: [PaneId, string[], boolean, number[]]; AvailableLayoutInfo: [LayoutInfo[], LayoutWithError[]];
  PluginConfigurationChanged: StringMap; HighlightClicked: {pane_id: PaneId; pattern: string; matched_string: string; context: StringMap};
  ContextMenuRequested: ContextMenuContext; ContextMenuItemSelected: {label: string; item_context: StringMap; context: ContextMenuContext};
  InitialKeybinds: Keybinds; HostTerminalThemeChanged: 'Dark' | 'Light'; HintText: Record<string, {text: string; indices: Integer[][]}>;
  ActivePaneScroll: [Integer, Integer] | null; PluginDataChanged: {watch_id: Integer; rows: DataRows; truncated: boolean};
  QueueJobStarted: {queue: string; job_id: Integer; plugin_id: number; context: StringMap};
  QueueJobFinished: {queue: string; job_id: Integer; outcome: QueueJobOutcome; payload: string | null; context: StringMap};
  QueueChanged: QueueSnapshot; Hook: {point: HookPoint; payload: HookPayload}; AgentEvent: AgentEvent;
  ModifiersHeld: {modifiers: KeyModifier[]; is_live: boolean};
}
export type UnitEvent = "SystemClipboardFailure" | "InputReceived" | "ConfigWasWrittenToDisk" | "BeforeClose";
export type EventType = keyof EventPayloads | UnitEvent;
export type Event = UnitEvent | { [K in keyof EventPayloads]: { [P in K]: EventPayloads[K] } }[keyof EventPayloads];
export type EventPayload<K extends EventType> = K extends keyof EventPayloads ? EventPayloads[K] : undefined;
/** Named, byte-decoded forms of the events whose raw shape is positional or bytes. Every other
 * event keeps its payload in `value` alone; struct payloads are not spread. */
export interface DecodedFields {
  RunCommandResult: {exitCode: number | null; stdout: Uint8Array; stderr: Uint8Array; context: StringMap};
  WebRequestResult: {status: number; headers: StringMap; body: Uint8Array; context: StringMap};
  RunCommandOutput: {data: Uint8Array; stderr: boolean; context: StringMap};
  PaneOutput: {paneId: number; data: Uint8Array; context: StringMap};
  WebRequestStreamChunk: {streamId: string; chunk: Uint8Array; done: boolean; error: string | null};
  SessionUpdate: {sessions: SessionInfo[]; resurrectableSessions: [string, Duration][]};
  CustomMessage: {message: string; payload: string};
  CommandPaneOpened: {paneId: number; context: StringMap}; CommandPaneExited: {paneId: number; exitCode: number | null; context: StringMap};
  CommandPaneReRun: {paneId: number; context: StringMap};
  EditPaneOpened: {paneId: number; context: StringMap}; EditPaneExited: {paneId: number; exitCode: number | null; context: StringMap};
  UserAction: {action: Action; clientId: number; terminalId: number | null; cliClientId: number | null};
  ActionComplete: {action: Action; paneId: PaneId | null; context: StringMap};
  CwdChanged: {paneId: PaneId; cwd: string; focusedClientIds: number[]};
  CommandChanged: {paneId: PaneId; command: string[]; isForeground: boolean; focusedClientIds: number[]};
  AvailableLayoutInfo: {layouts: LayoutInfo[]; errors: LayoutWithError[]};
  Key: {key: string}; InterceptedKeyPress: {key: string};
  Mouse: MouseSummary;
  FileSystemCreate: {files: FsEntry[]}; FileSystemRead: {files: FsEntry[]}; FileSystemUpdate: {files: FsEntry[]}; FileSystemDelete: {files: FsEntry[]};
  PluginDataChanged: {records: DataRecord[]};
}
/** `type`, the untouched payload as `value`, the whole event as `raw`, and the named fields above. */
export type DecodedEventOf<K extends EventType> = {type: K; value: EventPayload<K>; raw: Event} & (K extends keyof DecodedFields ? DecodedFields[K] : {});
export type DecodedEvent = {[K in EventType]: DecodedEventOf<K>}[EventType];
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
  /** String defaults are shallow-merged beneath the supplied load configuration. */
  defaults?: Record<string, string>;
  load?(configuration: Record<string, string>): void | Promise<void>;
  update?(event: Event): boolean | void | Promise<boolean | void>;
  pipe?(message: PipeMessage): boolean | void | Promise<boolean | void>;
  render?(rows: number, columns: number): string | null | void | Promise<string | null | void>;
}
/** Cancellation rejects an unsettled promise with AbortError. Chained promises are ordinary promises. */
export interface CancellablePromise<T> extends Promise<T> { cancel(): void; }
export interface TimerHandle { cancel(): void; }
export type TimerCallback = () => boolean | void | Promise<boolean | void>;
export interface RunCommandOptions {
  command: string[];
  envVariables?: Record<string, string>;
  cwd?: string;
  context?: Record<string, string>;
  maxBytesReturned?: number;
  /** Synchronous, ordered chunks. Return values are ignored. Throws reject the command. */
  onOutput?: (output: {data: Uint8Array; stderr: boolean; context: StringMap}) => void;
}
export interface RunCommandResult {
  exitCode: number | null;
  stdout: Uint8Array;
  stderr: Uint8Array;
  context: Record<string, string>;
}
export interface WebRequestOptions {
  url: string;
  verb?: "Get" | "Post" | "Put" | "Delete";
  headers?: Record<string, string>;
  body?: string | Uint8Array;
  context?: Record<string, string>;
  proxy?: string;
  /** Receive chunks through WebRequestStreamChunk, filtered by stream_id. Register before dispatch. */
  streamId?: string;
}
export interface WebRequestResult {
  status: number;
  headers: Record<string, string>;
  body: Uint8Array;
  context: Record<string, string>;
}
export interface Zellij {
  readonly serverBinary: string;
  readonly serverSession: string;
  readonly serverIncarnation: string;
  readonly swampTiming: boolean;
  monotonicMs(): number;
  readonly instanceId: string;
  /** Internal: available only to the trusted zellij:swamp-bridge builtin. */
  readonly workflowSignal?: (request: {terminal_id: number; owner: string; process?: number; supervisor: string; kill: boolean}) => void;
  readonly workflowRemotePanes?: () => {terminal_id: number; host_id: string; remote_host: string; pane_token: string; owner: string | null; child_group_terminated?: boolean}[];
  readonly agent: Agents;
  readonly hocus: HocusExtensions;
  readonly rpc: Rpc;
  readonly ui: UserInterface;
  readonly text: TerminalText;
  readonly fs: Filesystem;
  readonly projectConfig: ProjectConfiguration;
  readonly data: DataStore;
  /** Commands retain their Rust spelling, e.g. command("RenameTab", [0, "logs"]). */
  command<K extends CommandName>(name: K, ...args: CommandArgs[K] extends undefined ? [] : [payload: CommandArgs[K]]): CommandResults[K];
  command<C extends CommandObject>(command: C & Record<Exclude<keyof C, CommandName>, never>): CommandResults[keyof C & CommandName];
  definePlugin<T extends Plugin>(plugin: T): T;
  subscribe(events: EventType[]): void;
  unsubscribe(events: EventType[]): void;
  /** Queues a permission request without waiting for its result. */
  requestPermission(permissions: Permission[]): void;
  requestRender(): void;
  on<K extends EventType>(type: K, handler: (payload: EventPayload<K>, event: Event) => boolean | void | Promise<boolean | void>): () => void;
  /** on(), handing the handler the decoded event. Same ordering and return contract; the raw payload stays as `event.value`. */
  onEvent<K extends EventType>(type: K, handler: (event: DecodedEventOf<K>) => boolean | void | Promise<boolean | void>): () => void;
  /** The event as a discriminated union: `type`, its untouched payload as `value`, and named camelCase
   * fields (bytes as Uint8Array, `key` as keyName spells it, Mouse flattened) for the events whose raw
   * shape is positional or bytes. Throws TypeError for a value that is not an Event. */
  event(raw: Event): DecodedEvent;
  sleep(milliseconds: number): CancellablePromise<void>;
  after(milliseconds: number, callback: TimerCallback): TimerHandle;
  /** Waits one interval after the previous callback settles. Callbacks never overlap. */
  every(milliseconds: number, callback: TimerCallback): TimerHandle;
  paneSequence(options: PaneSequenceOptions): PaneSequence;
  /** Observes any terminal pane. Requires ReadApplicationState. Already-exited or missing panes resolve immediately. */
  waitForPaneExit(pane: AgentPane): CancellablePromise<PaneExit>;
  runInPane(pane: AgentPane, step: PaneStep): CancellablePromise<PaneExit>;
  /** Requires RunCommands. Defaults: empty maps, /host, host output limit. Nonzero exits resolve. */
  runCommand(options: RunCommandOptions): CancellablePromise<RunCommandResult>;
  swampRequest(options: {directory: string; operation: string; payload?: Json; id?: string; started?: boolean; onProgress?: (payload: Json) => void}): CancellablePromise<Json>;
  /** Requires WebAccess. Default Get. With streamId, resolves the leading response (empty body for 2xx).
   * HTTP error bodies stay buffered. cancel() stops the wait; command("CancelWebRequest", streamId) stops streaming. */
  webRequest(options: WebRequestOptions): CancellablePromise<WebRequestResult>;
  encodeUtf8(text: string): Uint8Array;
  decodeUtf8(bytes: Uint8Array): string;
  /** Raw host commands by camelCased name: zellij.host.renameTab(0, "logs") or renameTab({tabIndex: 0, name: "logs"}). */
  readonly host: HostCommands;
  /** A Uint8Array from raw event bytes (number[]), text, or an existing Uint8Array (returned as is). */
  bytes(value: Bytes | string): Uint8Array;
  /** "t", "Esc", "F2": the key as the user names it. */
  keyName(key: BareKey): string;
  /** The PaneRenderReport spelling of a pane id: {Terminal: 3} -> "terminal_3". */
  paneKey(pane: PaneId | number): PaneKey;
  mouse(event: Mouse | {Mouse: Mouse}): MouseSummary;
  /** The active tab of a TabUpdate payload, or null. */
  getFocusedTab(tabs: TabInfo[]): TabInfo | null;
  /** The focused terminal pane of one tab of a PaneUpdate payload, plugins ignored, or null. */
  getFocusedPane(tabPosition: number, manifest: PaneManifest): PaneInfo | null;
  /** Requires RunCommands. Runs a command in an existing terminal pane and does not wait for it. */
  runCommandInPane(pane: AgentPane, command: CommandToRun, options?: {context?: StringMap; keepScrollback?: boolean}): void;
  terminalPane(id: number): {Terminal: number};
  pluginPane(id: number): {Plugin: number};
  /** Requires WriteToStdin. Text is UTF-8 encoded. */
  write(text: string | Uint8Array): void;
  writeToPaneId(text: string | Uint8Array, pane: PaneId): void;
  /** Requires AuthorPlugins. The last lines (default 50) zellij logged about the named plugin. */
  readPluginLog(name: string, lines?: number): string;
  /** Requires OpenTerminalsOrPlugins. A new terminal pane, optionally in cwd. */
  openTerminal(cwd?: string | {cwd?: string | null; path?: string | null} | null): PaneId | null;
  closeSelf(): void;
  setSelectable(selectable: boolean): void;
  /** A keybinding action: positional arguments, one object of properties, or {args, props, children}. */
  action(name: string, ...args: KdlScalar[]): KeybindAction;
  action(name: string, spec: {args?: KdlScalar[]; props?: Record<string, KdlScalar>; children?: Record<string, KdlScalar>} | Record<string, KdlScalar>): KeybindAction;
  /** Requires Reconfigure. Replaces this plugin's previous defaults. Handlers run from the reserved keybinding pipe and re-render when they return true. */
  registerDefaultKeybindings(bindings: DefaultKeybinding[]): void;
  getPluginIds(): PluginIds;
  /** Enable names-only tracing (default true), or disable it with false. */
  trace(enabled?: boolean): void;
  log(...values: unknown[]): void;
  /** Waits for a matching future event, excluding the event currently being delivered. */
  waitForEvent<K extends EventType>(type: K, predicate?: (payload: EventPayload<K>, event: Event) => boolean): CancellablePromise<EventPayload<K>>;
  /** Observes the next matching message without consuming it or the current pipe callback's message. */
  waitForPipe(name: string, predicate?: (message: PipeMessage) => boolean): CancellablePromise<PipeMessage>;
  /** Queues permissions with earlier grants and rejects on denial or dispatch failure. */
  waitForPermissions(permissions: Permission[]): Promise<void>;
}
export const zellij: Zellij;
export const definePlugin: Zellij["definePlugin"];
export default zellij;
/** The plugin's log, under the familiar names: every method joins its arguments with tabs, from module top level onward. */
export interface PluginConsole {
  log(...values: unknown[]): void; info(...values: unknown[]): void; warn(...values: unknown[]): void;
  error(...values: unknown[]): void; debug(...values: unknown[]): void; trace(...values: unknown[]): void;
}
declare global { const zellij: Zellij; const console: PluginConsole; }

// ---- section: sdk_commands
/** Integers outside the JS safe range use BigInt. Rust validates destination ranges. */
export type Integer = number | bigint;
export type Bytes = number[] | Uint8Array;
export type StringMap = Record<string, string>;
type Tagged<M> = { [K in keyof M]: { [P in K]: M[K] } }[keyof M];
/** Raw command payloads retain Rust field names and tuple positions. */
export interface CommandArgs {
  CheckPlugin: {name: string; source: string; mode: PluginCheckMode};
  SwampRequest: {directory: string; id: string; operation: string; payload: string};
  Subscribe: EventType[]; Unsubscribe: EventType[]; SetSelectable: boolean;
  ShowCursor: [Integer, Integer] | null; GetPluginIds: undefined; GetZellijVersion: undefined;
  OpenFile: [FileToOpen, StringMap]; OpenFileFloating: [FileToOpen, FloatingPaneCoordinates | null, StringMap];
  OpenTerminal: FileToOpen; OpenTerminalFloating: [FileToOpen, FloatingPaneCoordinates | null];
  OpenCommandPane: [CommandToRun, StringMap]; OpenCommandPaneFloating: [CommandToRun, FloatingPaneCoordinates | null, StringMap];
  OpenCommandPaneOverlay: [CommandToRun, OverlayPlacement, StringMap];
  SwitchTabTo: number; SetTimeout: number; ExecCmd: string[];
  /** Refused by the SDK: Deno plugins have no workers. Use zellij.after/every, the async helpers, or a work queue. */
  PostMessageTo: PluginMessage & {__deno_has_no_workers: never}; PostMessageToPlugin: PluginMessage;
  HideSelf: undefined; ShowSelf: boolean; SwitchToMode: InputMode; NewTabsWithLayout: string;
  NewTab: {name: string | null; cwd: string | null}; NewTabUnfocused: CommandArgs['NewTab'];
  NewTiledPaneInTab: {tab_id: number}; ToggleFloatingPanes: {tab_id: number | null};
  NewPane: undefined; GoToNextTab: undefined; GoToPreviousTab: undefined;
  Resize: Resize; ResizeWithDirection: ResizeStrategy;
  FocusNextPane: undefined; FocusPreviousPane: undefined; FocusLastPane: undefined;
  MoveFocus: Direction; MoveFocusOrTab: Direction; Detach: undefined; EditScrollback: undefined;
  Write: Bytes; WriteChars: string; ToggleTab: undefined; MovePane: undefined; MovePaneWithDirection: Direction;
  ClearScreen: undefined; ScrollUp: undefined; ScrollDown: undefined; ScrollToTop: undefined; ScrollToBottom: undefined;
  PageScrollUp: undefined; PageScrollDown: undefined; ToggleFocusFullscreen: undefined; ToggleFocusNoUiFullscreen: undefined;
  TogglePaneFrames: undefined; SetPaneFrameStyle: PaneFrameStyle; TogglePaneEmbedOrEject: undefined;
  UndoRenamePane: undefined; CloseFocus: undefined; ToggleActiveTabSync: undefined; CloseFocusedTab: undefined;
  UndoRenameTab: undefined; QuitZellij: undefined; PreviousSwapLayout: undefined; NextSwapLayout: undefined;
  GoToTabName: string; FocusOrCreateTab: string; GoToTab: number; StartOrReloadPlugin: string;
  CloseTerminalPane: number; ClosePluginPane: number;
  FocusTerminalPane: [number, boolean, boolean]; FocusPluginPane: [number, boolean, boolean];
  RenameTerminalPane: [number, string]; RenamePluginPane: [number, string]; RenameTab: [number, string];
  ReportPanic: string; RequestPluginPermissions: Permission[]; SwitchSession: ConnectToSession;
  DeleteDeadSession: string; DeleteAllDeadSessions: undefined; OpenTerminalInPlace: FileToOpen;
  OpenFileInPlace: [FileToOpen, StringMap]; OpenCommandPaneInPlace: [CommandToRun, StringMap];
  SubscribePaneOutput: StringMap | null;
  RunCommand: [string[], StringMap, string, StringMap, Integer | null] | [string[], StringMap, string, StringMap, Integer | null, boolean | null];
  WebRequest: [string, HttpVerb, StringMap, Bytes, StringMap, string | null, string | null];
  CancelWebRequest: string; RenameSession: string; Notify: [string, PaneId | null, NotifyClick | null];
  UnblockCliPipeInput: string; BlockCliPipeInput: string; CliPipeOutput: [string, string];
  MessageToPlugin: MessageToPlugin; DisconnectOtherClients: undefined; KillSessions: string[];
  ScanHostFolder: string; WatchFilesystem: undefined;
  FsRead: {path: string; max_bytes: Integer | null}; FsList: {path: string}; FsMetadata: {path: string};
  FsWrite: {path: string; contents: string | Bytes; append: boolean; create_dirs: boolean};
  FsRemove: {path: string; recursive: boolean}; FsCreateDir: {path: string; all: boolean}; FsRename: {from: string; to: string};
  GrantPluginPermissions: {plugin_url: string; permissions: Permission[]};
  DumpSessionLayout: {tab_index: number | null}; CloseSelf: undefined; NewTabsWithLayoutInfo: LayoutInfo;
  Reconfigure: [string, boolean]; HidePaneWithId: PaneId; ShowPaneWithId: [PaneId, boolean, boolean];
  OpenCommandPaneBackground: [CommandToRun, StringMap]; RerunCommandPane: number;
  RunCommandInPane: {pane_id: PaneId; command: CommandToRun; context: StringMap; keep_scrollback?: boolean};
  ResizePaneIdWithDirection: [ResizeStrategy, PaneId]; EditScrollbackForPaneWithId: PaneId;
  GetPaneScrollback: {pane_id: PaneId; get_full_scrollback: boolean};
  WriteToPaneId: [Bytes, PaneId]; WriteCharsToPaneId: [string, PaneId]; SendSigintToPaneId: PaneId; SendSigkillToPaneId: PaneId;
  GetPanePid: {pane_id: PaneId}; GetPaneRunningCommand: {pane_id: PaneId}; GetPaneCwd: {pane_id: PaneId};
  GetPaneGitProbe: {pane_id: PaneId; cwd: string; refresh_token: string | null};
  MovePaneWithPaneId: PaneId; MovePaneWithPaneIdInDirection: [PaneId, Direction]; ClearScreenForPaneId: PaneId;
  ScrollUpInPaneId: PaneId; ScrollDownInPaneId: PaneId; ScrollToTopInPaneId: PaneId; ScrollToBottomInPaneId: PaneId;
  PageScrollUpInPaneId: PaneId; PageScrollDownInPaneId: PaneId; TogglePaneIdFullscreen: PaneId; TogglePaneEmbedOrEjectForPaneId: PaneId;
  CloseTabWithIndex: number; BreakPanesToNewTab: [PaneId[], string | null, boolean]; BreakPanesToTabWithIndex: [PaneId[], number, boolean];
  SwitchTabToId: number; GoToTabWithId: number; CloseTabWithId: number; RenameTabWithId: [number, string];
  ArrangeProjectPanes: ProjectPaneGroup[]; SetTabProjects: Record<string, string>; BreakPanesToTabWithId: [PaneId[], number, boolean];
  ReloadPlugin: number; LoadNewPlugin: {url: string; config: StringMap; load_in_background: boolean; skip_plugin_cache: boolean};
  RebindKeys: {keys_to_rebind: [InputMode, KeyWithModifier, Action[]][]; keys_to_unbind: [InputMode, KeyWithModifier][]; write_config_to_disk: boolean};
  RegisterDefaultKeybindings: string; ListClients: undefined; ChangeHostFolder: string;
  SetFloatingPanePinned: [PaneId, boolean]; StackPanes: PaneId[]; ChangeFloatingPanesCoordinates: [PaneId, FloatingPaneCoordinates][];
  ChangeOverlayCoordinates: {placements: [PaneId, FloatingPaneCoordinates][]; override_user_placement: boolean};
  TogglePaneBorderless: PaneId; SetPaneBorderless: [PaneId, boolean];
  OpenCommandPaneNearPlugin: [CommandToRun, StringMap]; OpenCommandPaneInGitCheckoutNearPlugin: [CommandToRun, string, GitCheckout, boolean?];
  OpenTerminalNearPlugin: FileToOpen; OpenTerminalFloatingNearPlugin: [FileToOpen, FloatingPaneCoordinates | null];
  OpenTerminalInPlaceOfPlugin: [FileToOpen, boolean]; OpenCommandPaneFloatingNearPlugin: [CommandToRun, FloatingPaneCoordinates | null, StringMap];
  OpenCommandPaneInPlaceOfPlugin: [CommandToRun, boolean, StringMap]; OpenFileNearPlugin: [FileToOpen, StringMap];
  OpenFileFloatingNearPlugin: [FileToOpen, FloatingPaneCoordinates | null, StringMap]; OpenFileInPlaceOfPlugin: [FileToOpen, boolean, StringMap];
  GroupAndUngroupPanes: [PaneId[], PaneId[], boolean]; HighlightAndUnhighlightPanes: [PaneId[], PaneId[]]; SetRelatedPanes: PaneId[];
  RevealPaneWithId: [PaneId, string | null]; CloseMultiplePanes: PaneId[]; FloatMultiplePanes: PaneId[]; EmbedMultiplePanes: PaneId[];
  SetSelfMouseSelectionSupport: boolean; InterceptKeyPresses: undefined; ClearKeyPressesIntercepts: undefined;
  ReplacePaneWithExistingPane: [PaneId, PaneId, boolean]; RunAction: [Action, StringMap]; CopyToClipboard: string;
  OverrideLayout: [LayoutInfo, boolean, boolean, boolean, StringMap];
  SaveLayout: {layout_name: string; layout_kdl: string; overwrite: boolean}; DeleteLayout: {layout_name: string};
  RenameLayout: {old_layout_name: string; new_layout_name: string}; EditLayout: {layout_name: string; context: StringMap};
  GenerateRandomName: undefined; DumpLayout: string; ParseLayout: string; GetLayoutDir: undefined; GetUserPluginDir: undefined;
  GetFocusedPaneInfo: undefined; SaveSession: undefined; CurrentSessionLastSavedTime: undefined;
  GetPaneInfo: PaneId; GetTabInfo: number; GetSessionEnvironmentVariables: undefined; GetPluginAliasConfiguration: string;
  OpenCommandPaneInNewTab: [CommandToRun, StringMap]; OpenPluginPaneInNewTab: {plugin_url: string; configuration: StringMap; context: StringMap};
  OpenEditorPaneInNewTab: [FileToOpen, StringMap]; OpenCommandPaneInPlaceOfPaneId: [PaneId, CommandToRun, boolean, StringMap];
  OpenTerminalPaneInPlaceOfPaneId: [PaneId, FileToOpen, boolean]; OpenEditPaneInPlaceOfPaneId: [PaneId, FileToOpen, boolean, StringMap];
  HideFloatingPanes: {tab_id: number | null}; ShowFloatingPanes: {tab_id: number | null};
  SetPaneColor: [PaneId, string | null, string | null]; SetPaneRegexHighlights: [PaneId, RegexHighlight[]];
  SetContextMenu: {request_id: Integer; items: ContextMenuItem[]}; ClearPaneHighlights: PaneId;
  OpenPluginPaneFloating: {plugin_url: string; configuration: StringMap; floating_pane_coordinates: FloatingPaneCoordinates | null; context: StringMap};
  GetSessionList: undefined; KillSessionsAndReply: string[]; DeleteDeadSessionAndReply: string; DeleteAllDeadSessionsAndReply: undefined;
  FocusHostSession: undefined; ListRecordings: undefined; ListNotifications: undefined; SearchRecordings: {query: string; limit: Integer};
  WarpPaneToSession: {pane_id: PaneId; to_session: string; placement: WarpPlacement};
  WarpPaneFromSession: {pane_id: PaneId; from_session: string; placement: WarpPlacement};
  WarpTabToSession: {tab_id: number; to_session: string}; WarpTabFromSession: {tab_id: number; from_session: string};
  ListOrphanedPanes: undefined; AdoptOrphanedPane: {host_id: string; placement: WarpPlacement};
  DataDefineEntity: {namespace: string | null; def: EntityDef; allow_destructive: boolean};
  DataApply: {namespace: string | null; mutations: Mutation[]}; DataFetch: {request: FetchRequest}; DataCount: {request: FetchRequest};
  DataWatch: {request: FetchRequest; mode: WatchMode}; DataUnwatch: {watch_id: Integer}; DataCatalog: {namespace: string | null};
  SetPaneTitlePrefix: {pane_id: PaneId; prefix: string | null};
  SetPaneSubtitle: {pane_id: PaneId; subtitle: string | null};
  EnqueueQueueJob: {queue: string; plugin_url: string; config: StringMap; context: StringMap};
  ConfigureQueue: {queue: string; concurrency: Integer}; CancelQueueJob: {job_id: Integer}; QueueStatus: {queue: string | null};
  FinishQueueJob: {success: boolean; payload: string | null};
  SetPaneAgent: {pane_id: PaneId; agent: AgentInfo | null}; SetPaneAgentRun: {pane_id: PaneId; run_id: string | null};
  ProjectConfigRead: {directory: string}; ProjectConfigWrite: {directory: string; source: string; expected: string | null};
  SpawnAgent: {spec: AgentSpawnSpec; source_directory?: string | null; cwd?: string | null; name?: string | null; context?: StringMap; automatic_firecracker_vm?: boolean; start_suppressed?: boolean};
  SetQuitNotice: {notice: string | null; guard: boolean}; ReloadPluginByUrl: string;
  ReadPluginLog: {name: string; lines: Integer}; GetLocalUtcOffset: {epoch_ms: Integer};
}
// ---- section: sdk_replies
export type CommandName = keyof CommandArgs;
export type CommandObject = {[K in CommandName]: {[P in K]: CommandArgs[K] extends undefined ? null : CommandArgs[K]} & Partial<Record<Exclude<CommandName, K>, never>>}[CommandName];
/** No declared command intentionally returns undecoded protobuf bytes.
 * FsRead returns file contents as Uint8Array. Shim inventory checks require every
 * reply reader to have a decoder; absent commands have no immediate reply (null). */
export type OpaqueReplyCommand = never;
export interface CommandReplies {
  CheckPlugin: PluginCheckReport;
  GetPluginIds: PluginIds; GetZellijVersion: string; GetUserPluginDir: string | null; GetLayoutDir: string | null;
  GetLocalUtcOffset: number; GetSessionEnvironmentVariables: StringMap; GetPluginAliasConfiguration: StringMap;
  GetFocusedPaneInfo: {tab_id: number; pane_id: PaneId}; GetPaneInfo: PaneInfo | null; GetTabInfo: TabInfo | null;
  GetPaneCwd: string | null; GetPanePid: number | null; GetPaneRunningCommand: string[] | null; GetPaneGitProbe: PaneGitProbe | null;
  GetPaneScrollback: PaneContents; GetSessionList: SessionListSnapshot; CurrentSessionLastSavedTime: Integer | null;
  SaveSession: boolean; DumpLayout: string; DumpSessionLayout: {layout: string; metadata: LayoutMetadata | null};
  ParseLayout: LayoutMetadata; SaveLayout: boolean; DeleteLayout: boolean; RenameLayout: boolean; EditLayout: boolean;
  ShowFloatingPanes: boolean; HideFloatingPanes: boolean; DeleteDeadSessionAndReply: boolean; DeleteAllDeadSessionsAndReply: boolean; KillSessionsAndReply: boolean;
  WarpPaneToSession: string; WarpPaneFromSession: string; WarpTabToSession: string; WarpTabFromSession: string; AdoptOrphanedPane: string;
  ListOrphanedPanes: OrphanedPane[]; DataDefineEntity: DataSchemaInfo; DataApply: ApplyReport; DataFetch: DataRows; DataCount: Integer;
  DataWatch: DataWatchHandle; DataUnwatch: boolean; DataCatalog: DataEntityInfo[]; EnqueueQueueJob: Integer; QueueStatus: QueueSnapshot[];
  ListRecordings: RecordingSummary[]; SearchRecordings: SearchHit[]; ListNotifications: NotificationInfo[];
  NewTab: number | null; NewTabUnfocused: number | null; NewTabsWithLayout: number[]; NewTabsWithLayoutInfo: number[];
  FocusOrCreateTab: number | null; BreakPanesToNewTab: number | null; BreakPanesToTabWithIndex: number | null; BreakPanesToTabWithId: number | null;
  NewTiledPaneInTab: PaneId | null; OpenFile: PaneId | null; OpenFileFloating: PaneId | null; OpenFileInPlace: PaneId | null;
  OpenFileNearPlugin: PaneId | null; OpenFileFloatingNearPlugin: PaneId | null; OpenFileInPlaceOfPlugin: PaneId | null;
  OpenTerminal: PaneId | null; OpenTerminalFloating: PaneId | null; OpenTerminalInPlace: PaneId | null;
  OpenTerminalNearPlugin: PaneId | null; OpenTerminalFloatingNearPlugin: PaneId | null; OpenTerminalInPlaceOfPlugin: PaneId | null;
  OpenTerminalPaneInPlaceOfPaneId: PaneId | null; OpenEditPaneInPlaceOfPaneId: PaneId | null;
  OpenCommandPane: PaneId | null; OpenCommandPaneFloating: PaneId | null; OpenCommandPaneInPlace: PaneId | null; OpenCommandPaneBackground: PaneId | null;
  OpenCommandPaneNearPlugin: PaneId | null; OpenCommandPaneOverlay: PaneId | null; OpenCommandPaneFloatingNearPlugin: PaneId | null;
  OpenCommandPaneInPlaceOfPlugin: PaneId | null; OpenCommandPaneInPlaceOfPaneId: PaneId | null; OpenPluginPaneFloating: PaneId | null;
  OpenCommandPaneInGitCheckoutNearPlugin: PaneId | null; SpawnAgent: SpawnedAgent;
  OpenCommandPaneInNewTab: PaneInNewTab; OpenPluginPaneInNewTab: PaneInNewTab; OpenEditorPaneInNewTab: PaneInNewTab;
  ReadPluginLog: string; ProjectConfigRead: ProjectConfigSnapshot; ProjectConfigWrite: ProjectConfigSnapshot;
  FsRead: Uint8Array; FsList: FsEntry[]; FsMetadata: FileMetadata | null;
  FsWrite: boolean; FsRemove: boolean; FsCreateDir: boolean; FsRename: boolean;
  GenerateRandomName: string;
}
export type CommandResults = {[K in CommandName]: K extends keyof CommandReplies ? CommandReplies[K] : null};

type IsTuple<T> = T extends readonly unknown[] ? (number extends T["length"] ? false : true) : false;
/** Tuple payloads spread as positional arguments or pass as one array; other payloads are one argument. */
export type HostArgs<K extends CommandName> = CommandArgs[K] extends undefined ? []
  : IsTuple<CommandArgs[K]> extends true ? [...Extract<CommandArgs[K], readonly unknown[]>] | [payload: CommandArgs[K]]
  : [payload: CommandArgs[K]];
/** Named-argument forms of the tuple commands. Omitted optional fields become null. */
export interface HostNamedArgs {
  /** Omitted fields default to Get, no headers, an empty body and no context; a string body is UTF-8 encoded. */
  webRequest: {url: string; verb?: HttpVerb; headers?: StringMap; body?: string | Bytes; context?: StringMap; streamId?: string | null; proxy?: string | null};
  renameTab: {tabIndex: number; name: string}; renameTabWithId: {tabId: number; name: string};
  renameTerminalPane: {paneId: number; name: string}; renamePluginPane: {paneId: number; name: string};
  focusTerminalPane: {paneId: number; shouldFloatIfHidden: boolean; shouldBeInPlaceIfHidden: boolean};
  focusPluginPane: {paneId: number; shouldFloatIfHidden: boolean; shouldBeInPlaceIfHidden: boolean};
  showPaneWithId: {paneId: PaneId; shouldFloatIfHidden: boolean; shouldFocusPane: boolean};
  setFloatingPanePinned: {paneId: PaneId; shouldBePinned: boolean}; setPaneBorderless: {paneId: PaneId; borderless: boolean};
  revealPaneWithId: {paneId: PaneId; label?: string | null}; notify: {message: string; paneId?: PaneId | null; click?: NotifyClick | null};
  cliPipeOutput: {pipeName: string; output: string}; reconfigure: {configuration: string; writeToDisk: boolean};
  runCommand: {command: string[]; envVariables: StringMap; cwd: string; context: StringMap; maxBytesReturned?: Integer | null; streamOutput?: boolean | null};
  openCommandPaneInGitCheckoutNearPlugin: {command: CommandToRun; sourceDirectory: string; checkout: GitCheckout};
}
/** Every PluginCommand with its first letter lowercased, dispatched through command(). */
type HostCommand<K extends CommandName, N> = N extends keyof HostNamedArgs
  ? (...args: HostArgs<K> | [named: HostNamedArgs[N]]) => CommandResults[K]
  : (...args: HostArgs<K>) => CommandResults[K];
export type HostCommands = {readonly [K in CommandName as Uncapitalize<K>]: HostCommand<K, Uncapitalize<K>>};
export type PaneKey = keyof EventPayloads["PaneRenderReport"];
export type KdlScalar = string | number | boolean;
export interface KeybindAction {name: string; args: KdlScalar[]; props: Record<string, KdlScalar>; children?: Record<string, KdlScalar>;}
export type KeybindMode = Lowercase<InputMode> | "shared";
export type DefaultKeybinding = {mode: KeybindMode; key: string} & (
  | {actions: (string | KeybindAction)[]; handler?: never}
  | {handler: (message: PipeMessage) => boolean | void | Promise<boolean | void>; actions?: never});
export type MouseSummary = {kind: 'LeftClick' | 'RightClick' | 'Hold' | 'Release' | 'Hover'; line: Integer; col: Integer}
  | {kind: 'ScrollUp' | 'ScrollDown' | 'ScrollLeft' | 'ScrollRight'; lines: Integer};

// ---- section: sdk_types
export type Direction = 'Left' | 'Right' | 'Up' | 'Down';
export type Resize = 'Increase' | 'Decrease';
export type PaneFrameStyle = 'Full' | 'Titles' | 'None';
export type InputMode = 'Normal' | 'Locked' | 'Resize' | 'Pane' | 'Tab' | 'Scroll' | 'EnterSearch' | 'Search' | 'RenameTab' | 'RenamePane' | 'Session' | 'Move' | 'Prompt' | 'Tmux' | 'Agent' | 'Select';
export type KeyModifier = 'Ctrl' | 'Alt' | 'Shift' | 'Super';
export type BareKey = 'PageDown' | 'PageUp' | 'Left' | 'Down' | 'Up' | 'Right' | 'Home' | 'End' | 'Backspace' | 'Delete' | 'Insert' | 'Tab' | 'Esc' | 'Enter' | 'CapsLock' | 'ScrollLock' | 'NumLock' | 'PrintScreen' | 'Pause' | 'Menu' | {F: number} | {Char: string};
export interface KeyWithModifier {bare_key: BareKey; key_modifiers: KeyModifier[];}
export type Mouse = Tagged<{ScrollUp: Integer; ScrollDown: Integer; ScrollLeft: Integer; ScrollRight: Integer; LeftClick: [Integer, Integer]; RightClick: [Integer, Integer]; Hold: [Integer, Integer]; Release: [Integer, Integer]; Hover: [Integer, Integer]}>;
export type HttpVerb = 'Get' | 'Post' | 'Put' | 'Delete';
export type PercentOrFixed = {Percent: Integer} | {Fixed: Integer};
export interface FloatingPaneCoordinates {x: PercentOrFixed | null; y: PercentOrFixed | null; width: PercentOrFixed | null; height: PercentOrFixed | null; pinned: boolean | null; borderless: boolean | null;}
export interface OverlayPlacement {kind: 'Dismissible' | 'Modal'; coordinates?: FloatingPaneCoordinates | null;}
export interface ResizeStrategy {resize: Resize; direction: Direction | null; invert_on_boundaries: boolean;}
export type PaneTarget = 'Local' | {Ssh: {host: string}} | {Firecracker: {vm: string; image: string | null; worktree: string | null}};
export interface CommandToRun {path?: string; args?: string[]; cwd?: string | null; target?: PaneTarget | null; remote_host?: string | null; firecracker_vm?: string | null; firecracker_image?: string | null; firecracker_worktree?: string | null; remote_default_shell?: boolean;}
export interface FileToOpen {path?: string; line_number?: Integer | null; cwd?: string | null;}
export interface PluginMessage {name?: string; payload?: string; worker_name?: string | null;}
export interface NotifyClick {message: string; payload: string;}
export interface NewPluginArgs {should_float: boolean | null; pane_id_to_replace: PaneId | null; pane_title: string | null; cwd: string | null; skip_cache: boolean; should_focus: boolean | null;}
export interface MessageToPlugin {plugin_url: string | null; destination_plugin_id: number | null; plugin_config: StringMap; message_name: string; message_payload: string | null; message_args: StringMap; new_plugin_args: NewPluginArgs | null; floating_pane_coordinates: FloatingPaneCoordinates | null;}
export interface ConnectToSession {name: string | null; tab_position: number | null; pane_id?: [number, boolean] | null; layout: LayoutInfo | null; cwd: string | null;}
export type GitCheckout = 'Current' | {NewBranch: {branch: string; base: string}} | {NewWorktree: {branch: string; base: string; path: string}};
export interface ProjectPaneGroup {identity: string; name: string; configuration_root: string | null; pane_ids: number[];}
export type LayoutInfo = {BuiltIn: string} | {File: [string, LayoutMetadata]} | {Url: string} | {Stringified: string};
export interface LayoutMetadata {tabs: TabMetadata[]; creation_time: string; update_time: string;}
export interface TabMetadata {panes: PaneMetadata[]; name: string | null;}
export interface PaneMetadata {name: string | null; is_plugin: boolean; is_builtin_plugin: boolean;}
export interface LayoutDiagnostic {error_message: string; offset: Integer | null; len: Integer | null; help_message: string | null;}
export type LayoutParsingError = 'SyntaxError' | {KdlError: {kdl_error: LayoutDiagnostic; file_name: string; source_code: string}};
export interface LayoutWithError {layout_name: string; error: LayoutParsingError;}
export interface FileMetadata {is_dir: boolean; is_file: boolean; is_symlink: boolean; len: Integer;}
export interface FsEntry {path: string; metadata: FileMetadata | null;}
export interface Position {line: Integer; column: Integer;}
export interface CellCoordinates {x: Integer; y: Integer;}
export interface PaneContents {lines_above_viewport: string[]; lines_below_viewport: string[]; viewport: string[]; selected_text: {start: Position; end: Position} | null; cursor: CellCoordinates | null; last_output_ms_ago: Integer | null; is_scrolled: boolean;}
export interface PaneInfo {
  id: number; is_plugin: boolean; is_focused: boolean; is_fullscreen: boolean; is_floating: boolean; is_suppressed: boolean; is_stack_list_member: boolean;
  title: string; exited: boolean; exit_status: number | null; is_held: boolean; restart_error: string | null;
  pane_x: Integer; pane_content_x: Integer; pane_y: Integer; pane_content_y: Integer;
  pane_rows: Integer; pane_content_rows: Integer; pane_columns: Integer; pane_content_columns: Integer;
  cursor_coordinates_in_pane: CellCoordinates | null; terminal_command: string | null; plugin_url: string | null;
  is_selectable: boolean; index_in_pane_group: Record<string, Integer>; default_fg: string | null; default_bg: string | null;
  cwd: string | null; host: string | null; target: PaneTarget; agent: AgentInfo | null;
}
export interface TabInfo {
  position: number; name: string; active: boolean; panes_to_hide: Integer; is_fullscreen_active: boolean; is_sync_panes_active: boolean;
  are_floating_panes_visible: boolean; other_focused_clients: number[]; active_swap_layout_name: string | null; is_swap_layout_dirty: boolean;
  viewport_rows: Integer; viewport_columns: Integer; display_area_rows: Integer; display_area_columns: Integer;
  selectable_tiled_panes_count: Integer; selectable_floating_panes_count: Integer; tab_id: number; has_bell_notification: boolean; is_flashing_bell: boolean;
}
export interface PaneManifest {panes: Record<string, PaneInfo[]>;}
export interface Duration {secs: Integer; nanos: number;}
export type Keybinds = [InputMode, [KeyWithModifier, Action[]][]][];
export interface ModeInfo {
  mode: InputMode; base_mode: InputMode | null; keybinds: Keybinds; style: Style; capabilities: {arrow_fonts: boolean; kitty_graphics: boolean};
  session_name: string | null; editor: string | null; shell: string | null; currently_marking_pane_group: boolean | null;
  pane_frame_style: PaneFrameStyle | null; session_dimmed: boolean | null; session_ancestry: string[]; host_fullscreen: boolean | null;
  nested_ascend_keys: KeyWithModifier[]; session_ascended: boolean | null; nested_descend_keys: KeyWithModifier[];
}
export interface SessionInfo {
  name: string; tabs: TabInfo[]; panes: PaneManifest; connected_clients: Integer; is_current_session: boolean;
  available_layouts: LayoutInfo[]; plugins: Record<string, {location: string; configuration: StringMap}>;
  tab_history: Record<string, number[]>; pane_history: Record<string, PaneId[]>; creation_time: Duration;
}
export interface SessionListSnapshot {live_sessions: SessionInfo[]; resurrectable_sessions: [string, Duration][];}
export interface PaneGitProbe {host: string; cwd: string; exit_code: number | null; metadata: number[]; config: number[];}
export interface OrphanedPane {host_id: string; session_name: string | null; command: string; cwd: string | null; child_pid: number | null; started_at_unix: Integer; exit_status: number | null; buffered_bytes: Integer;}
export interface ClientInfo {client_id: number; pane_id: PaneId; running_command: string; is_current_client: boolean;}
export interface NotificationInfo {id: Integer; message: string; received_at_unix_ms: Integer;}
export interface PaneInNewTab {tab_id: number | null; pane_id: PaneId | null;}
export interface RecordingSummary {id: Integer; session_name: string; tab_name: string; terminal_id: number; pane_title: string; started_at_us: Integer; duration_us: Integer; is_running: boolean; capture_input: boolean; error: string | null;}
export interface SearchHit {recording_id: Integer; session_name: string; tab_name: string; pane_title: string; elapsed_us: Integer; started_at_us: Integer; text: string;}
// ---- section: sdk_types_data
export type PaletteColor = {Rgb: [number, number, number]} | {EightBit: number};
export interface StyleDeclaration {base: PaletteColor; background: PaletteColor; emphasis_0: PaletteColor; emphasis_1: PaletteColor; emphasis_2: PaletteColor; emphasis_3: PaletteColor;}
export interface Style {colors: Styling; rounded_corners: boolean; hide_session_name: boolean;}
export interface Styling {
  text_unselected: StyleDeclaration; text_selected: StyleDeclaration; ribbon_unselected: StyleDeclaration; ribbon_selected: StyleDeclaration;
  table_title: StyleDeclaration; table_cell_unselected: StyleDeclaration; table_cell_selected: StyleDeclaration; list_unselected: StyleDeclaration; list_selected: StyleDeclaration;
  frame_unselected: StyleDeclaration | null; frame_selected: StyleDeclaration; frame_highlight: StyleDeclaration; exit_code_success: StyleDeclaration; exit_code_error: StyleDeclaration;
  multiplayer_user_colors: Record<`player_${1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10}`, PaletteColor>;
}
export type HighlightStyle = 'None' | 'Emphasis0' | 'Emphasis1' | 'Emphasis2' | 'Emphasis3' | 'BackgroundEmphasis0' | 'BackgroundEmphasis1' | 'BackgroundEmphasis2' | 'BackgroundEmphasis3'
  | {CustomRgb: {fg: [number, number, number] | null; bg: [number, number, number] | null}} | {CustomIndex: {fg: number | null; bg: number | null}};
export interface RegexHighlight {pattern: string; style: HighlightStyle; layer: 'Hint' | 'Tool' | 'ActionFeedback'; context: StringMap; on_hover: boolean; bold: boolean; italic: boolean; underline: boolean; tooltip_text: string | null;}
export interface ContextMenuItem {id?: Integer | null; label?: string; shortcut?: string | null; actions?: Action[]; context?: StringMap; disabled?: boolean; separator?: boolean;}
export interface ContextMenuContext {
  request_id: Integer; client_id: number; pane_id: PaneId; tab_position: number; click: [Integer, Integer]; click_in_pane: [Integer, Integer];
  on_frame: boolean; pane_title: string; pane_command: string | null; pane_exited: boolean; pane_exit_status: number | null; pane_is_held: boolean; pane_is_floating: boolean;
  selected_text: string | null; word_under_cursor: string | null; line_under_cursor: string | null;
  link: {plugin_id: number; pattern: string; matched: string; context: StringMap} | null; items: ContextMenuItem[];
}
export type AgentState = 'Busy' | 'Waiting' | 'Idle' | 'Exited';
export interface AgentInfo {kind: string; state: AgentState; source?: 'Spawned' | 'Registered' | 'Reported'; detail?: string | null; waiting_on?: string | null; wait_needs_a_person?: boolean; cwd?: string | null; session_id?: string | null; run_id?: string | null;}
export interface AgentSpawnSpec {
  kind?: string | null; command?: string | null; args?: string[]; model?: string | null; effort?: string | null; prompt?: string | null;
  branch?: string | null; base?: string | null; worktree?: boolean | null; worktree_dir?: string | null; env?: StringMap; permission_mode?: string | null;
  resume?: string | null; hooks?: boolean | null; firecracker?: string | null; firecracker_image?: string | null; reuse?: boolean | null; allow_outside_worktree?: boolean | null;
}
export interface SpawnedAgent {pane_id: PaneId; branch: string | null; cwd: string | null;}
export interface AgentEvent {pane_id: number; kind: string; hook: string; session_id: string | null; state: AgentState | null; detail: string | null; prompt: string | null; last_message: string | null; tool_name: string | null; cwd: string | null; raw: string;}
export type HookPoint = 'agent_exit' | 'agent_start' | 'agent_waiting' | 'agent_idle' | 'pane_exit' | 'session_start' | 'context_menu';
export type HookPayload = {pane_id: number; agent: AgentInfo; previous_state: AgentState | null; command: string | null}
  | {pane_id: number; exit_code: number | null; cwd: string | null; command: string | null; held: boolean}
  | {session_name: string; client_id: number};
export type WarpPlacement = 'NewTab' | 'FocusedTab' | 'Floating';
export type QueueJobOutcome = 'Succeeded' | 'Failed' | 'Cancelled' | 'Aborted';
export interface QueueJobInfo {job_id: Integer; queue: string; plugin_url: string; plugin_id: number | null; enqueuer_plugin_id: number; context: StringMap; state: 'Pending' | 'Running';}
export interface QueueSnapshot {queue: string; concurrency: Integer; pending: QueueJobInfo[]; running: QueueJobInfo[];}
export interface ProjectConfigSnapshot {root: string; source: string | null; config: ProjectConfig; error: string | null;}
export interface ProjectConfig {workflow: string | null; control_checkout: string | null; ci: string[] | null; default_branch: string | null; rebase_prompt: string | null; agent: AgentSpawnSpec | null; commit_agent: AgentSpawnSpec | null; review_agent: AgentSpawnSpec | null; review_fix_agent: AgentSpawnSpec | null; rebase_agent: AgentSpawnSpec | null; test_fix_agent: AgentSpawnSpec | null;}
export type DataValue = 'Null' | {Int: Integer} | {Real: number} | {Text: string} | {Bool: boolean} | {Blob: Bytes};
export type AttrKind = 'Text' | 'Int' | 'Real' | 'Bool' | 'Blob' | 'Json';
export interface AttrDef {name: string; kind: AttrKind; indexed: boolean; full_text: boolean; unique: boolean; required: boolean; default: DataValue | null;}
export interface EntityDef {name: string; version: number; attrs: AttrDef[];}
export type Pred = Tagged<{Eq: [string, DataValue]; Ne: [string, DataValue]; Gt: [string, DataValue]; Gte: [string, DataValue]; Lt: [string, DataValue]; Lte: [string, DataValue]; In: [string, DataValue[]]; IsNull: string; NotNull: string; Prefix: [string, string]; Contains: [string, string]; And: Pred[]; Or: Pred[]; Not: Pred}>;
export interface FetchRequest {namespace: string | null; entity: string; filter: Pred | null; search: string | null; sort: [string, 'Asc' | 'Desc'][]; select: string[]; limit: number; offset: number; after_id: Integer | null;}
export type Mutation = {Put: {entity: string; uid: string | null; values: [string, DataValue][]; expect_rev?: Integer | null}}
  | {Update: {entity: string; filter: Pred; values: [string, DataValue][]; expect_rev?: Integer | null}} | {Delete: {entity: string; filter: Pred}}
  | {AssertRows: {entity: string; filter: Pred; rows: [Integer, Integer][]}};
export interface DataRows {columns: string[]; rows: DataValue[][]; truncated: boolean; next_cursor: Integer | null;}
export interface DataSchemaInfo {namespace: string; entity: string; version: number; created: boolean; attrs_added: string[]; indexes_created: string[]; full_text_rebuilt: boolean;}
export interface ApplyReport {rows_written: Integer; rows_deleted: Integer; ids: Integer[]; per_mutation: Integer[];}
export type WatchMode = 'Rows' | 'Ids';
export interface DataWatchHandle {watch_id: Integer; rows: DataRows;}
export interface DataEntityInfo {namespace: string; owner_plugin: string; def: EntityDef;}
export interface OriginatingPlugin {plugin_id: number; client_id: number; context: StringMap;}
export interface RunCommandAction {
  cmd: string; args?: string[]; cwd?: string | null; direction?: Direction | null; hold_on_close?: boolean; hold_on_start?: boolean;
  originating_plugin?: OriginatingPlugin | null; use_terminal_title?: boolean; target?: PaneTarget; remote_default_shell?: boolean;
}
export interface RunCommand extends Omit<RunCommandAction, 'cmd' | 'direction'> {command: string; workflow_owner?: string | null;}
export interface RunPlugin { _allow_exec_host_cmd?: boolean; location: {File: string} | {Zellij: string} | {Remote: string}; configuration: StringMap; initial_cwd: string | null; relative_path?: string | null;}
export type RunPluginOrAlias = {RunPlugin: RunPlugin} | {Alias: {name: string; configuration: StringMap | null; initial_cwd: string | null; run_plugin: RunPlugin | null}};
export type Run = {plugin: RunPluginOrAlias} | {command: RunCommand} | {EditFile: [string, Integer | null, string | null]} | {Cwd: string};
export interface TiledPaneLayout {
  user_named?: boolean;
  children_split_direction: 'Horizontal' | 'Vertical'; name: string | null; children: TiledPaneLayout[]; split_size: PercentOrFixed | null;
  run: Run | null; borderless: boolean | null; focus: boolean | null; external_children_index: Integer | null;
  children_are_stacked: boolean; is_expanded_in_stack: boolean; exclude_from_sync: boolean | null; run_instructions_to_ignore: (Run | null)[];
  hide_floating_panes: boolean; pane_initial_contents: string | null; default_fg: string | null; default_bg: string | null;
}
export interface FloatingPaneLayout extends FloatingPaneCoordinates {
  name: string | null; run: Run | null; focus: boolean | null; already_running: boolean; pane_initial_contents: string | null;
  logical_position: Integer | null; default_fg: string | null; default_bg: string | null;
}
/** JSON can represent only the unit constraint as a map key. */
export type SwapTiledLayout = [Partial<Record<'NoConstraint', TiledPaneLayout>>, string | null];
export type SwapFloatingLayout = [Partial<Record<'NoConstraint', FloatingPaneLayout[]>>, string | null];
export interface TabLayoutInfo {tab_index: number; tab_name: string | null; tiled_layout: TiledPaneLayout; floating_layouts: FloatingPaneLayout[]; swap_tiled_layouts: SwapTiledLayout[] | null; swap_floating_layouts: SwapFloatingLayout[] | null;}
export type CommandOrPlugin = {Command: RunCommandAction} | {Plugin: RunPluginOrAlias} | {File: FileToOpen};
export type UnblockCondition = 'OnExitSuccess' | 'OnExitFailure' | 'OnAnyExit';
export type NewPanePlacement = {NoPreference: {borderless: boolean | null}} | {Tiled: {direction: Direction | null; borderless: boolean | null}}
  | {Floating: FloatingPaneCoordinates | null} | {InPlace: {pane_id_to_replace: PaneId | null; close_replaced_pane: boolean; borderless: boolean | null}}
  | {Stacked: {pane_id_to_stack_under: PaneId | null; borderless: boolean | null}} | {Overlay: OverlayPlacement};
export interface OpenFilePayload {path: string; line_number: Integer | null; cwd: string | null; originating_plugin: OriginatingPlugin | null;}
export interface MouseEvent {event_type: 'Press' | 'Release' | 'Motion'; left: boolean; right: boolean; middle: boolean; wheel_up: boolean; wheel_down: boolean; wheel_left?: boolean; wheel_right?: boolean; shift: boolean; alt: boolean; ctrl: boolean; position: Position;}
export interface WarpedPane {host_id: string; control_socket: string; title: string | null; contents: string | null; borderless: boolean; remote: {host: string; pane_token: string; received: Integer; cmd: RunCommand} | null; agent?: AgentInfo | null;}
export interface WarpedTab {name: string | null; tiled_layout: TiledPaneLayout; floating_layout: FloatingPaneLayout[]; tiled_panes: WarpedPane[]; floating_panes: WarpedPane[]; hide_floating_panes: boolean;}
// ---- section: sdk_actions
interface ActionPane {pane_id: PaneId;}
interface ActionTab {id: number;}
interface PaneLaunch {pane_name: string | null; near_current_pane: boolean; no_focus: boolean; tab_id: number | null;}
interface CommandPaneLaunch extends PaneLaunch {command: RunCommandAction | null;}
interface PluginPaneLaunch {plugin: RunPluginOrAlias; pane_name: string | null; skip_cache: boolean; no_focus: boolean; tab_id: number | null;}
interface ActionPipe {name: string | null; payload: string | null; args: StringMap | null; plugin: string | null; configuration: StringMap | null; launch_new: boolean; skip_cache: boolean; floating: boolean | null; in_place: boolean | null; cwd: string | null; pane_title: string | null;}
export interface ActionPayloads {
  Write: {key_with_modifier: KeyWithModifier | null; bytes: Bytes; is_kitty_keyboard_protocol: boolean}; WriteChars: {chars: string};
  WriteToPaneId: ActionPane & {bytes: Bytes}; WriteCharsToPaneId: ActionPane & {chars: string}; Paste: {chars: string; pane_id: PaneId | null};
  SwitchToMode: {input_mode: InputMode}; SwitchModeForAllClients: {input_mode: InputMode}; Resize: {resize: Resize; direction: Direction | null};
  MoveFocus: {direction: Direction}; MoveFocusOrTab: {direction: Direction}; MovePane: {direction: Direction | null};
  DumpScreen: {file_path: string | null; include_scrollback: boolean; pane_id: PaneId | null; ansi: boolean}; EditScrollback: {ansi: boolean};
  ScrollUpAt: {position: Position}; ScrollDownAt: {position: Position}; MoveTextSelection: {direction: Direction}; SetPaneFrameStyle: PaneFrameStyle;
  NewPane: {direction: Direction | null; pane_name: string | null; start_suppressed: boolean};
  NewBlockingPane: CommandPaneLaunch & {placement: NewPanePlacement; unblock_condition: UnblockCondition | null};
  EditFile: {payload: OpenFilePayload; direction: Direction | null; floating: boolean; in_place: boolean; close_replaced_pane: boolean; start_suppressed: boolean; placement: NewPanePlacement; near_current_pane: boolean; no_focus: boolean; tab_id: number | null};
  NewFloatingPane: CommandPaneLaunch & {placement: NewPanePlacement}; NewTiledPane: CommandPaneLaunch & {direction: Direction | null; borderless: boolean | null};
  NewInPlacePane: CommandPaneLaunch & {pane_id_to_replace: PaneId | null; close_replaced_pane: boolean}; NewStackedPane: CommandPaneLaunch;
  ShowFloatingPanes: {tab_id: number | null}; HideFloatingPanes: {tab_id: number | null}; AreFloatingPanesVisible: {tab_id: number | null};
  PaneNameInput: {input: Bytes}; NewTab: {layout_source?: string | null; tiled_layout: TiledPaneLayout | null; floating_layouts: FloatingPaneLayout[]; swap_tiled_layouts: SwapTiledLayout[] | null; swap_floating_layouts: SwapFloatingLayout[] | null; tab_name: string | null; should_change_focus_to_new_tab: boolean; cwd: string | null; initial_panes: CommandOrPlugin[] | null; first_pane_unblock_condition: UnblockCondition | null};
  GoToTab: {index: number}; GoToTabName: {name: string; create: boolean}; TabNameInput: {input: Bytes}; MoveTab: {direction: Direction};
  Run: {command: RunCommandAction; near_current_pane: boolean; no_focus: boolean}; SetPaneColor: ActionPane & {fg: string | null; bg: string | null};
  SetPaneAgent: ActionPane & {agent: AgentInfo | null}; AgentEvent: AgentEvent;
  SwitchSession: {name: string; tab_position: number | null; pane_id?: [number, boolean] | null; layout: LayoutInfo | null; cwd: string | null};
  WarpPane: {pane_id: PaneId | null; to_session: string; placement: WarpPlacement}; AdoptPane: {payload: WarpedPane; placement: WarpPlacement};
  WarpTab: {tab_id: number | null; to_session: string}; AdoptTab: {payload: WarpedTab}; WarpTabFrom: {from_session: string; tab_id: number};
  LaunchOrFocusPlugin: {plugin: RunPluginOrAlias; should_float: boolean; move_to_focused_tab: boolean; should_open_in_place: boolean; close_replaced_pane: boolean; skip_cache: boolean; tab_id: number | null};
  LaunchPlugin: {plugin: RunPluginOrAlias; should_float: boolean; should_open_in_place: boolean; close_replaced_pane: boolean; skip_cache: boolean; cwd: string | null; no_focus: boolean; tab_id: number | null};
  MouseEvent: {event: MouseEvent}; SkipConfirm: {action: Action}; SearchInput: {input: Bytes}; Search: {direction: 'Down' | 'Up'};
  SearchToggleOption: {option: 'CaseSensitivity' | 'WholeWord' | 'Wrap'};
  OverrideLayout: {tabs: TabLayoutInfo[]; retain_existing_terminal_panes: boolean; retain_existing_plugin_panes: boolean; apply_only_to_active_tab: boolean};
  NewTiledPluginPane: PluginPaneLaunch & {cwd: string | null}; NewFloatingPluginPane: PluginPaneLaunch & {cwd: string | null; placement: NewPanePlacement};
  NewInPlacePluginPane: PluginPaneLaunch & {close_replaced_pane: boolean}; StartOrReloadPlugin: {plugin: RunPluginOrAlias};
  CloseTerminalPane: {pane_id: number}; ClosePluginPane: {pane_id: number};
  FocusTerminalPaneWithId: {pane_id: number; should_float_if_hidden: boolean; should_be_in_place_if_hidden: boolean};
  FocusPluginPaneWithId: {pane_id: number; should_float_if_hidden: boolean; should_be_in_place_if_hidden: boolean};
  RenameTerminalPane: {pane_id: number; name: Bytes}; RenamePluginPane: {pane_id: number; name: Bytes}; RenameTab: {tab_index: number; name: Bytes};
  GoToTabById: ActionTab; CloseTabById: ActionTab; RenameTabById: ActionTab & {name: string}; RenameSession: {name: string};
  Reconfigure: {config: string; write_to_disk: boolean}; Notify: string; CliPipe: ActionPipe & {pipe_id: string}; KeybindPipe: ActionPipe & {plugin_id: number | null};
  ListPanes: {show_tab: boolean; show_command: boolean; show_state: boolean; show_geometry: boolean; show_all: boolean; output_json: boolean};
  ListTabs: {show_state: boolean; show_dimensions: boolean; show_panes: boolean; show_layout: boolean; show_all: boolean; output_json: boolean};
  CurrentTabInfo: {output_json: boolean}; StackPanes: {pane_ids: PaneId[]}; ChangeFloatingPaneCoordinates: ActionPane & {coordinates: FloatingPaneCoordinates};
  TogglePaneBorderless: ActionPane; SetPaneBorderless: ActionPane & {borderless: boolean};
  ScrollUpByPaneId: ActionPane; ScrollDownByPaneId: ActionPane; ScrollToTopByPaneId: ActionPane; ScrollToBottomByPaneId: ActionPane;
  PageScrollUpByPaneId: ActionPane; PageScrollDownByPaneId: ActionPane; HalfPageScrollUpByPaneId: ActionPane; HalfPageScrollDownByPaneId: ActionPane;
  ResizeByPaneId: ActionPane & {resize: Resize; direction: Direction | null}; MovePaneByPaneId: ActionPane & {direction: Direction | null};
  MovePaneBackwardsByPaneId: ActionPane; ClearScreenByPaneId: ActionPane; EditScrollbackByPaneId: ActionPane & {ansi: boolean};
  ToggleFocusFullscreenByPaneId: ActionPane; ToggleFocusNoUiFullscreenByPaneId: ActionPane; TogglePaneEmbedOrFloatingByPaneId: ActionPane;
  CloseFocusByPaneId: ActionPane; RenamePaneByPaneId: {pane_id: PaneId | null; name: Bytes}; UndoRenamePaneByPaneId: ActionPane;
  TogglePanePinnedByPaneId: ActionPane; FocusPaneByPaneId: ActionPane; UndoRenameTabByTabId: ActionTab; ToggleActiveSyncTabByTabId: ActionTab;
  ToggleFloatingPanesByTabId: ActionTab; PreviousSwapLayoutByTabId: ActionTab; NextSwapLayoutByTabId: ActionTab; MoveTabByTabId: ActionTab & {direction: Direction};
}
export type UnitAction = 'Quit' | 'FocusNextPane' | 'FocusPreviousPane' | 'FocusLastPane' | 'SwitchFocus' | 'MovePaneBackwards' | 'ClearScreen'
  | 'DumpLayout' | 'SaveSession' | 'ScrollUp' | 'ScrollDown' | 'ScrollToBottom' | 'ScrollToTop' | 'ScrollToPreviousPrompt' | 'ScrollToNextPrompt'
  | 'SelectCommandAtScrollPosition' | 'CopyLastCommandOutput' | 'SwapTextSelectionEnds' | 'PageScrollUp' | 'PageScrollDown' | 'HalfPageScrollUp' | 'HalfPageScrollDown'
  | 'ToggleFocusFullscreen' | 'ToggleFocusNoUiFullscreen' | 'TogglePaneFrames' | 'ToggleActiveSyncTab' | 'TogglePaneEmbedOrFloating' | 'ToggleFloatingPanes'
  | 'CloseFocus' | 'UndoRenamePane' | 'NoOp' | 'GoToNextTab' | 'GoToPreviousTab' | 'CloseTab' | 'ToggleTab' | 'UndoRenameTab' | 'Detach'
  | 'SetDarkTheme' | 'SetLightTheme' | 'ToggleTheme' | 'Copy' | 'Confirm' | 'Deny' | 'ToggleMouseMode' | 'PreviousSwapLayout' | 'NextSwapLayout' | 'QueryTabNames'
  | 'ArrangePanesByProject' | 'BreakPane' | 'BreakPaneRight' | 'BreakPaneLeft' | 'FocusHostSession' | 'FocusGuestSession' | 'ToggleHostFullscreen'
  | 'ListClients' | 'TogglePanePinned' | 'TogglePaneInGroup' | 'ToggleGroupMarking';
export type Action = UnitAction | Tagged<ActionPayloads>;

// ---- section: sdk_helpers
export interface Filesystem {
  read(path: string, options?: {maxBytes?: Integer}): Uint8Array;
  list(path: string): FsEntry[]; metadata(path: string): FileMetadata | null;
  write(path: string, contents: string | Uint8Array, options?: {append?: boolean; createDirs?: boolean}): void;
  remove(path: string, options?: {recursive?: boolean}): void; mkdir(path: string, options?: {all?: boolean}): void; rename(from: string, to: string): void;
}
export interface ProjectConfiguration {
  read(directory: string): ProjectConfigSnapshot;
  write(directory: string, source: string, expectedSource: string | null): ProjectConfigSnapshot;
}
export type DataScalar = null | boolean | Integer | string | Uint8Array;
export type DataInput = DataScalar | DataValue;
export type DataRecord = Record<string, DataScalar> & {rev?: Integer};
export interface DataSnapshot {records: DataRecord[]; raw: DataRows;}
export interface DataQuery {entity: string; namespace?: string | null; filter?: Pred | null; search?: string | null; sort?: [string, 'Asc' | 'Desc'][]; select?: string[]; limit?: number; offset?: number; afterId?: Integer | null;}
export interface DataAttribute {kind: AttrKind; indexed?: boolean; fullText?: boolean; unique?: boolean; required?: boolean; default?: DataInput;}
export interface DataWatch {watchId: Integer; initial: DataSnapshot; dispose(): void;}
export interface DataStore {
  /** The expectRev of a put that must create: the row must not exist yet. */
  readonly REV_ABSENT: 0;
  define(name: string, attributes: Record<string, AttrKind | DataAttribute>, options?: {namespace?: string | null; version?: number; allowDestructive?: boolean}): DataSchemaInfo;
  catalog(namespace?: string | null): DataEntityInfo[]; fetch(query: DataQuery): DataSnapshot; count(query: DataQuery): Integer;
  apply(mutations: Mutation[], options?: {namespace?: string | null}): ApplyReport;
  put(entity: string, values: Record<string, DataInput>, options?: {uid?: string | null; expectRev?: Integer | null}): Mutation;
  update(entity: string, filter: Pred, values: Record<string, DataInput>, options?: {expectRev?: Integer | null}): Mutation;
  delete(entity: string, filter: Pred): Mutation;
  eq(column: string, value: DataInput): Pred; ne(column: string, value: DataInput): Pred;
  gt(column: string, value: DataInput): Pred; gte(column: string, value: DataInput): Pred;
  lt(column: string, value: DataInput): Pred; lte(column: string, value: DataInput): Pred;
  in(column: string, values: DataInput[]): Pred; isNull(column: string): Pred; notNull(column: string): Pred;
  prefix(column: string, value: string): Pred; contains(column: string, value: string): Pred;
  and(...filters: Pred[]): Pred; or(...filters: Pred[]): Pred; not(filter: Pred): Pred;
  toValue(value: DataInput): DataValue; fromValue(value: DataValue): DataScalar;
  fromRecord(record: Record<string, DataInput>): [string, DataValue][]; toRecords(raw: DataRows): DataRecord[];
  int(value: Integer): {Int: Integer}; real(value: number): {Real: number}; json(value: Json): {Text: string}; isConflict(error: unknown): boolean;
  watch(query: DataQuery, onChange: (snapshot: DataSnapshot, event: EventPayloads['PluginDataChanged']) => boolean | void | Promise<boolean | void>, options?: {mode?: WatchMode}): DataWatch;
}

export type PluginCheckMode = 'Syntax' | 'Semantic';
export interface OutlineEntry {line: Integer; kind: 'Require' | 'Permission' | 'Function' | 'Handler' | 'Define' | 'Registration' | 'Tool'; text: string}
export interface PluginCheckReport {diagnostics: string[]; outline: OutlineEntry[]; permissions: Permission[]; warnings: string[]}

/** Tool handlers answer version-1 Hocus RPC calls. Source is supplied by the host. */
export interface RpcCall { version: 1; callId: string; name: string; payload: Json; source: {Plugin: number}; progress(payload: Json): void; }
export type HocusToolCall = RpcCall;
export type RpcOptions = { name: string; payload?: Json; timeoutMs?: number; onProgress?: (payload: Json) => void } & (
  { pluginId: number; pluginUrl?: never; pluginConfig?: never } |
  { pluginUrl: string; pluginId?: never; pluginConfig?: Record<string, string> }
);
export interface Rpc {
  handle(name: string, handler: (payload: Json, call: RpcCall) => Json | void | Promise<Json | void>): void;
  call(options: RpcOptions): CancellablePromise<Json>;
}
export interface HocusToolDefinition {
  description?: string;
  parameters?: {type: 'object'; [key: string]: Json};
  safety?: 'read_only' | 'automatic' | 'confirm';
  /** Required for confirm. */
  consequence?: string;
  handler(payload: Json, call: HocusToolCall): Json | void | Promise<Json | void>;
}
export interface HocusExtensions {
  complete(options: CompletionOptions): CancellablePromise<CompletionResult>;
  /** Requires MessageAndLaunchOtherPlugins. Replaces all handlers and registrations. */
  tools(definitions: Record<string, HocusToolDefinition>): void;
  launch(): {namespace: string; path: string} | null;
}
export interface Viewport { x?: number; y?: number; width: number; height: number; }
export interface MarkdownViewport extends Viewport { scroll?: number; }
export type WidgetInteraction =
  | {kind: 'fieldChanged' | 'complete' | 'buttonPressed'; id: string; shouldRender: boolean}
  | {kind: 'focusMoved' | 'cancel' | 'ignored'; shouldRender: boolean};
export type WidgetValue = string | boolean | number | null;
export interface WidgetInteractionMethods {
  focusedId(): string | null;
  moveFocus(forward?: boolean): WidgetInteraction;
  /** The key, or the raw Key/InterceptedKeyPress event as it arrived. */
  handleKey(key: KeyWithModifier | {Key: KeyWithModifier} | {InterceptedKeyPress: KeyWithModifier}): WidgetInteraction;
  /** The mouse payload, or the raw Mouse event as it arrived. */
  handleMouse(mouse: Mouse | {Mouse: Mouse}): WidgetInteraction;
  insertPastedText(text: string): WidgetInteraction;
  /** Returns terminal text; cursor updates are attached to the current render invocation. */
  render(viewport: Viewport): string;
  values(): Record<string, WidgetValue>;
}
export interface Control<T extends WidgetValue> extends WidgetInteractionMethods {
  value(): T;
  setValue(value: T): this;
  focus(): this;
}
export interface ChoiceControl<T extends WidgetValue> extends Control<T> { setOptions(options: string[]): this; }
export interface FormWidget extends WidgetInteractionMethods {
  value(id: string): WidgetValue;
  setValue(id: string, value: WidgetValue): this;
  setOptions(id: string, options: string[]): this;
  focus(id: string): this;
}
export interface ButtonControl extends WidgetInteractionMethods { focus(): this; }
export interface DialogWidget extends FormWidget { setStatus(text?: string | null): this; }
export interface ControlOptions { id?: string; label?: string; }
export interface TextInputOptions extends ControlOptions { value?: string; }
export interface CheckboxOptions extends ControlOptions { value?: boolean; }
/** Selections use zero-based indices; null means no selection. */
export interface RadioGroupOptions extends ControlOptions { options?: string[]; value?: number | null; }
export interface SelectOptions extends RadioGroupOptions { placeholder?: string; maxVisibleOptions?: number; }
export interface ComboOptions extends TextInputOptions { options?: string[]; placeholder?: string; maxVisibleOptions?: number; }
export interface ButtonOptions extends ControlOptions { disabled?: boolean; style?: 'ribbon' | 'text'; }
export type FieldDescriptor = {id: string} & (
  | ({kind: 'textInput'} & TextInputOptions) | ({kind: 'checkbox'} & CheckboxOptions)
  | ({kind: 'radioGroup'} & RadioGroupOptions) | ({kind: 'select'} & SelectOptions)
  | ({kind: 'combo'} & ComboOptions) | ({kind: 'button'} & ButtonOptions));
export interface StyledText {
  len(): number;
  selected(): this; opaque(): this; disabled(): this;
  /** Ranges count Unicode characters, with zero-based starts and exclusive ends. */
  emphasis(level: number, from?: number, to?: number): this;
  dim(from?: number, to?: number): this; unbold(from?: number, to?: number): this;
  error(from?: number, to?: number): this; success(from?: number, to?: number): this;
  render(viewport?: Viewport): string;
  /** render() without a viewport, so a StyledText can be used where a string is. */
  toString(): string;
}
export type ComponentText = string | StyledText;
export interface TableComponent { row(...cells: ComponentText[]): this; render(viewport?: Viewport): string; }
export interface NestedListComponent { item(text: ComponentText, indent?: number): this; render(viewport?: Viewport): string; }
export interface MarkdownComponent { render(viewport: MarkdownViewport): {text: string; totalRows: number}; }
export interface UserInterface {
  text(text?: ComponentText): StyledText; ribbon(text?: ComponentText): StyledText;
  table(rows?: ComponentText[][]): TableComponent;
  nestedList(items?: (ComponentText | {text: ComponentText; indent?: number})[]): NestedListComponent;
  textInput(options?: TextInputOptions): Control<string>;
  checkbox(options?: CheckboxOptions): Control<boolean>;
  radioGroup(options?: RadioGroupOptions): ChoiceControl<number | null>;
  select(options?: SelectOptions): ChoiceControl<number | null>;
  combo(options?: ComboOptions): ChoiceControl<string>;
  button(options?: ButtonOptions): ButtonControl;
  form(fields: FieldDescriptor[]): FormWidget;
  dialog(options: {title: string; message?: string; help?: string; fields?: FieldDescriptor[]}): DialogWidget;
  markdown(source: string, options?: {compact?: boolean; dim?: boolean}): MarkdownComponent;
}
export interface TerminalText {
  width(text: string): number;
  truncate(text: string, width: number, ellipsis?: string): string;
  pad(text: string, width: number): string;
  padLeft(text: string, width: number): string;
  center(text: string, width: number): string;
  wrap(text: string, width: number): string[];
  sanitize(text: string): string;
  /** sanitize() over lossily decoded UTF-8 bytes. */
  sanitizeBytes(bytes: Bytes | string): string;
}

export type AgentPane = number | {Terminal: number};
export type AgentStateFilter = AgentState | Lowercase<AgentState>;
export interface AgentSpawnOptions extends AgentSpawnSpec {
  sourceDirectory?: string; cwd?: string; name?: string; context?: StringMap;
  automaticFirecrackerVm?: boolean; startSuppressed?: boolean;
}
export interface AgentTurnOptions {fallback?: 'idle';}
export interface AgentTurn {message: string | null; hook: string | null; state: AgentState | null; sessionId: string | null;}
export interface Agents {
  spawnDetails(options: AgentSpawnOptions): SpawnedAgent;
  spawn(options: AgentSpawnOptions): PaneId;
  info(pane: AgentPane): AgentInfo | null;
  all(): Record<number, AgentInfo>;
  on(handler: (event: AgentEvent) => boolean | void | Promise<boolean | void>): () => void;
  send(pane: AgentPane, text: string, options?: {enter?: boolean}): void;
  /** Resolves on a future PaneUpdate; after requires a preceding matching update. */
  waitForState(pane: AgentPane, states: AgentStateFilter | AgentStateFilter[], options?: {after?: AgentStateFilter | AgentStateFilter[]}): CancellablePromise<PaneInfo>;
  waitForEvent(pane: AgentPane, hooks?: string | string[]): CancellablePromise<AgentEvent>;
  waitForTurn(pane: AgentPane, options?: AgentTurnOptions): CancellablePromise<AgentTurn>;
  prompt(pane: AgentPane, text: string, options?: AgentTurnOptions & {enter?: boolean}): CancellablePromise<AgentTurn>;
}
/** Nested fields retain the completion wire spelling for transcript round trips. */
export interface CompletionToolCall {id: string; name: string; arguments: string;}
export type CompletionMessage = {
  role: 'system' | 'user' | 'assistant'; content?: string; tool_calls?: CompletionToolCall[]; tool_call_id?: string | null;
} | {role: 'tool'; content?: string; tool_calls?: CompletionToolCall[]; tool_call_id: string;};
export interface CompletionUsage {prompt_tokens: number; completion_tokens: number; total_tokens: number;}
export interface CompletionOptions {
  messages: CompletionMessage[]; system?: string | null; model?: string | null; tools?: Json[]; toolChoice?: Json;
  temperature?: number | null; maxTokens?: number | null; timeoutMs?: number;
}
export interface CompletionResult {
  content: string; toolCalls: CompletionToolCall[]; finishReason: string; usage: CompletionUsage | null; model: string;
}

export interface PaneStep extends CommandToRun {name?: string; stdin?: string; timeoutMs?: number;}
export interface PaneExit {paneId: {Terminal: number}; exitCode: number | null; closed: boolean;}
export type PaneSequenceOutcome = 'success' | 'failed' | 'cancelled' | 'pane_closed';
export interface PaneSequenceResult {status: PaneSequenceOutcome; paneId: PaneId | null; exitCodes: (number | null)[]; failedStep: number | null;}
export interface PaneSequenceOptions {
  steps: PaneStep[]; name?: string; floating?: boolean | FloatingPaneCoordinates;
  stopOnFailure?: boolean; closeOnSuccess?: boolean; onCancel?: 'close' | 'keep';
  keepScrollback?: boolean; killGraceMs?: number;
  onStep?: (number: number, step: PaneStep, exitCode: number | null) => void | Promise<void>;
  onDone?: (result: PaneSequenceResult) => void | Promise<void>;
}
export interface PaneSequence {
  readonly paneId: PaneId | null; readonly status: 'running' | 'cancelling' | PaneSequenceOutcome;
  readonly done: Promise<PaneSequenceResult>; push(step: PaneStep): void; cancel(): void;
}
