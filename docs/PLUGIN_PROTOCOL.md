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
  ClientTimezoneChanged: { client_id: number; timezone: string | null };
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
  AgentProvidersChanged: AgentProvider[];
  PickResult: { request_id: string; result: { Ok: string | null } | { Err: string } };
  Hook: {point: HookPoint; payload: HookPayload}; AgentEvent: AgentEvent;
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
  /** Steel-only web interface commands; see "Web interface commands". */
  HttpRespond: {request_id: number; response: string};
  HttpAssets: {prefix: string; directory: string};
  HttpPublish: {path: string; content_type: string; body: string};
  AgentView: {id: string; request: string};
  Subscribe: EventType[]; Unsubscribe: EventType[]; SetSelectable: boolean;
  ShowCursor: [Integer, Integer] | null; GetPluginIds: undefined; GetCellPixelSize: undefined; GetClientTimezone: {client_id?: number}; GetPluginIdsForUrl: string; GetPluginInfo: number | null; Pick: PickCommand; GetPluginIdsForUrlWithConfiguration: { url: string; configuration: Record<string, string> }; GetYinzVersion: undefined;
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
  RestartAgentPane: {pane_id: PaneId; prompt?: string | null; session: "resume" | "fresh"; context?: StringMap};
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
  ArrangeProjectPanes: ProjectPaneGroup[]; SetTabProjects: Record<string, string>; SetProjectContext: [string[], [PaneId, string][], Record<string, string>?]; BreakPanesToTabWithId: [PaneId[], number, boolean];
  ReloadPlugin: number; LoadNewPlugin: {url: string; config: StringMap; load_in_background: boolean; skip_plugin_cache: boolean; cwd?: string | null};
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
  GroupAndUngroupPanes: [PaneId[], PaneId[], boolean]; HighlightAndUnhighlightPanes: [PaneId[], PaneId[]];
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
  OpenPluginPaneInPlaceOfPaneId: {pane_id: PaneId; plugin_url: string; configuration?: StringMap; context?: StringMap};
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
  SetPaneAgent: {pane_id: PaneId; agent: AgentInfo | null}; SetPaneAgentRun: {pane_id: PaneId; run_id: string | null};
  ResolveProject: {directory: string};
  ProjectConfigRead: {directory: string}; ProjectConfigWrite: {directory: string; source: string; expected: string | null};
  SpawnAgent: {spec: AgentSpawnSpec; source_directory?: string | null; cwd?: string | null; name?: string | null; context?: StringMap; start_suppressed?: boolean};
  SetQuitNotice: {notice: string | null; guard: boolean}; ReloadPluginByUrl: string;
  ReadPluginLog: {name: string; lines: Integer}; GetLocalUtcOffset: {epoch_ms: Integer; client_id?: number; timezone?: string}; GetLocalTime: {epoch_ms: Integer};
}

