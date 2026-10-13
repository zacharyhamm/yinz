import { test, session, assert, assertEquals } from "yinz:testing";

test(
  "Mother Overview reports setup blockers and controls its supervisor",
  { grants: ["RunCommands"] },
  async () => {
    session.fixture(
      { ".claude/settings.json": "{}", ".claude.json": "{}" },
      {
        // Keep the agent on the fixture PATH instead of Bubblewrap's host-derived PATH.
        bwrap: "exit 1\n",
        claude: "printf 'MOTHER FIXTURE READY\\r\\n'\nwhile IFS= read -r line; do :; done\n",
      },
    );
    session.start({ columns: 180, rows: 36 });
    await session.waitForText("yinz-test>");
    session.key("Ctrl e");
    session.text("m");
    await session.waitForText("Launch mother");
    session.text("claude");
    await session.waitForText("> claude");
    session.key("Enter");
    await session.waitForText("MOTHER FIXTURE READY");
    session.action("NextPaneTab;");
    await session.waitForText("Supervision:");
    await session.waitForText("lifecycle hooks");
    const paused = await session.agent({ Mother: "Pause" });
    assertEquals(paused.outcome, "ok");
    await session.waitForText("Supervision: paused");
    session.key("p");
    await session.waitFor(
      "Overview resumed observation",
      () => !session.screen.includes("Supervision: paused"),
    );
    session.action("PreviousPaneTab;");
    await session.waitForText("MOTHER FIXTURE READY");
  },
);

test(
  "Mother Overview reads Mother and worker archives with an attention filter",
  { grants: ["RunCommands"] },
  async () => {
    const cwd = session.fixture(
      { ".claude/settings.json": "{}", ".claude.json": "{}" },
      {
        bwrap: "exit 1\n",
        claude:
          "printf '\\033[?2004hOVERVIEW AGENT READY\\r\\n'\nwhile IFS= read -r line; do :; done\n",
      },
    );
    session.start({
      columns: 180,
      rows: 40,
      backgroundPlugins: [{ path: "yinz:agent-service", configuration: { shared: "true" } }],
    });
    await session.waitForText("yinz-test>");
    await session.waitFor("archive schemas", () =>
      session.catalog().some((entity) => entity.def.name === "agent_run_entry"),
    );
    const workers = [];
    for (const name of ["worker-one", "worker-two"]) {
      const spawned = await session.agent({
        Spawn: { spec: { kind: "claude", allow_outside_worktree: true }, cwd },
      });
      assertEquals(spawned.outcome, "ok");
      if (!spawned.spawned) throw new Error("worker pane missing");
      workers.push(spawned.spawned.pane_id);
      session.report(spawned.spawned.pane_id, "claude", {
        hook_event_name: "SessionStart",
        session_id: name,
      });
      session.report(spawned.spawned.pane_id, "claude", {
        hook_event_name: "UserPromptSubmit",
        session_id: name,
        prompt: `Retained prompt for ${name}`,
      });
      session.report(spawned.spawned.pane_id, "claude", {
        hook_event_name: "Stop",
        session_id: name,
        last_assistant_message: `Retained reply for ${name}`,
      });
    }
    session.key("Ctrl e");
    session.text("m");
    await session.waitForText("Launch mother");
    session.text("claude");
    await session.waitForText("> claude");
    session.key("Enter");
    await session.waitForText("OVERVIEW AGENT READY");
    let response = await session.agent({ Mother: "Overview" });
    await session.waitFor("Mother overview identity", async () => {
      response = await session.agent({ Mother: "Overview" });
      return response.outcome === "ok" && response.output !== null;
    });
    if (!response.output) throw new Error("Mother identity missing");
    const identity = JSON.parse(response.output) as { mother: { pane_id: string } };
    const mother = { Terminal: Number(identity.mother.pane_id.slice(9)) };
    await session.agent({ Mother: "Pause" });
    session.report(mother, "claude", {
      hook_event_name: "SessionStart",
      session_id: "mother-reader",
    });
    session.report(mother, "claude", {
      hook_event_name: "UserPromptSubmit",
      session_id: "mother-reader",
      prompt: "Supervise the retained workers",
    });
    session.report(mother, "claude", {
      hook_event_name: "Stop",
      session_id: "mother-reader",
      last_assistant_message: "Mother retained Markdown reply",
    });
    const first = workers[0];
    if (!first) throw new Error("first worker missing");
    session.report(first, "claude", {
      hook_event_name: "Notification",
      session_id: "worker-one",
      notification_type: "permission_prompt",
      message: "Worker needs permission",
    });
    session.action("NextPaneTab;");
    await session.waitForText("Supervision: paused");
    await session.waitForText("Mother retained Markdown reply");
    session.text("a");
    await session.waitForText("attention only");
    session.text("j");
    await session.waitForText("Retained reply for worker-one");
    session.key("Tab");
    session.key("Enter");
    await session.waitForText("Esc: timeline");
    session.key("Esc");
    session.text("/Retained reply for worker-two");
    session.key("Enter");
    await session.waitForText("Search results");
    await session.waitForText("Retained reply for worker-two");
    session.key("Enter");
    await session.waitForText("Esc: timeline");
    session.key("Esc");
    await session.waitForText("Tab: roster");
    assert(session.screen.includes("Retained reply for worker-two"));
    assert(!session.screen.includes("attention only"));
  },
);
