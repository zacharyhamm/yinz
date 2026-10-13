import type { ReadonlyWire } from "./status-bar/keys.ts";
import {
  definePlugin,
  yinz,
  type PaletteAction,
  type PaletteView,
  type PaletteExecution,
  type PaletteCompletion,
  type SessionListSnapshot,
  type SessionInfo,
  type LayoutInfo,
} from "yinz:plugin";

type Values = Readonly<Record<string, string>>;
const text = (value: string): string => yinz.text.sanitize(value);
const clean = (value: string): string => text(value).replaceAll("\n", " ").replaceAll("\t", " ");
const compare = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0);
const snapshot = (): SessionListSnapshot => yinz.command("GetSessionList");
const current = (state: ReadonlyWire<SessionListSnapshot>): ReadonlyWire<SessionInfo> => {
  const session = state.live_sessions.find((s) => s.is_current_session);
  if (!session) throw new Error("Current session is no longer available");
  return session;
};
const saved = (state: ReadonlyWire<SessionListSnapshot>): string[] =>
  state.resurrectable_sessions
    .filter(([name]) => !state.live_sessions.some((s) => s.name === name))
    .map(([name]) => name)
    .sort(compare);

// Search the entire inventory before slicing; page links retain the query.
function matches(query: string, value: string): boolean {
  const chars = Array.from(query.toLowerCase());
  let next = 0;
  for (const char of value.toLowerCase()) if (char === chars[next]) next += 1;
  return next === chars.length;
}
function list(
  title: string,
  collect: () => PaletteAction[],
  page = 0,
  previousQuery = "",
): PaletteView {
  return {
    type: "actions",
    title,
    initialQuery: previousQuery,
    actions(request) {
      const query = request.query;
      const all = collect().filter((a) => matches(query, `${a.label} ${a.description ?? ""}`));
      const offset =
        query === previousQuery
          ? Math.min(page * 98, Math.max(0, Math.ceil(all.length / 98) - 1) * 98)
          : 0;
      // Keep previews below the wire payload limit after searching the full messages.
      const actions = all.slice(offset, offset + 98);
      for (const action of actions)
        action.description = Array.from(action.description ?? "")
          .slice(0, 256)
          .join("");
      if (offset > 0)
        actions.push({
          id: "page:previous",
          label: "Previous results…",
          run: () => list(title, collect, offset / 98 - 1, query),
        });
      if (offset + 98 < all.length)
        actions.push({
          id: "page:next",
          label: "More results…",
          run: () => list(title, collect, offset / 98 + 1, query),
        });
      return actions;
    },
  };
}
function validateName(name: string, optional = false): void {
  if (optional && name === "") return;
  const max = yinz.getPluginIds().max_session_name_bytes;
  if (
    !name.trim() ||
    name === "." ||
    name === ".." ||
    name.includes("/") ||
    clean(name) !== name ||
    (name.startsWith("pty-broker-v") && name.endsWith(".sock"))
  )
    throw new Error("Enter a valid session name without slashes or controls");
  if (max != null && yinz.encodeUtf8(name).length > max)
    throw new Error(`Session name must be at most ${max} bytes`);
}
function switchTo(
  name: string,
  tabId?: number,
  pane?: readonly [number, boolean],
): PaletteCompletion {
  const state = snapshot();
  const live = state.live_sessions.find((s) => s.name === name);
  if (!live && !saved(state).includes(name)) throw new Error("Session no longer exists");
  const tab = tabId === undefined ? undefined : live?.tabs.find((t) => t.tab_id === tabId);
  if (tabId !== undefined && !tab) throw new Error("Tab no longer exists");
  if (
    pane &&
    !Object.values(live?.panes.panes ?? {})
      .flat()
      .some(
        (p) => p.id === pane[0] && p.is_plugin === pane[1] && !p.is_suppressed && p.is_selectable,
      )
  )
    throw new Error("Pane no longer exists");
  if (live?.is_current_session) {
    if (pane)
      yinz.command(pane[1] ? "FocusPluginPane" : "FocusTerminalPane", [pane[0], true, false]);
    else if (tab) yinz.command("GoToTabWithId", tab.tab_id);
    return { type: "complete", notification: "Focus requested" };
  }
  yinz.command("SwitchSession", {
    name,
    tab_position: tab?.position ?? null,
    pane_id: pane ? [pane[0], pane[1]] : null,
    layout: null,
    cwd: null,
  });
  return { type: "complete", notification: "Session switch requested" };
}
function collision(
  name: string,
  state: ReadonlyWire<SessionListSnapshot>,
): PaletteView | undefined {
  const running = state.live_sessions.some((s) => s.name === name);
  if (!running && !saved(state).includes(name)) return undefined;
  return {
    type: "actions",
    title: `Session ${clean(name)} already exists`,
    actions: [
      {
        id: "existing",
        label: running ? "Switch to this session" : "Resurrect this session",
        run: () => switchTo(name),
      },
    ],
  };
}
function layoutLabel(layout: ReadonlyWire<LayoutInfo>): string {
  if ("BuiltIn" in layout) return `${layout.BuiltIn} (built-in)`;
  if ("File" in layout) return `${layout.File[0]} (custom)`;
  return "Inline layout";
}
function create(values: Values): PaletteView | PaletteCompletion {
  const name = values["name"] ?? "";
  validateName(name, true);
  const state = snapshot();
  const existing = collision(name, state);
  if (existing) return existing;
  const directory = values["directory"] ?? "";
  const cwd = directory === "" ? null : directory;
  const selected = values["layout"] ?? "Default";
  const layout =
    selected === "Default"
      ? null
      : state.live_sessions
          .find((session) => session.is_current_session)
          ?.available_layouts.find((item) => layoutLabel(item) === selected);
  if (layout === undefined) throw new Error("Layout no longer exists");
  yinz.command("SwitchSession", {
    name: name || null,
    tab_position: null,
    pane_id: null,
    layout,
    cwd,
  });
  return { type: "complete", notification: "Session creation requested" };
}
function configure(values: Values): PaletteView {
  const layouts = current(snapshot())
    .available_layouts.map((layout) => layoutLabel(layout))
    .sort(compare);
  return {
    type: "form",
    title: "Configure new session",
    submitLabel: "Create and switch",
    fields: [
      { type: "text", id: "name", label: "Name (optional)", initialValue: values["name"] ?? "" },
      {
        type: "choice",
        id: "layout",
        label: "Layout (type to search)",
        options: ["Default", ...layouts],
      },
      {
        type: "directory",
        id: "directory",
        label: "Directory",
        initialValue: values["directory"] ?? "",
      },
    ],
    onSubmit: (v) => create(v),
  };
}
function newSession(): PaletteView {
  return {
    type: "form",
    title: "New session",
    submitLabel: "Create and switch",
    fields: [{ type: "text", id: "name", label: "Name (optional)" }],
    onSubmit: (values) => create(values),
    actions: [{ id: "configure", label: "Configure…", run: configure }],
  };
}
function sessionDestinations(
  session: ReadonlyWire<SessionInfo>,
  kind: "tab" | "pane",
): PaletteAction[] {
  const actions: PaletteAction[] = [];
  for (const tab of [...session.tabs].sort((a, b) => a.position - b.position)) {
    const context = `${clean(session.name)} · ${clean(tab.name)} (tab ${tab.position + 1})`;
    if (kind === "tab")
      actions.push({
        id: JSON.stringify([session.name, tab.tab_id]),
        label: context,
        run: () => switchTo(session.name, tab.tab_id),
      });
    else
      for (const pane of [...(session.panes.panes[String(tab.position)] ?? [])].sort(
        (a, b) => Number(a.is_plugin) - Number(b.is_plugin) || a.id - b.id,
      )) {
        if (pane.is_suppressed || !pane.is_selectable) continue;
        actions.push({
          id: JSON.stringify([session.name, pane.is_plugin, pane.id]),
          label: `[${pane.is_plugin ? "plugin" : "terminal"}_${pane.id}] ${clean(pane.display_title ?? pane.title) || "Untitled pane"}`,
          description: context,
          run: () => switchTo(session.name, tab.tab_id, [pane.id, pane.is_plugin]),
        });
      }
  }
  return actions;
}
function destinations(kind: "session" | "saved" | "tab" | "pane"): PaletteView {
  return list(
    kind === "tab"
      ? "Find tab"
      : kind === "pane"
        ? "Find pane"
        : kind === "saved"
          ? "Resurrect session"
          : "Switch session",
    () => {
      const state = snapshot();
      const actions: PaletteAction[] = [];
      for (const session of [...state.live_sessions].sort((a, b) => compare(a.name, b.name))) {
        if (kind === "saved") continue;
        if (kind === "session") {
          actions.push({
            id: JSON.stringify([session.name]),
            label: clean(session.name),
            description: "Running",
            run: () => switchTo(session.name),
          });
          continue;
        }
        actions.push(...sessionDestinations(session, kind));
      }
      if (kind === "session" || kind === "saved")
        for (const name of saved(state))
          actions.push({
            id: JSON.stringify([name]),
            label: clean(name),
            description: "Saved",
            run: () => switchTo(name),
          });
      return actions;
    },
  );
}
function confirm(
  title: string,
  consequences: string,
  targets: readonly string[],
  read: () => string[],
  mutate: (targets: readonly string[]) => void,
): PaletteView {
  if (targets.length === 0) throw new Error("No matching targets remain");
  const captured = [...targets].sort(compare);
  return {
    type: "form",
    title: `${title}: ${captured.map((name) => clean(name)).join(", ")} — ${consequences}`,
    fields: [],
    confirmation: true,
    submitLabel: title,
    onSubmit(): PaletteView | undefined {
      const now = [...read()].sort(compare);
      if (JSON.stringify(now) !== JSON.stringify(captured))
        return confirm("Targets changed — confirm again", consequences, now, read, mutate);
      mutate(captured);
      return undefined;
    },
  };
}
function requireSuccess(ok: boolean): void {
  if (!ok) throw new Error("Host did not complete the operation");
}
function kill(names?: readonly string[]): PaletteView {
  const read = (): string[] =>
    snapshot()
      .live_sessions.filter((s) => (names ? names.includes(s.name) : !s.is_current_session))
      .map((s) => s.name);
  return confirm(
    "Kill sessions",
    "Stops their processes and disconnects their clients",
    read(),
    read,
    (targets) => {
      requireSuccess(yinz.command("KillSessionsAndReply", [...targets]));
    },
  );
}
function deleteSaved(names?: readonly string[]): PaletteView {
  const read = (): string[] => saved(snapshot()).filter((name) => !names || names.includes(name));
  return confirm(
    "Delete saved sessions",
    "Permanently removes saved state; these sessions cannot be resurrected",
    read(),
    read,
    (targets) => {
      for (const name of targets) requireSuccess(yinz.command("DeleteDeadSessionAndReply", name));
    },
  );
}
function details(session: ReadonlyWire<SessionInfo>): PaletteView {
  const lastSave = session.is_current_session
    ? yinz.command("CurrentSessionLastSavedTime")
    : undefined;
  const lines = [
    `Age: ${session.creation_time.secs}s`,
    `Connected clients: ${session.connected_clients}`,
  ];
  if (lastSave !== undefined)
    lines.push(
      `Last save: ${lastSave === null ? "Never" : `${Math.floor(Number(lastSave) / 1000)}s ago`}`,
    );
  for (const tab of [...session.tabs].sort((a, b) => a.position - b.position)) {
    lines.push(`\nTab ${tab.position + 1}: ${tab.name}`);
    for (const pane of session.panes.panes[String(tab.position)] ?? [])
      lines.push(
        `  ${pane.is_plugin ? "Plugin" : "Terminal"} ${pane.id}: ${pane.title} — ${pane.exited ? `Exited (${pane.exit_status ?? "unknown status"})` : "Running"}`,
      );
  }
  return {
    type: "text",
    title: `Session details: ${clean(session.name)}`,
    text: text(lines.join("\n")),
  };
}
function disconnect(
  _context: Readonly<Pick<PaletteExecution, "clientId" | "onCancel">>,
  changed = false,
  targets = yinz.command("GetDisconnectTargets").map(String).sort(compare),
): PaletteView {
  if (targets.length === 0) throw new Error("No other clients are connected");
  return {
    type: "form",
    title: `${changed ? "Clients changed — confirm again. " : ""}Disconnect clients ${targets.join(", ")}? Their sessions and processes keep running.`,
    fields: [],
    confirmation: true,
    submitLabel: "Disconnect other clients",
    onSubmit(_values, execution): PaletteView | PaletteCompletion {
      const now = yinz.command("GetDisconnectTargets").map(String).sort(compare);
      if (JSON.stringify(now) !== JSON.stringify(targets)) return disconnect(execution, true, now);
      // shortcut: clients can change after revalidation; use guarded disconnection if atomic confirmation is needed.
      yinz.command("DisconnectOtherClients");
      return { type: "complete", notification: "Client disconnection requested" };
    },
  };
}
const actions: PaletteAction[] = [
  { id: "switch-session", label: "Switch session…", run: () => destinations("session") },
  { id: "find-tab", label: "Find tab…", aliases: ["switch tab"], run: () => destinations("tab") },
  {
    id: "find-pane",
    label: "Find pane…",
    aliases: ["focus pane"],
    run: () => destinations("pane"),
  },
  { id: "new-session", label: "New session…", run: newSession },
  { id: "resurrect", label: "Resurrect session…", run: () => destinations("saved") },
  {
    id: "rename",
    label: "Rename current session…",
    run: () => ({
      type: "form",
      title: "Rename current session",
      fields: [{ type: "text", id: "name", label: "Name", initialValue: current(snapshot()).name }],
      submitLabel: "Rename",
      onSubmit(values): PaletteView | PaletteCompletion | undefined {
        const name = values["name"] ?? "";
        validateName(name);
        const state = snapshot();
        if (name === current(state).name) return undefined;
        const existing = collision(name, state);
        if (existing) return existing;
        yinz.command("RenameSession", name);
        return { type: "complete", notification: "Session rename requested" };
      },
    }),
  },
  {
    id: "save",
    label: "Save current session",
    run() {
      requireSuccess(yinz.command("SaveSession"));
      return { type: "complete", notification: "Session saved" };
    },
  },
  {
    id: "kill",
    label: "Kill session…",
    run: () =>
      list("Kill session", () =>
        snapshot()
          .live_sessions.sort((a, b) => compare(a.name, b.name))
          .map((s) => ({
            id: JSON.stringify([s.name]),
            label: clean(s.name),
            run: () => kill([s.name]),
          })),
      ),
  },
  { id: "kill-others", label: "Kill all other sessions…", run: () => kill() },
  {
    id: "delete",
    label: "Delete saved session…",
    run: () =>
      list("Delete saved session", () =>
        saved(snapshot()).map((name) => ({
          id: JSON.stringify([name]),
          label: clean(name),
          run: () => deleteSaved([name]),
        })),
      ),
  },
  { id: "delete-all", label: "Delete all saved sessions…", run: () => deleteSaved() },
  { id: "disconnect", label: "Disconnect other clients", run: disconnect },
  {
    id: "details",
    label: "Session details…",
    run: () =>
      list("Session details", () =>
        snapshot()
          .live_sessions.sort((a, b) => compare(a.name, b.name))
          .map((s) => ({
            id: JSON.stringify([s.name]),
            label: clean(s.name),
            run() {
              const session = snapshot().live_sessions.find((now) => now.name === s.name);
              if (!session) throw new Error("Session no longer exists");
              return details(session);
            },
          })),
      ),
  },
  {
    id: "notifications",
    label: "Notifications…",
    run: () =>
      list("Notifications", () =>
        yinz
          .command("ListNotifications")
          .sort(
            (a, b) =>
              Number(b.received_at_unix_ms) - Number(a.received_at_unix_ms) ||
              Number(b.id) - Number(a.id),
          )
          .map((n) => ({
            id: String(n.id),
            label: Array.from(clean(n.message)).slice(0, 160).join("") || "Notification",
            description: text(n.message),
            run: () => ({ type: "text", title: "Notification", text: text(n.message) }),
          })),
      ),
  },
];
export default definePlugin({
  load() {
    yinz.commandPalette.registerProvider({ id: "sessions", title: "Sessions", actions });
  },
});
