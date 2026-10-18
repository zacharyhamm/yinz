# Plugin command and event shapes

This reference describes the values accepted by `yinz-command` and delivered to
`update` and `pipe`. Use the Scheme functions in [the SDK reference](PLUGIN_SDK.md)
to construct and consume them.

The notation describes data, not executable plugin source:

- `string`, `boolean`, `number`, and `null` correspond to Scheme strings, booleans,
  numbers, and `json-null`. `Integer` is an exact signed or unsigned integer within
  the destination field's range. `Bytes` is a bytevector in binary command replies
  and output events. Binary command arguments also accept lists of integers from
  0 through 255; the JSON wire notation below shows them as integer arrays.
- `T[]` is a list. `[A, B]` is a positional list of two values. `{field: T}` is a
  string-keyed hash. `field?` is optional. `Record<string, T>` is a string-keyed hash.
- `A | B` permits either shape. Quoted strings are literal enum values.
- `CommandArgs` maps each command name to its payload. `undefined` means a unit
  command: pass the name as a string. Other commands use `(hash name payload)`.
- `CommandReplies` lists synchronous query results. Other commands return
  `json-null` or deliver a later event. Host errors raise Scheme errors.
- `EventPayloads` maps event names to payloads. Unit events are strings; other
  events are single-key hashes, such as `(hash "Timer" 0.0)`.

These definitions retain the host field names and tuple positions. The Rust wire
models remain authoritative when adding or changing a command.

`GetCellPixelSize` reports the acting client's terminal cell width and height in
pixels, or `null` until a valid report is available. It requires no permission.
`CellPixelSizeChanged` sends the current value when subscribed, then changes from
terminal reports and reconnects. Text-area reports carry the grid observed by
the reporting client, so a resize alone cannot reinterpret stale pixel data.
Shared plugins receive each report in the reporting client's context.
No default dimensions are invented.

`SaveLayout` and `RenameLayout` write the layout directory, whose `default.kdl`
the next plain `yinz` start loads without a prompt. Besides
`ChangeApplicationState`, they require `OpenFiles`, `RunCommands`, and
`OpenTerminalsOrPlugins`, the permissions that applying a layout file through
`OverrideLayout` or `SwitchSession` requires. `DeleteLayout` requires only
`ChangeApplicationState`.

