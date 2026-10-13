import {
  test,
  loadPlugin,
  assert,
  assertEquals,
  type PaletteTestView,
  type PaletteTestReply,
} from "yinz:testing";
import type { SessionInfo, SessionListSnapshot } from "yinz:plugin";

function live(name: string, active = false): SessionInfo {
  return {
    name,
    is_current_session: active,
    connected_clients: 1,
    creation_time: { secs: 12, nanos: 0 },
    tabs: [{ tab_id: 7, position: 0, name: "shell" }],
    panes: {
      panes: {
        "0": [
          {
            id: 4,
            is_plugin: false,
            is_selectable: true,
            is_suppressed: false,
            title: "shell",
            exited: false,
            exit_status: null,
          },
        ],
      },
    },
    available_layouts: [{ BuiltIn: "default" }, { File: ["custom", {}] }],
  } as unknown as SessionInfo;
}
async function harness(
  state: SessionListSnapshot = {
    live_sessions: [live("current", true), live("other")],
    resurrectable_sessions: [["saved", { secs: 20, nanos: 0 }]],
  },
) {
  const p = await loadPlugin("./session-palette.ts", {
    replies: {
      RequestPluginPermissions: null,
      CommandPalette: null,
      GetPluginIds: {
        plugin_id: 9,
        client_id: 1,
        yinz_pid: 1,
        initial_cwd: "/tmp",
        hostname: "test",
        host_id: "test",
        max_session_name_bytes: 80,
      },
      GetSessionList: state,
      SaveSession: true,
      KillSessionsAndReply: true,
      DeleteDeadSessionAndReply: true,
      SwitchSession: null,
      RenameSession: null,
      GetDisconnectTargets: [2],
      DisconnectOtherClients: null,
      CurrentSessionLastSavedTime: 1000,
      ListNotifications: [],
    },
  });
  await p.event({ PermissionRequestResult: "Granted" });
  const registration = p.paletteCommands.find((c) => c.type === "register");
  assert(registration?.type === "register");
  let scope = { registration: registration.registration, opening: 1, view: 0, revision: 1 };
  const context = { clientId: 1, paneId: { Terminal: 4 }, tabId: 7, currentProject: null };
  function reply(): PaletteTestReply {
    const command = p.paletteCommands.at(-1);
    assert(command?.type === "reply");
    if ("Err" in command.result) throw new Error(command.result.Err);
    return command.result.Ok;
  }
  async function discover(source: string | null = null, query = "") {
    scope = { ...scope, revision: scope.revision + 1 };
    await p.palette({ type: "discover", scope, context, source, query }, 1);
    const result = reply();
    assert(result.type === "actions");
    return result.actions;
  }
  async function execute(handle: string, values: Record<string, string> | null = null) {
    scope = { ...scope, revision: scope.revision + 1 };
    await p.palette({ type: "execute", scope, context, handle, values }, 1);
    const result = reply();
    if (result.type === "view") scope = { ...scope, view: scope.view + 1 };
    return result;
  }
  const roots = await discover();
  async function open(id: string): Promise<PaletteTestView> {
    const action = roots.find((a) => a.id === id);
    assert(action);
    const result = await execute(action.handle);
    assert(result.type === "view");
    return result.view;
  }
  return { p, roots, open, execute, discover, reply, scope: () => scope, context };
}

test("session palette registers every root command and separates running from saved", async () => {
  const h = await harness();
  assertEquals(h.roots.length, 14);
  const view = await h.open("switch-session");
  assert(view.type === "actions");
  const actions = await h.discover(view.source);
  assertEquals(
    actions.map((a) => [a.label, a.description]),
    [
      ["current", "Running"],
      ["other", "Running"],
      ["saved", "Saved"],
    ],
  );
  await h.execute(actions[2]!.handle);
  assert(h.p.commands.some((c) => JSON.stringify(c).includes('"name":"saved"')));
});

