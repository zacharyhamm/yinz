import { test, session, assert, assertEquals } from "yinz:testing";

// The stub captures real PTY input and treats Ctrl-C as whole-composer clearing.
// Lifecycle reports are injected independently so a test
// can exercise missing, stale, reordered, and matching hooks while a control request waits.
const terminalAgent = String.raw`
stty raw -echo
printf 'agent-args:'
printf '<%s>' "$@"
printf '\r\n\033[?2004hfixture-agent-ready\r\n'
while IFS= read -r line; do
  case "$line" in
    *"$(printf '\003')"*) printf 'composer-cleared\r\n' ;;
  esac
  line=$(printf '%s' "$line" | tr '\003' '\n' | tail -n 1)
  printf 'received:'
  printf '%s' "$line" | od -An -tx1 | tr -d ' \n'
  printf '\r\n'
done
`;

test(
  "TypeScript provider registers, launches, detects, and controls a real terminal",
  { grants: ["RunCommands"] },
  async () => {
    const cwd = session.fixture({}, { "fixture-agent": terminalAgent });
    session.start({
      columns: 180,
      rows: 30,
      backgroundPlugins: [
        {
          path: "./testing-fixtures/agent-provider.ts",
          configuration: { shared: "true" },
        },
      ],
    });
    await session.waitFor("TypeScript provider registration", () =>
      session.logs.includes("fixture agent provider ready"),
    );
    const spawned = await session.agent({
      Spawn: {
        spec: {
          kind: "fixture-agent-alias",
          model: "small model",
          allow_outside_worktree: true,
        },
        cwd,
      },
    });
    assertEquals(spawned.outcome, "ok");
    assert(spawned.spawned);
    const pane = spawned.spawned.pane_id;
    await session.waitForText("agent-args:<--model><small model>");
    await session.waitForText("fixture-agent-ready");
    const listed = await session.agent({ List: {} });
    assert(listed.agents.some((status) => status.agent.kind === "fixture-agent"));
    const report = (hook_event_name: string, extra: Record<string, string> = {}) =>
      session.report(pane, "fixture-agent", {
        hook_event_name,
        session_id: "fixture-conversation",
        ...extra,
      });
    report("SessionStart", { source: "startup" });
    const statusRequest = { Status: { pane_id: pane } };
    await session.waitFor(
      "reported agent ready for input",
      async () => (await session.agent(statusRequest)).agents[0]?.ready === true,
    );
    const rejected = await session.agent({
      Prompt: { pane_id: pane, prompt: ":reset", wait: true, timeout_ms: 5000 },
    });
    assertEquals(rejected.outcome, "rejected");

    let finished = false;
    const pending = session
      .agent({
        Prompt: {
          pane_id: pane,
          expected_session_id: "fixture-conversation",
          prompt: "review\ncode",
          wait: true,
          timeout_ms: 10_000,
        },
      })
      .then((response) => {
        finished = true;
        return response;
      });
    // Read: + bracketed paste containing CR + Ctrl-J submission, with the line delimiter
    // consumed by read. The exact bytes come from the terminal process, not a mocked writer.
    await session.waitForText("composer-cleared");
    await session.waitForText(
      "received:526561643a201b5b3230307e7265766965770d636f64651b5b3230317e",
    );
    assert(!finished);
    report("UserPromptSubmit", { turn_id: "current", prompt: "Read: review\rcode" });
    report("Stop", { turn_id: "stale" });
    const during = await session.agent(statusRequest);
    assert(!finished, "a stale completion cannot finish the submitted prompt");
    assert(!during.agents[0].ready);
    report("Stop", { turn_id: "current", last_assistant_message: "fixture complete" });
    const completed = await pending;
    assertEquals(completed.outcome, "completed");
    assertEquals(completed.turn_id, "current");
    assertEquals(completed.message, "fixture complete");
    assert(completed.acknowledged);
    assertEquals(
      (
        await session.agent({
          Wait: { pane_id: pane, until: "Idle", timeout_ms: 5000 },
        })
      ).outcome,
      "ok",
    );
  },
);

test(
  "OpenCode support is registered by its TypeScript plugin and remains status-only",
  { grants: ["RunCommands"] },
  async () => {
    const cwd = session.fixture({}, { opencode: terminalAgent });
    session.start({
      columns: 180,
      rows: 30,
      backgroundPlugins: [
        { path: "./testing-fixtures/agent-provider.ts", configuration: { shared: "true" } },
        "yinz:agent-opencode",
      ],
    });
    await session.waitFor("provider observer ready", () =>
      session.logs.includes("fixture agent provider ready"),
    );
    await session.waitFor("OpenCode provider registration", () =>
      session.logs.includes("fixture observed provider opencode"),
    );
    const spawned = await session.agent({
      Spawn: {
        spec: { kind: "opencode", model: "fixture/model", allow_outside_worktree: true },
        cwd,
      },
    });
    assertEquals(spawned.outcome, "ok");
    assert(spawned.spawned);
    await session.waitForText("agent-args:<--model><fixture/model>");
    await session.waitForText("fixture-agent-ready");
    const pane = spawned.spawned.pane_id;
    session.report(pane, "opencode", {
      hook_event_name: "SessionStart",
      session_id: "opencode-session",
      source: "startup",
    });
    const status = await session.agent({ Status: { pane_id: pane } });
    assertEquals(status.agents[0].agent.kind, "opencode");
    assert(status.agents[0].status_available);
    assert(!status.agents[0].prompt_supported);
    assertEquals(
      (
        await session.agent({
          Prompt: { pane_id: pane, prompt: "review", wait: true, timeout_ms: 5000 },
        })
      ).outcome,
      "status_unavailable",
    );
    assertEquals(
      (
        await session.agent({
          Wait: { pane_id: pane, until: "Idle", timeout_ms: 5000 },
        })
      ).outcome,
      "ok",
    );
  },
);