```text
type Permission =
  | "ReadApplicationState" | "ChangeApplicationState" | "OpenFiles" | "RunCommands"
  | "OpenTerminalsOrPlugins" | "WriteToStdin" | "WebAccess" | "ReadCliPipes"
  | "MessageAndLaunchOtherPlugins" | "Reconfigure" | "FullHdAccess" | "InterceptInput"
  | "ReadPaneContents" | "RunActionsAsUser" | "WriteToClipboard"
  | "ReadSessionEnvironmentVariables" | "PluginData" | "SharedPluginData"
  | "ReadFiles" | "WriteFiles" | "AuthorPlugins";

type PermissionStatus = "Granted" | "Denied";

type PaneId = { Terminal: number } | { Plugin: number };

type PipeSource = { Cli: string } | { Plugin: number } | "Keybind";

interface EventPayloads {
  CellPixelSizeChanged: { width: Integer; height: Integer } | null;
  ModeUpdate: ModeInfo; TabUpdate: TabInfo[]; PaneUpdate: PaneManifest; Key: KeyWithModifier;
  Mouse: Mouse; Timer: number; CopyToClipboard: 'Command' | 'Primary' | 'System'; Visible: boolean; CustomMessage: [string, string];
  FileSystemCreate: [string, FileMetadata | null][]; FileSystemRead: [string, FileMetadata | null][];
  FileSystemUpdate: [string, FileMetadata | null][]; FileSystemDelete: [string, FileMetadata | null][];
  PermissionRequestResult: PermissionStatus; SessionUpdate: [SessionInfo[], [string, Duration][]];
  RunCommandResult: [number | null, number[], number[], StringMap]; WebRequestResult: [number, StringMap, number[], StringMap];
  RunCommandOutput: {data: Bytes; stderr: boolean; context: StringMap};
  PaneTabsUpdate: PaneTabsInfo[];
  PaneOutput: {pane_id: number; data: Bytes; context: StringMap};
  WebRequestStreamChunk: {stream_id: string; chunk: number[]; done: boolean; error: string | null};
  CommandPaneOpened: [number, StringMap]; CommandPaneExited: [number, number | null, StringMap]; PaneClosed: PaneId;
  EditPaneOpened: [number, StringMap]; EditPaneExited: [number, number | null, StringMap]; CommandPaneReRun: [number, StringMap];
  FailedToWriteConfigToDisk: {file_path: string | null; reason: string | null}; ListClients: ClientInfo[]; HostFolderChanged: string; FailedToChangeHostFolder: string | null;
  PastedText: string; InterceptedKeyPress: KeyWithModifier; UserAction: [Action, number, number | null, number | null];
  PaneRenderReport: Record<`terminal_${number}` | `plugin_${number}`, PaneContents>; ActionComplete: [Action, PaneId | null, StringMap];
  CwdChanged: [PaneId, string, number[]]; CommandChanged: [PaneId, string[], boolean, number[]]; AvailableLayoutInfo: [LayoutInfo[], LayoutWithError[]];
  PluginConfigurationChanged: StringMap; HighlightClicked: {pane_id: PaneId; pattern: string; matched_string: string; context: StringMap};
  NotificationReceived: NotificationData; NotificationRequested: NotificationData; NotificationResolved: NotificationResolution;
  ContextMenuRequested: ContextMenuContext; ContextMenuItemSelected: {label: string; item_context: StringMap; context: ContextMenuContext};
  InitialKeybinds: Keybinds; HostTerminalThemeChanged: 'Dark' | 'Light'; HintText: Record<string, {text: string; indices: Integer[][]}>;
  ActivePaneScroll: [Integer, Integer] | null; PluginDataChanged: {watch_id: Integer; rows: DataRows; truncated: boolean};
  QueueJobStarted: {queue: string; job_id: Integer; plugin_id: number; context: StringMap};
  QueueJobFinished: {queue: string; job_id: Integer; outcome: QueueJobOutcome; payload: string | null; context: StringMap};
  AgentProvidersChanged: AgentProvider[];
  PickResult: { request_id: string; result: { Ok: string | null } | { Err: string } };
  QueueChanged: QueueSnapshot; Hook: {point: HookPoint; payload: HookPayload}; AgentEvent: AgentEvent;
  ModifiersHeld: {modifiers: KeyModifier[]; is_live: boolean};
}

type UnitEvent = "SystemClipboardFailure" | "InputReceived" | "ConfigWasWrittenToDisk" | "BeforeClose";

type EventType = keyof EventPayloads | UnitEvent;

type Event = UnitEvent | { [K in keyof EventPayloads]: { [P in K]: EventPayloads[K] } }[keyof EventPayloads];

interface PluginIds {
  plugin_id: number; yinz_pid: number; initial_cwd: string; client_id: number;
  hostname: string; host_id: string;
  /** Maximum session-name bytes for the host's socket directory; absent on older hosts. */
  max_session_name_bytes?: number | null;
}

interface PipeMessage {
  source: PipeSource;
  name: string;
  payload: string | null;
  args: Record<string, string>;
  is_private: boolean;
  pane_id: PaneId | null;
}

type Integer = integer;

type Bytes = integer[];

type StringMap = Record<string, string>;

type Tagged<M> = { [K in keyof M]: { [P in K]: M[K] } }[keyof M];

interface CommandArgs {
  /** Pass a theme name to load its fields; null returns searchable theme names. */
  InspectConfiguration: string | null;
  EditConfiguration: ConfigurationEdit;
  TestPlugin: {name: string; options: PluginTestOptions; request_id: string};
  CancelPluginTest: string;
  CheckPlugin: {name: string; source: string; mode: PluginCheckMode};
  AgentControl: {id: string; request: string};
  Subscribe: EventType[]; Unsubscribe: EventType[]; SetSelectable: boolean;
  ShowCursor: [Integer, Integer] | null; GetPluginIds: undefined; GetCellPixelSize: undefined; GetPluginIdsForUrl: string; GetPluginInfo: number | null; Pick: PickCommand; GetPluginIdsForUrlWithConfiguration: { url: string; configuration: Record<string, string> }; GetYinzVersion: undefined;
  OpenFile: [FileToOpen, StringMap]; OpenFileFloating: [FileToOpen, FloatingPaneCoordinates | null, StringMap];
  OpenTerminal: FileToOpen; OpenTerminalFloating: [FileToOpen, FloatingPaneCoordinates | null];
  OpenCommandPane: [CommandToRun, StringMap]; OpenCommandPaneFloating: [CommandToRun, FloatingPaneCoordinates | null, StringMap];
  OpenCommandPaneOverlay: [CommandToRun, OverlayPlacement, StringMap];
  SwitchTabTo: number; SetTimeout: number; ExecCmd: string[];
  PostMessageTo: PluginMessage & {worker_name: string}; PostMessageToPlugin: PluginMessage;
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
  UndoRenameTab: undefined; QuitYinz: undefined; PreviousSwapLayout: undefined; NextSwapLayout: undefined;
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
  FsRead: {path: string; max_bytes: Integer | null; offset?: Integer | null}; FsList: {path: string}; FsMetadata: {path: string; follow_symlinks?: boolean};
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
  ArrangeProjectPanes: ProjectPaneGroup[]; SetTabProjects: Record<string, string>; SetProjectContext: [string[], [PaneId, string][]]; BreakPanesToTabWithId: [PaneId[], number, boolean];
  ReloadPlugin: number; LoadNewPlugin: {url: string; config: StringMap; load_in_background: boolean; skip_plugin_cache: boolean};
  RebindKeys: {keys_to_rebind: [InputMode, KeyWithModifier, Action[]][]; keys_to_unbind: [InputMode, KeyWithModifier][]; write_config_to_disk: boolean};
  RegisterAgentProvider: AgentProvider; GetAgentProviders: undefined;
  RegisterDefaultKeybindings: string; ListClients: undefined; ChangeHostFolder: string;
  SetFloatingPanePinned: [PaneId, boolean]; StackPanes: PaneId[]; ChangeFloatingPanesCoordinates: [PaneId, FloatingPaneCoordinates][];
  PaneTabs: PaneTabsCommand;
  CreateMultipane: CreateMultipane;
  UpdateMultipane: UpdateMultipane;
  ReplaceMultipaneChild: ReplaceMultipaneChild;
  ChangeOverlayCoordinates: {placements: [PaneId, FloatingPaneCoordinates][]; override_user_placement: boolean};
  TogglePaneBorderless: PaneId; SetPaneBorderless: [PaneId, boolean];
  OpenCommandPaneNearPlugin: [CommandToRun, StringMap]; OpenCommandPaneInGitCheckoutNearPlugin: [CommandToRun, string, GitCheckout];
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
  GetDisconnectTargets: undefined; GetSessionList: undefined; KillSessionsAndReply: string[]; DeleteDeadSessionAndReply: string; DeleteAllDeadSessionsAndReply: undefined;
  FocusHostSession: undefined; ListNotifications: undefined; ResolveNotification: {id: Integer; decision: NotificationDecision};
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
  SpawnAgent: {spec: AgentSpawnSpec; source_directory?: string | null; cwd?: string | null; name?: string | null; context?: StringMap; start_suppressed?: boolean};
  SetQuitNotice: {notice: string | null; guard: boolean}; ReloadPluginByUrl: string;
  ReadPluginLog: {name: string; lines: Integer}; GetLocalUtcOffset: {epoch_ms: Integer};
}

interface CommandReplies {
  CheckPlugin: PluginCheckReport;
  GetPluginIds: PluginIds; GetCellPixelSize: { width: Integer; height: Integer } | null; GetPluginIdsForUrl: number[]; GetPluginInfo: PluginInfo | null; Pick: { options: PickOptions; cwd: string } | null; GetPluginIdsForUrlWithConfiguration: number[]; GetYinzVersion: string; GetUserPluginDir: string | null; GetLayoutDir: string | null;
  GetLocalUtcOffset: number; GetSessionEnvironmentVariables: StringMap; GetPluginAliasConfiguration: StringMap;
  GetFocusedPaneInfo: {tab_id: number; pane_id: PaneId}; GetPaneInfo: PaneInfo | null; GetTabInfo: TabInfo | null;
  GetPaneCwd: string | null; GetPanePid: number | null; GetPaneRunningCommand: string[] | null; GetPaneGitProbe: PaneGitProbe | null;
  GetPaneScrollback: PaneContents; GetDisconnectTargets: number[]; GetSessionList: SessionListSnapshot; CurrentSessionLastSavedTime: Integer | null;
  SaveSession: boolean; DumpLayout: string; DumpSessionLayout: {layout: string; metadata: LayoutMetadata | null};
  ParseLayout: LayoutMetadata; SaveLayout: boolean; DeleteLayout: boolean; RenameLayout: boolean; EditLayout: boolean;
  ShowFloatingPanes: boolean; HideFloatingPanes: boolean; DeleteDeadSessionAndReply: boolean; DeleteAllDeadSessionsAndReply: boolean; KillSessionsAndReply: boolean;
  WarpPaneToSession: string; WarpPaneFromSession: string; WarpTabToSession: string; WarpTabFromSession: string; AdoptOrphanedPane: string;
  ListOrphanedPanes: OrphanedPane[]; DataDefineEntity: DataSchemaInfo; DataApply: ApplyReport; DataFetch: DataRows; DataCount: Integer;
  DataWatch: DataWatchHandle; DataUnwatch: boolean; DataCatalog: DataEntityInfo[]; EnqueueQueueJob: Integer; QueueStatus: QueueSnapshot[];
  ListNotifications: NotificationInfo[];
  NewTab: number | null; NewTabUnfocused: number | null; NewTabsWithLayout: number[]; NewTabsWithLayoutInfo: number[];
  FocusOrCreateTab: number | null; BreakPanesToNewTab: number | null; BreakPanesToTabWithIndex: number | null; BreakPanesToTabWithId: number | null;
  NewTiledPaneInTab: PaneId | null; OpenFile: PaneId | null; OpenFileFloating: PaneId | null; OpenFileInPlace: PaneId | null;
  OpenFileNearPlugin: PaneId | null; OpenFileFloatingNearPlugin: PaneId | null; OpenFileInPlaceOfPlugin: PaneId | null;
  OpenTerminal: PaneId | null; OpenTerminalFloating: PaneId | null; OpenTerminalInPlace: PaneId | null;
  OpenTerminalNearPlugin: PaneId | null; OpenTerminalFloatingNearPlugin: PaneId | null; OpenTerminalInPlaceOfPlugin: PaneId | null;
  OpenTerminalPaneInPlaceOfPaneId: PaneId | null; OpenEditPaneInPlaceOfPaneId: PaneId | null;
  OpenCommandPane: PaneId | null; OpenCommandPaneFloating: PaneId | null; OpenCommandPaneInPlace: PaneId | null; OpenCommandPaneBackground: PaneId | null;
  PaneTabs: PaneTabsInfo | null;
  CreateMultipane: MultipaneInfo; UpdateMultipane: MultipaneInfo; ReplaceMultipaneChild: MultipaneInfo;
  OpenCommandPaneNearPlugin: PaneId | null; OpenCommandPaneOverlay: PaneId | null; OpenCommandPaneFloatingNearPlugin: PaneId | null;
  OpenCommandPaneInPlaceOfPlugin: PaneId | null; OpenCommandPaneInPlaceOfPaneId: PaneId | null; OpenPluginPaneFloating: PaneId | null;
  RegisterAgentProvider: AgentProvider[]; GetAgentProviders: AgentProvider[];
  OpenCommandPaneInGitCheckoutNearPlugin: PaneId | null; SpawnAgent: SpawnedAgent;
  OpenCommandPaneInNewTab: PaneInNewTab; OpenPluginPaneInNewTab: PaneInNewTab; OpenEditorPaneInNewTab: PaneInNewTab;
  InspectConfiguration: ConfigurationSnapshot; EditConfiguration: ConfigurationSnapshot;
  ReadPluginLog: string; ProjectConfigRead: ProjectConfigSnapshot; ProjectConfigWrite: ProjectConfigSnapshot;
  FsRead: Bytes; FsList: FsEntry[]; FsMetadata: FileMetadata | null;
  FsWrite: boolean; FsRemove: boolean; FsCreateDir: boolean; FsRename: boolean;
  GenerateRandomName: string;
}

type Direction = 'Left' | 'Right' | 'Up' | 'Down';

type Resize = 'Increase' | 'Decrease';

type PaneFrameStyle = 'full' | 'titles' | 'none';

type InputMode = 'Normal' | 'Locked' | 'Resize' | 'Pane' | 'Tab' | 'Scroll' | 'EnterSearch' | 'Search' | 'RenameTab' | 'RenamePane' | 'Session' | 'Move' | 'Prompt' | 'Tmux' | 'Agent' | 'Select' | 'Notifications';

type KeyModifier = 'Ctrl' | 'Alt' | 'Shift' | 'Super';

type BareKey = 'PageDown' | 'PageUp' | 'Left' | 'Down' | 'Up' | 'Right' | 'Home' | 'End' | 'Backspace' | 'Delete' | 'Insert' | 'Tab' | 'Esc' | 'Enter' | 'CapsLock' | 'ScrollLock' | 'NumLock' | 'PrintScreen' | 'Pause' | 'Menu' | {F: number} | {Char: string};

interface KeyWithModifier {bare_key: BareKey; key_modifiers: KeyModifier[];}

type Mouse = Tagged<{ScrollUp: Integer; ScrollDown: Integer; ScrollLeft: Integer; ScrollRight: Integer; LeftClick: [Integer, Integer]; RightClick: [Integer, Integer]; Hold: [Integer, Integer]; Release: [Integer, Integer]; Hover: [Integer, Integer]}>;

type HttpVerb = 'Get' | 'Post' | 'Put' | 'Delete' | 'Patch';

type PercentOrFixed = {Percent: Integer} | {Fixed: Integer};

interface FloatingPaneCoordinates {x: PercentOrFixed | null; y: PercentOrFixed | null; width: PercentOrFixed | null; height: PercentOrFixed | null; pinned: boolean | null; borderless: boolean | null;}

interface OverlayPlacement {kind: 'Dismissible' | 'Modal'; coordinates?: FloatingPaneCoordinates | null; /** Below 100 opts into global background_opacity; the percentage does not override it. 100 or omitted means opaque. */ background_opacity?: number | null;}

interface ResizeStrategy {resize: Resize; direction: Direction | null; invert_on_boundaries: boolean;}

type PaneTarget = 'Local' | {Ssh: {host: string}} | {Bubblewrap: {root: string}};

interface CommandToRun {path?: string; args?: string[]; cwd?: string | null; target?: PaneTarget | null; remote_host?: string | null; remote_default_shell?: boolean;}

interface FileToOpen {path?: string; line_number?: Integer | null; cwd?: string | null;}

interface PluginMessage {name?: string; payload?: string; worker_name?: string | null;}

interface NotifyClick {message: string; payload: string;}

interface NewPluginArgs {should_float: boolean | null; pane_id_to_replace: PaneId | null; pane_title: string | null; cwd: string | null; skip_cache: boolean; should_focus: boolean | null;}

interface MessageToPlugin {plugin_url: string | null; destination_plugin_id: number | null; plugin_config: StringMap; message_name: string; message_payload: string | null; message_args: StringMap; new_plugin_args: NewPluginArgs | null; floating_pane_coordinates: FloatingPaneCoordinates | null;}

interface ConnectToSession {name: string | null; tab_position: number | null; pane_id?: [number, boolean] | null; layout: LayoutInfo | null; cwd: string | null;}

type GitCheckout = 'Current' | {NewBranch: {branch: string; base: string}} | {NewWorktree: {branch: string; base: string; path: string}};

interface ProjectPaneGroup {identity: string; name: string; configuration_root: string | null; pane_ids: number[];}

type LayoutInfo = {BuiltIn: string} | {File: [string, LayoutMetadata]} | {Stringified: string};

interface LayoutMetadata {tabs: TabMetadata[]; creation_time: string; update_time: string;}

interface TabMetadata {panes: PaneMetadata[]; name: string | null;}

interface PaneMetadata {name: string | null; is_plugin: boolean; is_builtin_plugin: boolean;}

interface LayoutDiagnostic {error_message: string; offset: Integer | null; len: Integer | null; help_message: string | null;}

type LayoutParsingError = 'SyntaxError' | {KdlError: {kdl_error: LayoutDiagnostic; file_name: string; source_code: string}};

interface LayoutWithError {layout_name: string; error: LayoutParsingError;}

interface FileMetadata {is_dir: boolean; is_file: boolean; is_symlink: boolean; len: Integer;}

interface FsEntry {path: string; metadata: FileMetadata | null;}

interface Position {line: Integer; column: Integer;}

interface CellCoordinates {x: Integer; y: Integer;}

interface PaneContents {lines_above_viewport: string[]; lines_below_viewport: string[]; viewport: string[]; selected_text: {start: Position; end: Position} | null; cursor: CellCoordinates | null; last_output_ms_ago: Integer | null; is_scrolled: boolean;}

type PaneTabsId = number;

interface PaneTabDescriptor { label: string; location: string; configuration: StringMap; initial_cwd?: string | null; }

type PaneTabLoadState = "Unloaded" | "Loading" | "PermissionPending" | "Ready" | { Failed: string };

interface PaneTabInfo { descriptor: PaneTabDescriptor; plugin_id: number | null; terminal_id?: number | null; state: PaneTabLoadState; }

interface PaneTabsInfo { id: PaneTabsId; pane_id: PaneId; tabs: PaneTabInfo[]; active: number; bar_focused: number[]; }

interface PaneTabsLayout { tabs: PaneTabDescriptor[]; active?: number; }

type PaneTabsCommand = { Mount: { pane_id: PaneId; layout: PaneTabsLayout; existing: number } }
  | { Select: { id: PaneTabsId; index: number } } | { Focus: { id: PaneTabsId; bar: boolean } }
  | { Retry: { id: PaneTabsId; index: number } } | { CloseChild: { id: PaneTabsId; index: number } }
  | { Close: { id: PaneTabsId } };

type MultipaneId = number;

interface MultipaneSplit { direction: "LeftRight" | "TopBottom"; ratio: number; }

type MultipaneChild = { Existing: PaneId } | { Terminal: CommandToRun } | { Plugin: { url: string; configuration: StringMap } };

interface MultipaneInfo { id: MultipaneId; children: [PaneId, PaneId]; x: number; y: number; rows: number; columns: number; split: MultipaneSplit; }

interface CreateMultipane { children: [MultipaneChild, MultipaneChild]; placement: OverlayPlacement; split: MultipaneSplit; focus?: 0 | 1 | null; /** Fewest cells either half keeps along the split; the halves share evenly when two do not fit. Default 0. */ min_cells?: number; }

interface UpdateMultipane { id: MultipaneId; coordinates?: FloatingPaneCoordinates | null; split?: MultipaneSplit | null; override_user_placement: boolean; /** Below 100 opts into global background_opacity; the percentage does not override it. 100 means opaque; omitted preserves eligibility. */ background_opacity?: number | null; }

interface ReplaceMultipaneChild { id: MultipaneId; slot: 0 | 1; child: MultipaneChild; }

interface PaneInfo {
  pane_tabs?: PaneTabsInfo | null;
  multipane?: MultipaneInfo | null;
  id: number; is_plugin: boolean; is_focused: boolean; is_fullscreen: boolean; is_floating: boolean; is_suppressed: boolean; is_stack_list_member: boolean;
  display_title?: string | null;
  title: string; exited: boolean; exit_status: number | null; is_held: boolean; restart_error: string | null;
  pane_x: Integer; pane_content_x: Integer; pane_y: Integer; pane_content_y: Integer;
  pane_rows: Integer; pane_content_rows: Integer; pane_columns: Integer; pane_content_columns: Integer;
  cursor_coordinates_in_pane: CellCoordinates | null; terminal_command: string | null; plugin_url: string | null;
  is_selectable: boolean; index_in_pane_group: Record<string, Integer>; default_fg: string | null; default_bg: string | null;
  cwd: string | null; host: string | null; target: PaneTarget; agent: AgentInfo | null;
}

interface TabInfo {
  position: number; name: string; active: boolean; panes_to_hide: Integer; is_fullscreen_active: boolean; is_sync_panes_active: boolean;
  are_floating_panes_visible: boolean; other_focused_clients: number[];
  // Each other focused client's multiplayer color slot (`player_<slot>`), 1 to 10, or null past
  // the tenth. Client ids are never reused and grow without bound, so color by slot, not by id.
  // Absent from an older host, whose ids are the slots.
  other_focused_client_slots?: (number | null)[];
  active_swap_layout_name: string | null; is_swap_layout_dirty: boolean;
  viewport_rows: Integer; viewport_columns: Integer; display_area_rows: Integer; display_area_columns: Integer;
  selectable_tiled_panes_count: Integer; selectable_floating_panes_count: Integer; tab_id: number; has_bell_notification: boolean; is_flashing_bell: boolean;
}

interface PaneManifest {panes: Record<string, PaneInfo[]>;}

interface Duration {secs: Integer; nanos: number;}

type Keybinds = [InputMode, [KeyWithModifier, Action[]][]][];

interface ModeInfo {
  mode: InputMode; base_mode: InputMode | null; keybinds: Keybinds; style: Style; capabilities: {arrow_fonts: boolean; kitty_graphics: boolean};
  session_name: string | null; editor: string | null; shell: string | null; currently_marking_pane_group: boolean | null;
  pane_frame_style: PaneFrameStyle | null; session_dimmed: boolean | null; session_ancestry: string[]; host_fullscreen: boolean | null;
  nested_ascend_keys: KeyWithModifier[]; session_ascended: boolean | null; nested_descend_keys: KeyWithModifier[];
}

interface SessionInfo {
  name: string; tabs: TabInfo[]; panes: PaneManifest; connected_clients: Integer; is_current_session: boolean;
  available_layouts: LayoutInfo[]; plugins: Record<string, {location: string; configuration: StringMap}>;
  tab_history: Record<string, number[]>; pane_history: Record<string, PaneId[]>; creation_time: Duration;
}

interface SessionListSnapshot {live_sessions: SessionInfo[]; resurrectable_sessions: [string, Duration][];}

interface PaneGitProbe {host: string; cwd: string; exit_code: number | null; metadata: number[]; config: number[];}

interface OrphanedPane {host_id: string; session_name: string | null; command: string; cwd: string | null; child_pid: number | null; started_at_unix: Integer; exit_status: number | null; buffered_bytes: Integer;}

interface ClientInfo {client_id: number; pane_id: PaneId; running_command: string; is_current_client: boolean;}

type NotificationSource = "Toast" | "Osc9" | "Osc99" | "Osc777";

type NotificationDelivery = "Toast" | "Desktop" | "Both";

interface NotificationData {id: Integer; received_at_unix_ms: Integer; source: NotificationSource; pane_id: PaneId | null; plugin_id: Integer | null; title: string; body: string; delivery: NotificationDelivery;}

type NotificationDecision = "Pass" | "Suppress" | {Replace: {title?: string | null; body?: string | null; delivery?: NotificationDelivery | null}};

interface NotificationResolution {original: NotificationData; title: string; body: string; delivery: NotificationDelivery; suppressed: boolean; fallback_reason: string | null;}

interface NotificationInfo {id: Integer; message: string; received_at_unix_ms: Integer; outcome?: NotificationResolution | null;}

interface PaneInNewTab {tab_id: number | null; pane_id: PaneId | null;}

type PaletteColor = {Rgb: [number, number, number]} | {EightBit: number};

interface StyleDeclaration {base: PaletteColor; background: PaletteColor; emphasis_0: PaletteColor; emphasis_1: PaletteColor; emphasis_2: PaletteColor; emphasis_3: PaletteColor;}

interface Style {colors: Styling; rounded_corners: boolean; hide_session_name: boolean;}

interface Styling {
  pane_surround: PaletteColor | null;
  text_unselected: StyleDeclaration; text_selected: StyleDeclaration; ribbon_unselected: StyleDeclaration; ribbon_selected: StyleDeclaration;
  table_title: StyleDeclaration; table_cell_unselected: StyleDeclaration; table_cell_selected: StyleDeclaration; list_unselected: StyleDeclaration; list_selected: StyleDeclaration;
  frame_unselected: StyleDeclaration | null; frame_selected: StyleDeclaration; frame_highlight: StyleDeclaration; exit_code_success: StyleDeclaration; exit_code_error: StyleDeclaration;
  multiplayer_user_colors: Record<`player_${1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10}`, PaletteColor>;
}

type HighlightStyle = 'None' | 'Emphasis0' | 'Emphasis1' | 'Emphasis2' | 'Emphasis3' | 'BackgroundEmphasis0' | 'BackgroundEmphasis1' | 'BackgroundEmphasis2' | 'BackgroundEmphasis3'
  | {CustomRgb: {fg: [number, number, number] | null; bg: [number, number, number] | null}} | {CustomIndex: {fg: number | null; bg: number | null}};

interface RegexHighlight {pattern: string; style: HighlightStyle; layer: 'Hint' | 'Tool' | 'ActionFeedback'; context: StringMap; on_hover: boolean; bold: boolean; italic: boolean; underline: boolean; tooltip_text: string | null;}

interface ContextMenuItem {id?: Integer | null; label?: string; shortcut?: string | null; actions?: Action[]; context?: StringMap; disabled?: boolean; separator?: boolean;}

interface ContextMenuContext {
  request_id: Integer; client_id: number; pane_id: PaneId; tab_position: number; click: [Integer, Integer]; click_in_pane: [Integer, Integer];
  on_frame: boolean; pane_title: string; pane_command: string | null; pane_exited: boolean; pane_exit_status: number | null; pane_is_held: boolean; pane_is_floating: boolean;
  selected_text: string | null; word_under_cursor: string | null; line_under_cursor: string | null;
  link: {plugin_id: number; pattern: string; matched: string; context: StringMap} | null; items: ContextMenuItem[];
}

type AgentState = 'Busy' | 'Waiting' | 'Idle' | 'Exited';

interface AgentInfo {kind: string; state: AgentState; source?: 'Spawned' | 'Registered' | 'Reported' | 'Detected'; detail?: string | null; waiting_on?: string | null; wait_needs_a_person?: boolean; cwd?: string | null; session_id?: string | null; run_id?: string | null;}

interface AgentSpawnSpec {
  bubblewrap?: boolean;
  kind?: string | null; command?: string | null; args?: string[]; model?: string | null; effort?: string | null; prompt?: string | null;
  branch?: string | null; base?: string | null; worktree?: boolean | null; worktree_dir?: string | null; env?: StringMap; permission_mode?: string | null;
  resume?: string | null; hooks?: boolean | null; reuse?: boolean | null; reuse_prompt?: string | null; allow_outside_worktree?: boolean | null;
}

interface SpawnedAgent {pane_id: PaneId; branch: string | null; cwd: string | null;}

interface AgentEvent {pane_id: number; kind: string; hook: string; session_id: string | null; state: AgentState | null; detail: string | null; prompt: string | null; last_message: string | null; tool_name: string | null; cwd: string | null; raw: string;}

type HookPoint = 'agent_exit' | 'agent_start' | 'agent_waiting' | 'agent_idle' | 'pane_exit' | 'session_start' | 'context_menu' | 'notification';

type HookPayload = {pane_id: number; agent: AgentInfo; previous_state: AgentState | null; command: string | null}
  | {pane_id: number; exit_code: number | null; cwd: string | null; command: string | null; held: boolean}
  | {session_name: string; client_id: number};

type WarpPlacement = 'NewTab' | 'FocusedTab' | 'Floating';

type QueueJobOutcome = 'Succeeded' | 'Failed' | 'Cancelled' | 'Aborted';

interface QueueJobInfo {job_id: Integer; queue: string; plugin_url: string; plugin_id: number | null; enqueuer_plugin_id: number; context: StringMap; state: 'Pending' | 'Running';}

interface QueueSnapshot {queue: string; concurrency: Integer; pending: QueueJobInfo[]; running: QueueJobInfo[];}

interface ProjectConfigSnapshot {root: string; source: string | null; config: ProjectConfig; error: string | null;}

interface ProjectConfig {default_branch: string | null;}

type DataValue = 'Null' | {Int: Integer} | {Real: number} | {Text: string} | {Bool: boolean} | {Blob: Bytes};

type AttrKind = 'Text' | 'Int' | 'Real' | 'Bool' | 'Blob' | 'Json';

interface AttrDef {name: string; kind: AttrKind; indexed: boolean; full_text: boolean; unique: boolean; required: boolean; default: DataValue | null;}

interface EntityDef {name: string; version: number; attrs: AttrDef[];}

type Pred = Tagged<{Eq: [string, DataValue]; Ne: [string, DataValue]; Gt: [string, DataValue]; Gte: [string, DataValue]; Lt: [string, DataValue]; Lte: [string, DataValue]; In: [string, DataValue[]]; IsNull: string; NotNull: string; Prefix: [string, string]; Contains: [string, string]; And: Pred[]; Or: Pred[]; Not: Pred}>;

interface FetchRequest {namespace: string | null; entity: string; filter: Pred | null; search: string | null; sort: [string, 'Asc' | 'Desc'][]; select: string[]; limit: number; offset: number; after_id: Integer | null;}

type Mutation = {Put: {entity: string; uid: string | null; values: [string, DataValue][]; expect_rev?: Integer | null}}
  | {Update: {entity: string; filter: Pred; values: [string, DataValue][]; expect_rev?: Integer | null}} | {Delete: {entity: string; filter: Pred}}
  | {AssertRows: {entity: string; filter: Pred; rows: [Integer, Integer][]}};

interface DataRows {columns: string[]; rows: DataValue[][]; truncated: boolean; next_cursor: Integer | null;}

interface DataSchemaInfo {namespace: string; entity: string; version: number; created: boolean; attrs_added: string[]; indexes_created: string[]; full_text_rebuilt: boolean;}

interface ApplyReport {rows_written: Integer; rows_deleted: Integer; ids: Integer[]; per_mutation: Integer[];}

type WatchMode = 'Rows' | 'Ids';

interface DataWatchHandle {watch_id: Integer; rows: DataRows;}

interface DataEntityInfo {namespace: string; owner_plugin: string; def: EntityDef;}

interface OriginatingPlugin {plugin_id: number; client_id: number; context: StringMap;}

interface RunCommandAction {
  cmd: string; args?: string[]; cwd?: string | null; direction?: Direction | null; hold_on_close?: boolean; hold_on_start?: boolean;
  originating_plugin?: OriginatingPlugin | null; use_terminal_title?: boolean; target?: PaneTarget; remote_default_shell?: boolean;
}

interface RunCommand extends Omit<RunCommandAction, 'cmd' | 'direction'> {command: string;}

type GitRevision = {Branch: string} | {Tag: string} | {Commit: string};

interface GitPlugin {repo: string; entrypoint: string; revision: GitRevision | null;}

type RunPluginLocation = {File: string} | {Yinz: string} | {Git: GitPlugin};

interface RunPlugin { _allow_exec_host_cmd?: boolean; location: RunPluginLocation; configuration: StringMap; initial_cwd: string | null; relative_path?: string | null; declared_path?: string | null;}

type RunPluginOrAlias = {RunPlugin: RunPlugin} | {Alias: {name: string; configuration: StringMap | null; initial_cwd: string | null; run_plugin: RunPlugin | null}};

type Run = {plugin: RunPluginOrAlias} | {command: RunCommand} | {EditFile: [string, Integer | null, string | null]} | {Cwd: string};

interface TiledPaneLayout {
  user_named?: boolean;
  children_split_direction: 'Horizontal' | 'Vertical'; name: string | null; children: TiledPaneLayout[]; split_size: PercentOrFixed | null;
  run: Run | null; borderless: boolean | null; focus: boolean | null; external_children_index: Integer | null;
  children_are_stacked: boolean; is_expanded_in_stack: boolean; exclude_from_sync: boolean | null; run_instructions_to_ignore: (Run | null)[];
  hide_floating_panes: boolean; pane_initial_contents: string | null; default_fg: string | null; default_bg: string | null;
}

interface FloatingPaneLayout extends FloatingPaneCoordinates {
  name: string | null; run: Run | null; focus: boolean | null; already_running: boolean; pane_initial_contents: string | null;
  logical_position: Integer | null; default_fg: string | null; default_bg: string | null;
}

type SwapTiledLayout = [Partial<Record<'NoConstraint', TiledPaneLayout>>, string | null];

type SwapFloatingLayout = [Partial<Record<'NoConstraint', FloatingPaneLayout[]>>, string | null];

interface TabLayoutInfo {tab_index: number; tab_name: string | null; tiled_layout: TiledPaneLayout; floating_layouts: FloatingPaneLayout[]; swap_tiled_layouts: SwapTiledLayout[] | null; swap_floating_layouts: SwapFloatingLayout[] | null;}

type CommandOrPlugin = {Command: RunCommandAction} | {Plugin: RunPluginOrAlias} | {File: FileToOpen};

type UnblockCondition = 'OnExitSuccess' | 'OnExitFailure' | 'OnAnyExit';

type NewPanePlacement = {NoPreference: {borderless: boolean | null}} | {Tiled: {direction: Direction | null; borderless: boolean | null}}
  | {Floating: FloatingPaneCoordinates | null} | {InPlace: {pane_id_to_replace: PaneId | null; close_replaced_pane: boolean; borderless: boolean | null}}
  | {Stacked: {pane_id_to_stack_under: PaneId | null; borderless: boolean | null}} | {Overlay: OverlayPlacement};

interface OpenFilePayload {path: string; line_number: Integer | null; cwd: string | null; originating_plugin: OriginatingPlugin | null;}

interface MouseEvent {event_type: 'Press' | 'Release' | 'Motion'; left: boolean; right: boolean; middle: boolean; wheel_up: boolean; wheel_down: boolean; wheel_left?: boolean; wheel_right?: boolean; shift: boolean; alt: boolean; ctrl: boolean; position: Position;}

interface HeldTerminal {command: RunCommand; first_run: boolean; exit_status: number | null;}

interface WarpedPane {held?: HeldTerminal | null;host_id: string; control_socket: string; title: string | null; contents: string | null; borderless: boolean; remote: {host: string; pane_token: string; received: Integer; cmd: RunCommand} | null; agent?: AgentInfo | null;}

interface WarpedTab {name: string | null; tiled_layout: TiledPaneLayout; floating_layout: FloatingPaneLayout[]; tiled_panes: WarpedPane[]; floating_panes: WarpedPane[]; hide_floating_panes: boolean;}

interface ActionPane {pane_id: PaneId;}

interface ActionTab {id: number;}

interface PaneLaunch {pane_name: string | null; near_current_pane: boolean; no_focus: boolean; tab_id: number | null;}

interface CommandPaneLaunch extends PaneLaunch {command: RunCommandAction | null;}

interface PluginPaneLaunch {plugin: RunPluginOrAlias; pane_name: string | null; skip_cache: boolean; no_focus: boolean; tab_id: number | null;}

interface ActionPipe {name: string | null; payload: string | null; args: StringMap | null; plugin: string | null; configuration: StringMap | null; launch_new: boolean; skip_cache: boolean; floating: boolean | null; in_place: boolean | null; cwd: string | null; pane_title: string | null;}

interface ActionPayloads {
  Write: {key_with_modifier: KeyWithModifier | null; bytes: Bytes; is_kitty_keyboard_protocol: boolean}; WriteChars: {chars: string};
  WriteToPaneId: ActionPane & {bytes: Bytes}; WriteCharsToPaneId: ActionPane & {chars: string}; Paste: {chars: string; pane_id: PaneId | null};
  SwitchToMode: {input_mode: InputMode}; SwitchModeForAllClients: {input_mode: InputMode}; Resize: {resize: Resize; direction: Direction | null};
  MoveFocus: {direction: Direction}; MoveFocusOrTab: {direction: Direction}; MovePane: {direction: Direction | null};
  DumpScreen: {file_path: string | null; include_scrollback: boolean; pane_id: PaneId | null; ansi: boolean}; EditScrollback: {ansi: boolean};
  ScrollUpAt: {position: Position}; ScrollDownAt: {position: Position}; MoveTextSelection: {direction: Direction}; MoveNotificationSelection: {direction: Direction}; SetPaneFrameStyle: PaneFrameStyle;
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
  Reconfigure: {config: string; write_to_disk: boolean}; Notify: string; CliPipe: ActionPipe & {pipe_id: string; focus_plugin?: boolean}; KeybindPipe: ActionPipe & {plugin_id: number | null};
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

type UnitAction = 'FocusPaneTabs' | 'PreviousPaneTab' | 'NextPaneTab' | 'Quit' | 'FocusNextPane' | 'FocusPreviousPane' | 'FocusLastPane' | 'SwitchFocus' | 'MovePaneBackwards' | 'ClearScreen'
  | 'DumpLayout' | 'SaveSession' | 'ScrollUp' | 'ScrollDown' | 'ScrollToBottom' | 'ScrollToTop' | 'ScrollToPreviousPrompt' | 'ScrollToNextPrompt'
  | 'SelectCommandAtScrollPosition' | 'CopyLastCommandOutput' | 'SwapTextSelectionEnds' | 'ActivateNotification' | 'ActivateLastNotification' | 'ToggleNotificationDetail' | 'PageScrollUp' | 'PageScrollDown' | 'HalfPageScrollUp' | 'HalfPageScrollDown'
  | 'ToggleFocusFullscreen' | 'ToggleFocusNoUiFullscreen' | 'TogglePaneFrames' | 'ToggleCommandPalette' | 'ToggleActiveSyncTab' | 'TogglePaneEmbedOrFloating' | 'ToggleFloatingPanes'
  | 'CloseFocus' | 'UndoRenamePane' | 'NoOp' | 'GoToNextTab' | 'GoToPreviousTab' | 'CloseTab' | 'ToggleTab' | 'UndoRenameTab' | 'Detach'
  | 'SetDarkTheme' | 'SetLightTheme' | 'ToggleTheme' | 'Copy' | 'Confirm' | 'Deny' | 'ToggleMouseMode' | 'PreviousSwapLayout' | 'NextSwapLayout' | 'QueryTabNames'
  | 'ArrangePanesByProject' | 'BreakPane' | 'BreakPaneRight' | 'BreakPaneLeft' | 'FocusHostSession' | 'FocusGuestSession' | 'ToggleHostFullscreen'
  | 'ListClients' | 'TogglePanePinned' | 'TogglePaneInGroup' | 'ToggleGroupMarking';

type Action = UnitAction | Tagged<ActionPayloads>;

type PluginCheckMode = 'Syntax' | 'Semantic' | 'Format' | 'Prepare';

interface OutlineEntry {line: Integer; kind: 'Require' | 'Permission' | 'Function' | 'Handler' | 'Define'; text: string}

interface PluginCheckReport {diagnostics: string[]; outline: OutlineEntry[]; permissions: Permission[]; warnings: string[]; formatted_source?: string}

interface PluginTestOptions {kind?: "unit" | "integration" | "all"; filter?: string; timeout_ms?: number}

interface AgentProvider {
  kind: string; executable: string; aliases?: string[]; launch?: AgentLaunch; restart?: AgentRestart | null;
  input?: AgentInput | null; hooks?: AgentHookArtifact[];
}

interface AgentLaunch {
  args?: string[]; model_args?: string[]; prompt_args?: string[]; resume_args?: string[];
  resume_last_args?: string[]; resume_before_args?: boolean; permissions?: Record<string,string[]>; permission_descriptions?: Record<string,string>; efforts?: Record<string,string[]>;
}

interface AgentRestart {
  values?: string[]; switches?: string[]; inline_values?: string[]; forbidden_commands?: string[];
  remove_values?: string[]; remove_switches?: string[]; remove_optional_values?: string[]; keep_positionals?: boolean; max_positionals?: number;
}

interface AgentInput {
  /** Whole-composer clear key used before explicit prompts. Defaults to "Ctrl c". */
  clear_key?: string;
  submit_key?: string; submit_delay_ms?: number; paste_prefix?: string; command_prefixes?: string[]; newline?: "\n" | "\r" | "\r\n";
}

interface AgentHookArtifact {base?: "config" | "home"; path: string; content: string; marker: string; executable?: boolean;}

interface PluginInfo { pluginId: number; background: boolean; }

interface PickOptions {
  kind: "directory" | "file";
  roots?: string[];
  extensions?: string[];
  hidden?: boolean;
  recursive?: boolean;
  timeoutMs?: number;
}

type PickCommand = { Start: { request_id: string; options: PickOptions } } | { Cancel: { request_id: string; timeout: boolean } } | "Request" | { Attach: { pane_id: number } } | { Complete: { result: { Ok: string | null } | { Err: string } } };

interface ConfigurationNode {name: string; arguments: (string | number | boolean)[]; properties: Record<string, string | number | boolean>; children: ConfigurationNode[];}

interface ConfigurationPatch {reset?: boolean; path: string[]; node: ConfigurationNode | null;}

interface ConfigurationEdit {revision: string; destination: "session" | "permanent"; patches: ConfigurationPatch[]; reset: string[][];}

interface ConfigurationSetting {name: string; label: string; kind: string; choices: string[]; minimum: number | null; maximum: number | null; help: string; timing: string;}

interface ConfigurationSnapshot {action_children: Record<string, ConfigurationNode[]>; action_properties: Record<string, Record<string, string | number | boolean>>; section_timing: Record<string, string>; sections: string[]; templates: Record<string, ConfigurationNode[]>; actions: ConfigurationNode[]; revision: string; catalog: ConfigurationSetting[]; saved: ConfigurationNode[]; clients: Record<string, ConfigurationNode[]>; overrides: ConfigurationPatch[]; notice: string;}
```