test("session pagination searches the full inventory and preserves duplicate destination context", async () => {
  const h = await harness({
    live_sessions: Array.from({ length: 205 }, (_, i) =>
      live(`session-${String(i).padStart(3, "0")}`, i === 0),
    ),
    resurrectable_sessions: [],
  });
  const view = await h.open("find-pane");
  assert(view.type === "actions");
  const page = await h.discover(view.source);
  assertEquals(page.length, 99);
  assertEquals(new Set(page.map((a) => a.id)).size, 99);
  const next = await h.execute(page.at(-1)!.handle);
  assert(next.type === "view" && next.view.type === "actions");
  const second = await h.discover(next.view.source, next.view.initialQuery ?? "");
  assert(second[0]!.description.includes("session-098"));
  const found = await h.discover(view.source, "session-204");
  assertEquals(found.length, 1);
  await h.execute(found[0]!.handle);
  assert(
    h.p.commands.some((c) =>
      JSON.stringify(c).includes('"name":"session-204","tab_position":0,"pane_id":[4,false]'),
    ),
  );
});

test("new session supports automatic names, collisions, secondary configuration and validation", async () => {
  const h = await harness();
  const form = await h.open("new-session");
  assert(form.type === "form");
  const collision = await h.execute(form.handle, { name: "saved" });
  assert(collision.type === "view" && collision.view.type === "actions");
  assertEquals(collision.view.actions[0]!.label, "Resurrect this session");
  const running = await h.execute(form.handle, { name: "other" });
  assert(running.type === "view" && running.view.type === "actions");
  assertEquals(running.view.actions[0]!.label, "Switch to this session");
  let invalid = false;
  try {
    await h.execute(form.handle, { name: "bad/name" });
  } catch {
    invalid = true;
  }
  assert(invalid);
  const configured = await h.execute(form.actions[0]!.handle, { name: "chosen" });
  assert(configured.type === "view" && configured.view.type === "form");
  assertEquals(configured.view.fields[0]!.initialValue, "chosen");
  const layout = configured.view.fields[1];
  assert(layout?.type === "choice");
  assertEquals(layout.options, ["Default", "custom (custom)", "default (built-in)"]);
  await h.execute(configured.view.handle, {
    name: "chosen",
    layout: "custom (custom)",
    directory: "",
  });
  assert(h.p.commands.some((c) => JSON.stringify(c).includes('"layout":{"File":["custom",{}]}')));
  await h.execute(form.handle, { name: "" });
  assert(h.p.commands.some((c) => JSON.stringify(c).includes('"name":null')));
});

test("destructive operations capture targets and require revised confirmation", async () => {
  const h = await harness();
  const form = await h.open("kill-others");
  assert(form.type === "form" && form.confirmation);
  assert(form.title.includes("other") && !form.title.includes("current"));
  h.p.stub("GetSessionList", {
    live_sessions: [live("current", true), live("new")],
    resurrectable_sessions: [],
  });
  const revised = await h.execute(form.handle, {});
  assert(revised.type === "view" && revised.view.type === "form");
  assert(revised.view.title.includes("confirm again"));
  assert(!h.p.commands.some((c) => JSON.stringify(c).includes("KillSessionsAndReply")));
  await h.execute(revised.view.handle, {});
  assert(h.p.commands.some((c) => JSON.stringify(c) === '{"KillSessionsAndReply":["new"]}'));
});

test("deleted destinations report errors without switching or creating", async () => {
  const h = await harness();
  const view = await h.open("find-tab");
  assert(view.type === "actions");
  const choices = await h.discover(view.source, "other");
  h.p.stub("GetSessionList", {
    live_sessions: [live("current", true)],
    resurrectable_sessions: [],
  });
  let failed = false;
  try {
    await h.execute(choices[0]!.handle);
  } catch (error) {
    failed = String(error).includes("no longer exists");
  }
  assert(failed);
  assert(!h.p.commands.some((c) => JSON.stringify(c).includes("SwitchSession")));
});

test("save host errors stay inline and notifications are newest first with full searchable details", async () => {
  const h = await harness();
  h.p.stub("SaveSession", false);
  let failed = false;
  try {
    await h.execute(h.roots.find((a) => a.id === "save")!.handle);
  } catch {
    failed = true;
  }
  assert(failed);
  h.p.stub("ListNotifications", [
    { id: 1, received_at_unix_ms: 1, message: "old" },
    { id: 2, received_at_unix_ms: 2, message: "new\nfull message" },
  ]);
  const view = await h.open("notifications");
  assert(view.type === "actions");
  const choices = await h.discover(view.source);
  assertEquals(
    choices.map((a) => a.id),
    ["2", "1"],
  );
  const detail = await h.execute(choices[0]!.handle);
  assert(detail.type === "view" && detail.view.type === "text");
  assertEquals(detail.view.text, "new\nfull message");
  assertEquals((await h.discover(view.source, "full message")).length, 1);
  const message = "🔔 long notification ".repeat(220) + "searchable ending";
  h.p.stub(
    "ListNotifications",
    Array.from({ length: 205 }, (_, id) => ({
      id,
      received_at_unix_ms: id,
      message,
    })),
  );
  const large = await h.discover(view.source, "searchable ending");
  assertEquals(large.length, 99);
  assert(JSON.stringify(large).length < 100_000);
  const full = await h.execute(large[0]!.handle);
  assert(full.type === "view" && full.view.type === "text");
  assertEquals(full.view.text, message);
});