interface CommandReplies {
  CheckPlugin: PluginCheckReport;
  GetPluginIds: PluginIds; GetCellPixelSize: { width: Integer; height: Integer } | null; GetClientTimezone: string | null; GetPluginIdsForUrl: number[]; GetPluginInfo: PluginInfo | null; Pick: { options: PickOptions; cwd: string } | null; GetPluginIdsForUrlWithConfiguration: number[]; GetYinzVersion: string; GetUserPluginDir: string | null; GetLayoutDir: string | null;
  GetLocalUtcOffset: number | null; GetLocalTime: string; GetSessionEnvironmentVariables: StringMap; GetPluginAliasConfiguration: StringMap;
  GetFocusedPaneInfo: {tab_id: number; pane_id: PaneId}; GetPaneInfo: PaneInfo | null; GetTabInfo: TabInfo | null;
  GetPaneCwd: string | null; GetPanePid: number | null; GetPaneRunningCommand: string[] | null; GetPaneGitProbe: PaneGitProbe | null;
  GetPaneScrollback: PaneContents; GetDisconnectTargets: number[]; GetSessionList: SessionListSnapshot; CurrentSessionLastSavedTime: Integer | null;
  SaveSession: boolean; DumpLayout: string; DumpSessionLayout: {layout: string; metadata: LayoutMetadata | null};
  ParseLayout: LayoutMetadata; SaveLayout: boolean; DeleteLayout: boolean; RenameLayout: boolean; EditLayout: boolean;
  ShowFloatingPanes: boolean; HideFloatingPanes: boolean; DeleteDeadSessionAndReply: boolean; DeleteAllDeadSessionsAndReply: boolean; KillSessionsAndReply: boolean;
  WarpPaneToSession: string; WarpPaneFromSession: string; WarpTabToSession: string; WarpTabFromSession: string; AdoptOrphanedPane: string;
  ListOrphanedPanes: OrphanedPane[]; DataDefineEntity: DataSchemaInfo; DataApply: ApplyReport; DataFetch: DataRows; DataCount: Integer;
  DataWatch: DataWatchHandle; DataUnwatch: boolean; DataCatalog: DataEntityInfo[];
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
  OpenPluginPaneInPlaceOfPaneId: PaneId;
  RegisterAgentProvider: AgentProvider[]; GetAgentProviders: AgentProvider[];
  OpenCommandPaneInGitCheckoutNearPlugin: PaneId | null; SpawnAgent: SpawnedAgent;
  RestartAgentPane: {pane_id: PaneId; session_id: string | null; command: string[]};
  OpenCommandPaneInNewTab: PaneInNewTab; OpenPluginPaneInNewTab: PaneInNewTab; OpenEditorPaneInNewTab: PaneInNewTab;
  InspectConfiguration: ConfigurationSnapshot; EditConfiguration: ConfigurationSnapshot;
  ResolveProject: ResolvedProject | null;
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

interface ResolvedProject {identity: string; name: string; root: string;}
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
  last_agent_session_id?: string | null;
  pane_x: Integer; pane_content_x: Integer; pane_y: Integer; pane_content_y: Integer;
  pane_rows: Integer; pane_content_rows: Integer; pane_columns: Integer; pane_content_columns: Integer;
  cursor_coordinates_in_pane: CellCoordinates | null; terminal_command: string | null; plugin_url: string | null;
  is_selectable: boolean; index_in_pane_group: Record<string, Integer>; default_fg: string | null; default_bg: string | null;
  cwd: string | null; host: string | null; target: PaneTarget; agent: AgentInfo | null;
}