test("rename, resurrection, deletion and details use typed host commands", async () => {
  const h = await harness();
  const rename = await h.open("rename");
  assert(rename.type === "form");
  await h.execute(rename.handle, { name: "renamed" });
  assert(h.p.commands.some((c) => JSON.stringify(c) === '{"RenameSession":"renamed"}'));
  const resurrect = await h.open("resurrect");
  assert(resurrect.type === "actions");
  assertEquals(
    (await h.discover(resurrect.source)).map((a) => a.label),
    ["saved"],
  );
  const details = await h.open("details");
  assert(details.type === "actions");
  const detail = await h.execute((await h.discover(details.source, "current"))[0]!.handle);
  assert(
    detail.type === "view" &&
      detail.view.type === "text" &&
      detail.view.text.includes("Last save: 1s ago"),
  );
  const deletion = await h.open("delete-all");
  assert(deletion.type === "form" && deletion.confirmation);
  await h.execute(deletion.handle, {});
  assert(h.p.commands.some((c) => JSON.stringify(c) === '{"DeleteDeadSessionAndReply":"saved"}'));
  const single = await h.open("delete");
  assert(single.type === "actions");
  assertEquals((await h.discover(single.source)).length, 1);
  const kill = await h.open("kill");
  assert(kill.type === "actions");
  assertEquals((await h.discover(kill.source)).length, 2);
});

test("directory browsing returns or cancels without discarding current form values", async () => {
  const h = await harness();
  const first = await h.open("new-session");
  assert(first.type === "form");
  const result = await h.execute(first.actions[0]!.handle, { name: "kept" });
  assert(result.type === "view" && result.view.type === "form");
  const browse = result.view.actions.find((a) => a.id === "browse:directory");
  assert(browse);
  for (const selected of ["/chosen", null]) {
    await h.p.palette(
      {
        type: "execute",
        scope: { ...h.scope(), revision: h.scope().revision + 1 },
        context: h.context,
        handle: browse.handle,
        values: { name: "edited", layout: "custom (custom)", directory: "/original" },
      },
      1,
    );
    const pick = h.p.pickRequests.at(-1);
    assert(pick);
    assertEquals(pick.options, { kind: "directory", roots: ["/original"] });
    await h.p.completePick(pick.request_id, selected);
    const reply = h.reply();
    assert(reply.type === "fields");
    assertEquals(reply.values, {
      name: "edited",
      layout: "custom (custom)",
      directory: selected ?? "/original",
    });
    const transitions = h.p.paletteCommands.filter((c) => c.type === "suspend");
    assertEquals(
      transitions.slice(-2).map((c) => c.type === "suspend" && c.suspended),
      [true, false],
    );
  }
});

test("canonical pane IDs remain searchable before normal and long titles", async () => {
  const session = live("current", true);
  const pane = { ...session.panes.panes["0"]![0]!, id: 42 };
  const state = { live_sessions: [session], resurrectable_sessions: [] };
  const h = await harness(state);
  const view = await h.open("find-pane");
  assert(view.type === "actions");
  for (const title of ["shell", "long title ".repeat(100)]) {
    session.panes.panes["0"] = [
      { ...pane, title },
      { ...pane, title, is_plugin: true },
    ];
    h.p.stub("GetSessionList", state);
    for (const isPlugin of [false, true]) {
      const id = `${isPlugin ? "plugin" : "terminal"}_42`;
      const actions = await h.discover(view.source, id);
      assertEquals(actions.length, 1);
      assertEquals(actions[0]!.id, JSON.stringify([session.name, isPlugin, 42]));
      assert(actions[0]!.label.startsWith(`[${id}] `));
    }
  }
});

test("pane identifiers distinguish plugins from terminals and missing panes fail", async () => {
  const session = live("other");
  session.panes.panes["0"]!.push({ ...session.panes.panes["0"]![0]!, is_plugin: true });
  const h = await harness({
    live_sessions: [live("current", true), session],
    resurrectable_sessions: [],
  });
  const view = await h.open("find-pane");
  assert(view.type === "actions");
  const actions = await h.discover(view.source, "other");
  assertEquals(actions.length, 2);
  assert(actions[0]!.id !== actions[1]!.id);
  await h.execute(actions[1]!.handle);
  assert(h.p.commands.some((c) => JSON.stringify(c).includes('"pane_id":[4,true]')));
  session.panes.panes["0"] = [];
  h.p.stub("GetSessionList", { live_sessions: [session], resurrectable_sessions: [] });
  let failed = false;
  try {
    await h.execute(actions[0]!.handle);
  } catch (error) {
    failed = String(error).includes("Pane no longer exists");
  }
  assert(failed);
});

test("disconnect confirmation rechecks exact server targets", async () => {
  const h = await harness();
  let form = await h.open("disconnect");
  assert(form.type === "form" && form.confirmation && form.title.includes("clients 2?"));
  h.p.stub("GetDisconnectTargets", [3, 2]);
  const changed = await h.execute(form.handle, {});
  assert(changed.type === "view" && changed.view.type === "form");
  form = changed.view;
  assert(form.title.includes("Clients changed") && form.title.includes("clients 2, 3?"));
  assert(!h.p.commands.includes("DisconnectOtherClients"));
  const done = await h.execute(form.handle, {});
  assert(done.type === "complete");
  assert(h.p.commands.includes("DisconnectOtherClients"));
});

test("disconnect query failure and empty inventory never disconnect", async () => {
  const h = await harness();
  const form = await h.open("disconnect");
  assert(form.type === "form");
  for (const reply of [[], { error: "query failed" }]) {
    h.p.stub("GetDisconnectTargets", reply);
    let failed = false;
    try {
      await h.execute(form.handle, {});
    } catch {
      failed = true;
    }
    assert(failed);
    assert(!h.p.commands.includes("DisconnectOtherClients"));
  }
});

test("disconnect cancellation and reopening uses fresh targets", async () => {
  const h = await harness();
  const form = await h.open("disconnect");
  assert(form.type === "form");
  await h.p.palette(
    { type: "release", handles: [form.handle, ...form.actions.map((a) => a.handle)] },
    1,
  );
  h.p.stub("GetDisconnectTargets", [4]);
  const reopened = await h.open("disconnect");
  assert(reopened.type === "form" && reopened.title.includes("clients 4?"));
  let failed = false;
  try {
    await h.execute(form.handle, {});
  } catch {
    failed = true;
  }
  assert(failed);
  assert(!h.p.commands.includes("DisconnectOtherClients"));
});

test("session names enforce UTF-8 byte limits and released form callbacks cannot run", async () => {
  const h = await harness();
  const form = await h.open("new-session");
  assert(form.type === "form");
  h.p.stub("GetPluginIds", { max_session_name_bytes: 4 });
  for (const name of ["雪雪", ".", "..", "pty-broker-v1.sock", "line\nname", " "]) {
    let rejected = false;
    try {
      await h.execute(form.handle, { name });
    } catch {
      rejected = true;
    }
    assert(rejected, name);
  }
  await h.p.palette(
    { type: "release", handles: [form.handle, ...form.actions.map((a) => a.handle)] },
    1,
  );
  let released = false;
  try {
    await h.execute(form.actions[0]!.handle, { name: "ok" });
  } catch (error) {
    released = String(error).includes("no longer available");
  }
  assert(released);
});

test("pagination preloads the search and clearing it searches the entire collection", async () => {
  const h = await harness({
    live_sessions: Array.from({ length: 205 }, (_, i) =>
      live(`session-${String(i).padStart(3, "0")}`, i === 0),
    ),
    resurrectable_sessions: [],
  });
  const view = await h.open("switch-session");
  assert(view.type === "actions");
  const actions = await h.discover(view.source, "session-1");
  const next = await h.execute(actions.at(-1)!.handle);
  assert(next.type === "view" && next.view.type === "actions");
  assertEquals(next.view.initialQuery, "session-1");
  const cleared = await h.discover(next.view.source, "");
  assertEquals(cleared[0]!.label, "session-000");
});