interface TabGroupInfo {id: number; name: string;}
interface TabInfo {
  group?: TabGroupInfo | null;
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

interface ClientInfo {client_id: number; pane_id: PaneId; running_command: string; is_current_client: boolean; timezone: string | null;}

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
  | {session_name: string; client_id: number}
  | AgentExitHookPayload;

// agent_exit re-entry uses the same string values as the initial configuration.
interface AgentExitHookPayload {
  agent_pane_id: string; agent_pane_title?: string; agent_exit_code?: string;
  agent_cwd: string; agent_command: string; agent_kind?: string; agent_target: string;
  agent_session_id?: string;
}

type WarpPlacement = 'NewTab' | 'FocusedTab' | 'Floating';




interface ProjectConfigSnapshot {root: string; source: string | null; config: ProjectConfig; error: string | null;}

// Plugin-owned nodes exclude agent_exit. See PLUGIN_SDK.md, "Per-project configuration".
// Duplicate top-level names are errors. A sections-only file retains default_branch = "main".
// Reads and writes return the same shape; source preserves the original KDL text.
interface ProjectConfig {default_branch: string | null; sections: Record<string, ConfigurationNode>;}

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
  tab_group?: TabGroupInfo | null; group_focused?: boolean;
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
  GoToTabInGroup: {index: number}; SwitchTabGroup: {id: number};
  RenameTabGroup: {id: number; name: string}; CloseTabGroup: {id: number};
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

type UnitAction = 'NewTabGroup' | 'GoToNextTabGroup' | 'GoToPreviousTabGroup' | 'MovePaneToPreviousTabGroup' | 'MovePaneToNextTabGroup' | 'FocusPaneTabs' | 'PreviousPaneTab' | 'NextPaneTab' | 'Quit' | 'FocusNextPane' | 'FocusPreviousPane' | 'FocusLastPane' | 'SwitchFocus' | 'MovePaneBackwards' | 'ClearScreen'
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

/** pane: "always", "unless KEY=VALUE", or "none"; omitted when the check fails. */
interface PluginCheckReport {diagnostics: string[]; outline: OutlineEntry[]; permissions: Permission[]; warnings: string[]; formatted_source?: string; pane?: string}

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

/** pane: null while the declaration is unknown, such as during a clone. */
interface PluginInfo { pluginId: number; background: boolean; pane: boolean | null; }

interface PickOptions {
  kind: "directory" | "file";
  roots?: string[];
  extensions?: string[];
  hidden?: boolean;
  recursive?: boolean;
  timeoutMs?: number;
}

type PickCommand = { Start: { request_id: string; options: PickOptions } } | { Cancel: { request_id: string; timeout: boolean } } | "Request" | { Attach: { pane_id: number } } | { Complete: { result: { Ok: string | null } | { Err: string } } };

interface ConfigurationNode {name: string; arguments: (string | number | boolean | null)[]; properties: Record<string, string | number | boolean | null>; children: ConfigurationNode[];}

interface ConfigurationPatch {reset?: boolean; path: string[]; node: ConfigurationNode | null;}

interface ConfigurationEdit {revision: string; destination: "session" | "permanent"; patches: ConfigurationPatch[]; reset: string[][];}

interface ConfigurationSetting {name: string; label: string; kind: string; choices: string[]; minimum: number | null; maximum: number | null; help: string; timing: string;}

interface ConfigurationSnapshot {action_children: Record<string, ConfigurationNode[]>; action_properties: Record<string, Record<string, string | number | boolean>>; section_timing: Record<string, string>; sections: string[]; templates: Record<string, ConfigurationNode[]>; actions: ConfigurationNode[]; revision: string; catalog: ConfigurationSetting[]; saved: ConfigurationNode[]; clients: Record<string, ConfigurationNode[]>; overrides: ConfigurationPatch[]; notice: string;}
```

`SetProjectContext` replaces the registered project names, pane assignments, and
optional map of project names to saved sources. Omitted sources default to an empty
map. The project picker shows each source as secondary literal text, with `Current`
for the selected project. The project store prefers a nonblank saved Git remote,
falling back to the root folder, and preserves the saved spelling.

`LoadNewPlugin.cwd` sets the base directory for relative file URLs and the new
plugin's default working directory in either placement. An explicitly configured
alias directory takes precedence. If `cwd` is omitted or null, relative file URLs
and foreground plugins use the calling plugin's directory; background plugins
keep the session's default directory unless the alias specifies one.

## Headless plugins

A plugin without a [pane declaration](PLUGIN_SDK.md#pane-declaration) has no pane.
Commands that need the calling plugin's own pane are refused with a Scheme error:
`'<command>' needs the plugin's own pane, but <plugin> declares no pane; add
(yinz-declare-pane) at the top level of its entry file`. The SDK reference lists them.
Commands that open a headless plugin succeed with `{"Plugin": id}`. That ID addresses
the plugin for `MessageToPlugin`. Focusing it has no effect, and closing it unloads
the plugin. `plugins list --running --json` reports `pane` with the same meaning as
`PluginInfo.pane`.

## Pane slots

A pane opened in place of another takes that pane's slot. The other pane is
closed, or suppressed behind the new pane until the new pane leaves the slot.
`ReplacePaneWithExistingPane`, and the `…InPlaceOfPlugin` and `…InPlaceOfPaneId`
commands, require a target that holds a slot: a tiled, floating, or overlay pane.
A suppressed or missing target, or one that would be replaced by itself, is
refused with a Scheme error. Nothing is moved, and a pane spawned for the request
is closed instead of left running unseen. The commands that open in place of the
calling plugin fail the same way while the plugin is hidden.

With `close_replaced_pane` false, the `…InPlaceOfPlugin` and `…InPlaceOfPaneId`
commands *lend* the slot. Every close of the new pane, including an explicit one,
restores the pane behind it. This includes Ctrl-C on a held command,
`CloseTerminalPane`, and the user's close-pane key. For example, an `agent_exit`
hook `P` over its agent `A` can run a CI gate `C` in its own slot:

```scheme
(yinz-command (hash "OpenCommandPaneInPlaceOfPlugin"
  (list (hash "path" "make" "args" '("check")) #f (hash "job" "42"))))
```

The stack is `A <- P <- C`. When the user presses Ctrl-C on held `C`, `P`
returns to the slot, and `A` remains suppressed behind `P`. On `CommandPaneExited`,
the plugin can close `C` with `CloseTerminalPane` to take its slot back. It can
also use `ReplacePaneWithExistingPane` with `[C, P, false]`.

Other suppressions are one unit. These include an `agent_exit` hook over its pane,
the scrollback editor over its terminal, and panes opened in place of the focused
pane, such as `OpenCommandPaneInPlace` or a user's in-place pane. An explicit
close of either pane closes both. A natural exit or `CloseSelf` still restores the
pane behind. An explicit close also closes every pane in front of the closed pane,
whether that pane was lent or not. To close a whole stack, close its bottom pane.
For example, `CloseTerminalPane` on `A` closes `A`, `P`, and `C`. A slot lent
before a session warp arrives as an ordinary suppression.

`OpenPluginPaneInPlaceOfPaneId` requires `OpenTerminalsOrPlugins`, the permission
for opening plugin panes and for the other `…InPlaceOfPaneId` commands.
It loads `plugin_url` with `configuration` in place of `pane_id` and lends it
the slot. The command is refused when the
target holds no slot, before anything loads. It is also refused when an instance
with that URL and configuration is running or loading. In that case, focus or close
the existing instance, or use a distinct configuration. The reply is the new
plugin pane's id after the pane is placed. `context` is reserved; plugin panes do
not echo it. A plugin that declares no pane runs headless: the target keeps its
slot, and the reply is the plugin's id.

## Data watch delivery

`DataWatch` returns the initial query result and registers for `PluginDataChanged`
events. Watches include commits from every local server using the same
`plugin-data.db`. Namespace permissions and query filters also apply to these
external changes. No plugin command or event format changes are required.

The host checks SQLite's connection-local `data_version` every 250 ms while at
least one watch exists. A changed version triggers query evaluation and comparison
with the last delivered results. Query execution and event delivery can add
latency. Polling stops when the last watch is removed or its plugin unloads.

Events contain complete query results, including selection, ordering, limits,
and truncation. Several writes can coalesce into one event per watch. Intermediate
states, including a change followed by its reversal, can be omitted. `Ids` watches
only report changes to their selected IDs and result metadata. Unchanged results
produce no event. Local and external refreshes share the same comparison state,
so the poller does not duplicate a local notification.

Events for each watch follow refresh order. There is no global transaction order
across watches or servers. These events are query updates, not a durable change
log. A plugin that reloads must register its watches again.

## Restart an exited agent

`RestartAgentPane` requires `RunCommands`. The target must be an exited, held
terminal agent pane. It may be suppressed behind an `agent_exit` plugin.
The host cleans the structured held command with the normal restart grammar,
keeps launch flags, cwd and target, then applies the provider's session and
opening prompt arguments. Unknown or ambiguous restart arguments return the
same refusal as `PaneInfo.restart_error`. Resume requires a remembered session
id. Providers that cannot resume with a prompt return an error. Prompts must
fit in 1 MiB and contain no NUL bytes.

The reply contains the pane id (`{"Terminal":7}`), selected session id
(null for fresh), and executable plus argv. It confirms that the screen accepted
the restart for PTY spawning. It does not wait for provider startup. Inspect the
pane before retrying after a transport timeout. `context` is echoed in
`CommandPaneExited` to the calling plugin. Close the hook plugin with `CloseSelf`
after success to reveal the running agent.

The pane remains an agent pane and fires `agent_exit` again on its next exit.
If its hook instance is still loaded, it receives `Hook {point: "agent_exit", payload}`.
The payload contains the new firing's configuration keys and string values, including
optional fields only when available. Identical payloads also produce an event.
The first firing still supplies these values through the plugin's configuration.

The host records the hook point and terminal pane ID when it launches the instance.
While the hook is enabled, it uses that association for re-entry even if the payload or binding changes.
It queues events while the instance loads and removes the association before unload cleanup.
If the instance is gone or unavailable, the next firing launches the current binding.
`Hook` requires a subscription. It needs no extra permission because the user's hook
binding authorizes delivery to that plugin alone. It is never broadcast to other plugins.

`agent_exit` configuration includes `agent_session_id` when known, even after
`SessionEnd` cleared the live record. `PaneInfo.last_agent_session_id` carries
that same memory. A fresh restart clears it until a new session reports itself.
`RunCommandInPane` also clears the previous session id, even when the command
and arguments are unchanged. Use `RestartAgentPane` with `session: "resume"`
to retain the selected session. A report from a different provider clears the
previous provider's id before recording its own. Reusing the pane for a
non-agent command removes the remembered id and disarms `agent_exit`.

`AgentControl` also accepts `{"Restart":{"pane_id":{"Terminal":7},
"prompt":"P","session":"resume"}}`. Its ordinary `AgentResponse` has a
`restarted` object with the same reply fields on success, or outcome `rejected`
and a message on error. This direct control request retains the held command's
original plugin routing. Use `RestartAgentPane` to receive exit context in the
calling hook plugin.

`AgentControl` accepts `{"Usage":{"provider":"codex"}}` or provider `"claude"`
to read the local provider account's subscription quotas. It requires
`RunCommands` and runs outside the plugin executor. The response has outcome
`ok` and a `usage` object, including when quota data is unavailable:

```json
{
  "provider": "codex",
  "status": "ok",
  "observed_at_ms": 1793000000000,
  "plan": "plus",
  "windows": [{
    "label": "5 hours",
    "used_percent": 42.5,
    "resets_at_ms": 1793003600000
  }],
  "message": null
}
```

`status` is `ok`, `unavailable`, `unsupported`, `expired`, or `error`. The
observation time, plan, reset time, and message can be null. Percentages can
exceed 100. Unknown quotas have no windows; an empty list does not mean zero
usage. Errors contain safe diagnostics, never credentials or raw provider
responses. This request has no pane target: the server's provider environment
selects the local account. Clients must not attribute these quotas to an SSH
agent or a pane with different credentials. The Agents panel caches per
provider and preserves the original observation time when a refresh fails.
The server also retains successful observations across callers: an `error`
response may carry the previous windows, plan, and original observation time.
These values are stale. Claude observations are cached for five minutes, and
rate-limited checks back off from 15 minutes to one hour while honoring longer
`Retry-After` delays.

## Web interface commands

Four commands serve the mobile web interface. They are Steel-only: the
protobuf wire refuses them with "Web commands are Steel-only". Each requires
`RunCommands`.

- `HttpRespond {request_id, response}` answers one web request delivered to
  this instance. `response` is JSON:
  `{"status":200,"headers":{"content-type":"…"},"body":"…"}`. An unknown or
  already answered `request_id` is ignored.
- `HttpAssets {prefix, directory}` freezes a directory of static assets
  (plugin-relative, or `builtin:web/static`) under a URL prefix such as
  `/static/`.
- `HttpPublish {path, content_type, body}` publishes a public, pre-rendered
  page such as `/` or `/login`.
- `AgentView {id, request}` runs a native agent-view operation. The reply
  arrives as `CustomMessage("agent-view-response", {"id", "reply"})`, where
  `reply` is `{"ok": …}` or `{"error": {"code", "message"}}`.

`HttpAssets`, `HttpPublish` and `AgentView` are reserved for the configured
web plugin (`web { plugin … }`) and builtin plugins; any other plugin gets the
error "reserved for the configured web plugin".

### Native agent views

`AgentView.request` is a JSON object of at most 64 KiB. Subscribe to
`CustomMessage` and correlate `agent-view-response` by the caller's `id`.
Only the configured web plugin and builtins with `RunCommands` may call it.
Use the existing `shared true` plugin configuration for a session-lifetime web
handler that survives terminal detach. Operations reject unknown fields. No native action enum is accepted from a guest.

| `op` | Fields | `ok` payload |
| --- | --- | --- |
| `list` | none | `{session, now_ms, counts:{waiting,busy,idle}, agents:[row]}` |
| `history` | `run`, optional `agent` (live ID), `query` (default empty), `from`, `worker`, `before`, `after_revision`, `tz` | History page below |
| `hit` | `run`, `revision`, optional `tz` | History page with context/latest navigation |
| `event` | `run`, `revision`, optional `tz` | `{header:string, markdown:string, tool:string}` |
| `tools` | `run`, either `revisions` (1–30) or `range` with optional `before`, optional `tz` | Reader; range mode also includes `page` |
| `usage` | `agent` (live ID) | `{usage:ProviderUsage}` |
| `prompts` | none | `{prompts:[{text,description}]}` (at most 500) |
| `prompt` | `agent:AgentRef`, `submission_id`, `text` | `{response:AgentResponse}` |
| `screen` | `agent:AgentRef`, `lines` (1–200, default 80) | `{response:AgentResponse}` |
| `keys` | `agent:AgentRef`, `keys` (1–8) | `{response:AgentResponse}` |
| `focus` | `agent:AgentRef` | `true` |

`AgentRef` is `{id, kind, session_id}`. All identity strings must be nonblank
and contain no control characters. Actions take a fresh snapshot and require the
complete identity. Only peers represented in that snapshot can be routed.
Prompt, screen and keys also recheck kind and conversation on the owning screen
using the distinct `WebGuarded` IPC variant; unsupported peers fail closed.
Focus revalidates selectability and suppression and uses the first attached
terminal. Local focus executes in that same screen turn through the palette's
focus core; a pending terminal layout returns recoverable `unavailable` for retry.
Peers use the palette's session-switch behavior.
Peer focus uses the current session cache, without an atomic cross-server handoff.

Rows contain `id`, `ref`, `session`, `current_session`, `pane_id` (canonical),
`pane` (display number), `provider`, `state`, `glyph`, `status`, `host`, `title`,
`activity`, `project`, `branch`, `last_reply` (`{text,truncated}` or null),
`last_hook_at_ms`, `focused`, `selectable`, `suppressed`, `directory`, `run`,
`wait_needs_a_person`, `source`, `can_prompt`, `prompt_unavailable` and
`usage_unavailable`. Population, ordering, labels, presentation, live headers and
quota reasons reuse the terminal palette. Exited panes and Exited agent records
are excluded from both live lists; their retained run history remains readable.
`can_prompt` describes complete identity, independently of current input readiness.
Missing conversation IDs report `Agent has no conversation ID`.

History pages contain `run`, `retargeted`, `agent_gone`, `live` (row or null),
`header` (`name,status,location,started,task,project,branch,session_id,details`),
`events`, `earlier`, `next_from`, `previous_run`, `origin_run`, `resumable:false`,
`status`, `navigation` and `activity_ranges`. Run-only reads need no live snapshot.
A missing live ID sets `agent_gone` and still reads the supplied run. A live ID
whose current run changed sets `retargeted`, returns the new run and clears old
worker/page/delivery cursors. Clients must replace their cached run on retarget.
`tz` uses the palette's requesting-client timezone rules; omitted/invalid zones
use its explicit unavailable-zone behavior, never the server's local timezone.

Each event has `revision`, `revisions` (one revision for ordinary events, empty
for grouped activity), `range`, `kind`, `created_at_ms`, `time`, `markdown`,
`truncated`, `tool_calls` and a closed `request` for its event/hit/tools reader.
`navigation` maps native row IDs (`earlier`, `continue`, `latest`, `context`,
`received`, `previous`, `origin`, `event:…`, `tools`) to callable requests.
It omits archived search, workers-picker and resume actions. Current-run history
navigation (`earlier`, `continue`, `latest`, `received`) preserves the original
optional `agent` ID, including while that agent is gone. Follow requests as
returned, reapplying `tz`; each bound request takes a fresh live snapshot and
updates the live row/header. For example, a live-bound response may return:

```json
{"op":"history","run":"R","agent":"live:session:7","worker":null,"before":null,"after_revision":42}
```

If the agent disappears, this refresh keeps reading R with `agent_gone:true` and
`live:null`, and its returned refresh still carries the agent ID. If the agent
returns or changes to R2, following that refresh detects it; a changed run returns
`retargeted:true`, R2 content and navigation bound to R2. Replace cached history
on retarget: old worker, page and delivery cursors have been cleared.
`previous`/`origin` links deliberately omit `agent`, even for self-referential
lineage, so intentional stored-run browsing stays independent of the live agent.
Run-only history and hit/event/tools inspection also remain independent; those
reader operations do not accept `agent`. Worker scopes can be supplied on
history/range requests.

Chronological `before` is `{created_at_ms:number|null, revision:number}`; null
sorts before dated entries, with revision breaking timestamp ties. `range` is
`{start:Cursor|null, end:Cursor|null, worker:string|null}` with exclusive, ordered
prose boundaries. `after_revision` is an inclusive revision delivery watermark;
it cannot be combined with `before`. `from` is the search continuation (or a
legacy empty-query delivery watermark), independent of chronological paging.
Search cannot combine with worker/before/after_revision; `from` cannot combine
with worker/before/after_revision. Revisions must be positive (delivery/from may
be zero) and below `i64::MAX`, leaving space for the next revision. Cursor times
are bounded to years -9999 through 9999.

Follow `earlier` navigation for older messages, `continue` for search pages and
`received` for live revision delivery. Before merging events, remove cached tool
activity whose range overlaps any `activity_ranges` interval in the same worker
scope. Preserve replacement intervals even when no new event occupies them.
Then merge by revision and order by timestamp/revision. This retains tools across
earlier pages and handles late revisions without duplicate prose. A boolean
`earlier` and scalar `next_from` alone do not encode current palette navigation.
Range tools return a Reader with readable retained previews and the native page
under `page`; `header` remains a string. Truncation/status remain visible; these
are bounded previews, not full raw archived payloads.

Prompts are nonblank, at most 32 KiB UTF-8 and contain no control characters
except newline/tab. Submission IDs are 1–64 ASCII letters, digits, `.`, `_`, `-`.
Prompt delivery reuses `wait:false`, a 30000 ms timeout and the existing retry
state machine. Keep the same submission ID after uncertain delivery; there is
no automatic resubmission. A retained rejection remains a rejection on retry.
Keys are exactly `esc`, `enter`, `tab`, `up`, `down`, `left`, `right`, `backspace`,
`y`, `n`, `1`–`9`. Screen output removes Unicode controls except newline/tab.
No restart, interrupt, pane close or spawn action is exposed.

Errors use `{error:{code,message}}`: `invalid` for request validation,
`not_found` for missing usage targets/retained events, `target_changed` for
missing/stale action refs, `paused` during update pause, `unavailable` for
snapshot/focus timeout or detached focus (`No terminal attached`), and `failed`
for routing/storage failures. Snapshot/focus replies have 5 s bounds; peer
controls share one 32 s send/receive/cleanup deadline. Non-success native
`AgentResponse` outcomes remain under `ok.response`, including uncertain prompt
outcomes with the original submission ID. Detached list/history/prompt/read/key
requests do not require a palette or terminal client. Focus succeeds as
`{"ok":true}`, not a fabricated `AgentResponse`.

Production SDK/frontend, safe HTML/Markdown rendering, archived-agent search,
archived-run resume, and phone/Tailscale verification are separate work.

### Directory project resolution

`ResolveProject {directory}` requires `ReadApplicationState`. It resolves a local
host directory (relative to the plugin working directory when not absolute) with
the same project model as `ArrangeProjectPanes`. The reply is
`{identity, name, root}`, or `null` for a folder with no project. Invalid or missing
directories and failed probes return errors. The host reads the shared registry;
the plugin does not need `PluginData`, `ReadFiles`, or `RunCommands`.

Remembered roots can contain several repositories. Registered discovered projects,
subdirectories and linked Git worktrees use the same membership rules as pane
arrangement. For an unregistered checkout, the arrangement resolver supplies its
repository identity without registering a project. Published custom-provider
matches take precedence over Git; this command does not invoke provider plugins
for directories outside their published snapshot.

`identity` is an opaque persistence key, distinct from the display `name`. For a
registered project it is `project:<registered-root>`: renaming the display name,
changing branches, and entering linked worktrees or member repositories do not
change it. Moving or merging project roots can change it; it is not a permanent
UUID. Unregistered checkouts use the arrangement key based on provider identity,
normalized Git remote, or host-qualified Git common directory, in that order.
Registering such a checkout can change its key. `root` is the local configuration
root (the primary checkout for Git), not the identity of a multi-repository project.
Store the returned identity rather than deriving a key from `name` or `root`.

The protobuf command carries `resolve_project_payload` as a directory string.
Its reply uses the existing `AgentProvidersResponse` JSON result envelope:
`{"Ok": ResolvedProject | null}` or `{"Err": string}`. The Steel bridge unwraps
this envelope and raises errors.

`GetClientTimezone {client_id?}` defaults to the acting client. Its nullable string
is the validated report: an IANA name, POSIX rules, a fixed UTC offset, or `tzif:`
followed by base64 TZif data. Missing, invalid, disconnected and unknown reports
return null. Explicit rules are parsed as rules, never as filesystem paths.

`GetLocalUtcOffset {epoch_ms, client_id?, timezone?}` returns seconds east of UTC
at the requested instant, including DST and historical transitions. Select either
a client or an explicit rule string; supplying both is an error. Missing/invalid
selected rules or an out-of-range selected timestamp return null, with no
server-zone fallback. Omitted selectors preserve the server-local behavior and
its fallback to the current server offset for out-of-range timestamps.
`GetLocalTime` also remains server-local. These queries require no permission;
`ListClients` still requires `ReadApplicationState`.

`ClientTimezoneChanged` requires no permission and broadcasts every subject
client to all subscribed instances (pane, background/headless and shared).
Successful attachments publish their current value, including null. Validated
report changes publish updates; identical reports within an attachment are
suppressed. Disconnect clears the state and publishes null. Every new attachment
publishes again, including reused IDs. Subscription requests a snapshot of all
attached interactive clients, including unknown zones, ordered with live updates;
snapshot duplicates are allowed. Snapshots are scoped to the subscribing instance
and its runtime generation. The payload's `client_id` is the subject, while the
callback retains the recipient's normal actor. Select the subject explicitly
when resolving its offset.

`ListClients` captures each client's `timezone` with the listing snapshot, before
asynchronous process enrichment. Palette discovery, source refresh and execution
contexts carry `timezone: string | null` for the requesting client at the time of
each request. Older omitted fields decode as null.

Rust callers use `PluginCommand::GetClientTimezone { client_id: None }` and decode
`ProtobufGetClientTimezoneResponse::timezone`. Offset commands specify
`PluginCommand::GetLocalUtcOffset { epoch_ms, client_id, timezone }` and decode
`ProtobufGetLocalUtcOffsetResponse`. Its existing scalar `offset_seconds` field
and tag remain unchanged; additive `unavailable = true` means null. A missing
flag means available, preserving legacy scalar zero replies. Old decoders cannot
distinguish unavailable replies from zero and should upgrade before using selectors.

## Authenticated HTTP transport

The configured `web { plugin ...; }` target receives `yinz-web-dispatch` calls
through its supervised Steel worker. The request is a hash with `id`, `method`,
`path`, `segments`, `query` (last value wins), `query_pairs` (ordered duplicates),
`headers`, `body`, `auth`, `client`, `attached`, `server.session` and `deadline_ms`.
Only `accept`, `accept-language`, `content-type`, `if-none-match` and `user-agent`
are forwarded. `auth` contains the public session ID and label; bearer tokens,
cookies and proxy identity headers stay native. Detached requests have a null
client. An attached call and its continuations retain the selected acting client.

`HttpRespond`, `HttpAssets`, `HttpPublish` and `AgentView` require `RunCommands`.
The last three also require a builtin identity or the configured web target.
Only the selected live parent instance of that target can publish its site;
named workers cannot publish or answer a parent's requests. Each incarnation has
its own reply slots, capped at 64. Unknown, duplicate and closed replies are ignored.
Unload, reload, failed initialization and crashes close waiting slots as unavailable.
A retired incarnation cannot publish over or retire its replacement's snapshot.

`HttpRespond.response` is JSON with integer `status` (200–599), string-valued
`headers` and string `body` (at most 4 MiB). A nonempty body requires content-type;
304 bodies must be empty. Allowed response headers are content-type, etag,
cache-control, vary, retry-after, location and nonempty x-yinz-* names. Names and
values must satisfy HTTP syntax. Location requires a 3xx response and a safe
root-relative URI; authority, backslashes and escaped slash/control attacks are
refused. Invalid matching replies fail as 502. Native security headers remain
authoritative, API caching is always no-store, and Vary includes Authorization.
Unavailable replies are 503 with Retry-After (starting: 1 second, busy: 2,
unavailable or paused: 5); the HTTP deadline produces 504. Failed targets are
unloaded before automatic restart, with a 30-second restart interval.

`HttpAssets` freezes a plugin-relative directory or `builtin:<relative-path>`
from the session's retained builtin tree. All directory components are confined,
symlinks and special files are rejected, and reads are bounded: depth 6, 256
allowlisted files, 2 MiB/file, 8 MiB/tree. Extensions are js/mjs, css, svg, png,
ico, webmanifest, json, woff2 and txt. ETags contain the first 16 SHA-256 bytes.
Public prefixes use lowercase letters, digits, slash, underscore and hyphen,
with leading/trailing slash; API paths are reserved. Up to four prefixes may
coexist, including nested prefixes resolved by longest match.

`HttpPublish` accepts exact canonical public paths and HTML (`text/html; charset=utf-8`),
manifest (`application/manifest+json`) or text (`text/plain; charset=utf-8`).
Pages are limited to 16 and 512 KiB each. A page cannot lie under an asset prefix,
regardless of publication order. Replacing an existing page/prefix uses its
existing slot. Failed publication preserves the prior immutable snapshot.
Public GET/HEAD/ETag requests are served natively without guest calls.
