import { test, loadPlugin, assert, assertEquals } from "yinz:testing";
import type { PluginHarness } from "yinz:testing";

function requests(plugin: PluginHarness): { id: string; request: string }[] {
  return plugin.commands.flatMap((command) => {
    if (typeof command === "object" && command !== null && "AgentControl" in command)
      return [command.AgentControl as { id: string; request: string }];
    return [];
  });
}
async function answer(plugin: PluginHarness, value: unknown): Promise<void> {
  const request = requests(plugin).at(-1);
  assert(request);
  await plugin.event({
    CustomMessage: [
      "agent-response",
      JSON.stringify({
        id: request.id,
        response: {
          outcome: "ok",
          output: JSON.stringify(value),
          agents: [],
          spawned: null,
          message: null,
          acknowledged: false,
          turn_id: null,
        },
      }),
    ],
  });
}
const overview = {
  mother: {
    pane_id: "terminal_1",
    conversation_id: "mother",
    host_id: "host",
    yinz_session_name: "main",
  },
  observation_ready: true,
  paused: false,
  status: { state: "watching", pending: 0, blockers: 0 },
  checkpoint: { revision: 3, value: null },
  roster: {},
  batches: [],
  receipts: [],
  delivery_blocked: null,
  operation_in_flight: false,
  gap: false,
  recent: { records: [], next_cursor: null, high_water: 0, retention_gap: false },
};
test("Overview polls only while visible and keeps one request in flight", async () => {
  const plugin = await loadPlugin("./mother-overview.ts", {
    replies: {
      AgentControl: null,
      DataFetch: { columns: [], rows: [], truncated: false, next_cursor: null },
    },
  });
  await plugin.advance(3000);
  assertEquals(requests(plugin).length, 0);
  await plugin.event({ Visible: true });
  assertEquals(requests(plugin).length, 1);
  await plugin.advance(3000);
  assertEquals(requests(plugin).length, 1);
  await answer(plugin, overview);
  assert((await plugin.render(24, 100)).includes("Give Mother an assignment in Conversation"));
  await plugin.advance(1000);
  assertEquals(requests(plugin).length, 2);
  await plugin.event({ Visible: false });
  await answer(plugin, { ...overview, status: { ...overview.status, state: "paused" } });
  await plugin.advance(3000);
  assertEquals(requests(plugin).length, 2);
  assert((await plugin.render(24, 100)).includes("Supervision: paused"));
});

test("Overview keeps settled failures visible until their batches are acknowledged", async () => {
  const plugin = await loadPlugin("./mother-overview.ts", {
    replies: {
      AgentControl: null,
      DataFetch: { columns: [], rows: [], truncated: false, next_cursor: null },
    },
  });
  const receipts = [
    { batch: "pending", submission: "failed-turn", settled: true, response: { outcome: "failed" } },
    { batch: "acked", submission: "acked-turn", settled: true, response: { outcome: "failed" } },
    {
      batch: "pending",
      submission: "later-turn",
      settled: true,
      response: { outcome: "completed" },
    },
  ];
  await plugin.event({ Visible: true });
  await answer(plugin, { ...overview, batches: [{ id: "pending" }], receipts });
  const rendered = await plugin.render(24, 100);
  assert(rendered.includes("failed-turn: failed"));
  assert(!rendered.includes("acked-turn"));
  assert(!rendered.includes("later-turn"));
  await plugin.advance(1000);
  await answer(plugin, { ...overview, receipts });
  assert(!(await plugin.render(24, 100)).includes("failed-turn"));
});

test("Overview recovery uses the reviewed revision and requires a visible confirmation", async () => {
  const plugin = await loadPlugin("./mother-overview.ts", {
    replies: {
      AgentControl: null,
      DataFetch: { columns: [], rows: [], truncated: false, next_cursor: null },
    },
  });
  await plugin.event({ Visible: true });
  await answer(plugin, {
    ...overview,
    receipts: [
      { submission: "missing", settled: false, response: { outcome: "status_unavailable" } },
    ],
  });
  const key = async (char: string): Promise<void> =>
    plugin.event({ Key: { bare_key: { Char: char }, key_modifiers: [] } });
  await key("r");
  await plugin.render(2, 20);
  await key("y");
  assertEquals(requests(plugin).length, 1);
  assert((await plugin.render(10, 100)).includes("Work may already have occurred"));
  await key("y");
  assertEquals(JSON.parse(requests(plugin).at(-1)?.request ?? "{}"), {
    Mother: { Resolve: { submission_id: "missing", expect_rev: 3 } },
  });
});

test("A successful refresh does not hide a rejected control", async () => {
  const plugin = await loadPlugin("./mother-overview.ts", {
    replies: {
      AgentControl: null,
      DataFetch: { columns: [], rows: [], truncated: false, next_cursor: null },
    },
  });
  await plugin.event({ Visible: true });
  await answer(plugin, overview);
  await plugin.event({ Key: { bare_key: { Char: "p" }, key_modifiers: [] } });
  const request = requests(plugin).at(-1);
  assert(request);
  await plugin.event({
    CustomMessage: [
      "agent-response",
      JSON.stringify({
        id: request.id,
        response: { outcome: "rejected", output: null, message: "Control rejected" },
      }),
    ],
  });
  await answer(plugin, overview);
  assert((await plugin.render(24, 100)).includes("Control rejected"));
});

import z, { type DataValue, type FetchRequest, type Pred } from "yinz:plugin";
type Stored = Record<string, string | number | boolean | null>;
const scalar = (value: DataValue): unknown => (value === "Null" ? null : Object.values(value)[0]);
function matches(row: Stored, pred: Pred | null): boolean {
  if (!pred) return true;
  if ("And" in pred) return pred.And.every((part) => matches(row, part));
  if ("Or" in pred) return pred.Or.some((part) => matches(row, part));
  if ("Eq" in pred) return row[pred.Eq[0]] === scalar(pred.Eq[1]);
  if ("Lt" in pred) return Number(row[pred.Lt[0]]) < Number(scalar(pred.Lt[1]));
  if ("In" in pred) return pred.In[1].some((value) => scalar(value) === row[pred.In[0]]);
  throw new Error(`Unsupported predicate ${JSON.stringify(pred)}`);
}
const key = (plugin: PluginHarness, char: string): Promise<void> =>
  plugin.event({ Key: { bare_key: { Char: char }, key_modifiers: [] } });
const special = (
  plugin: PluginHarness,
  bare_key: "Tab" | "Enter" | "Esc" | "Up" | "End",
): Promise<void> => plugin.event({ Key: { bare_key, key_modifiers: [] } });
async function fixture() {
  const run = (id: string, pane: number, conversation: string, extra: Stored = {}): Stored => ({
    uid: id,
    pane_id: pane,
    session_id: conversation,
    host_id: "host",
    yinz_session_name: "main",
    started_at_ms: 1,
    header: JSON.stringify({ id }),
    payload: "{}",
    ...extra,
  });
  const archives = [run("mother-run", 1, "mother"), run("worker-run", 2, "worker")];
  const entry = (revision: number, text: string, run_id = "mother-run"): Stored => ({
    run_id,
    revision,
    kind: "message",
    text,
    created_at_ms: revision * 1000,
  });
  const messages = [
    entry(1, "# First reply\n\n- list item\n\n> quoted text\n\n```rust\nlet answer = 42;\n```"),
    entry(2, "Newest reply"),
    entry(1, "Worker retained answer", "worker-run"),
  ];
  const plugin = await loadPlugin("./mother-overview.ts", {
    replies: {
      AgentControl: null,
      FocusTerminalPane: null,
      CopyToClipboard: null,
      DataFetch: ({ request }: { request: FetchRequest }) => {
        let rows = (request.entity === "agent_run" ? archives : messages).filter((row) =>
          matches(row, request.filter),
        );
        if (request.search)
          rows = rows.filter((row) => String(row["text"]).includes(request.search ?? ""));
        for (const [column, direction] of request.sort)
          rows.sort(
            (a, b) => (Number(a[column]) - Number(b[column])) * (direction === "Desc" ? -1 : 1),
          );
        const columns =
          request.select.length > 0
            ? request.select
            : [...new Set(rows.flatMap((row) => Object.keys(row)))];
        let bytes = 0;
        const end = rows.findIndex((row, index) => {
          const rowBytes = columns.reduce((size, column) => {
            const value = row[column];
            return (
              size +
              (typeof value === "string"
                ? z.encodeUtf8(value).length
                : typeof value === "number"
                  ? 8
                  : 1)
            );
          }, 0);
          if (index >= request.limit || (index > 0 && bytes + rowBytes > 2 * 1024 * 1024))
            return true;
          bytes += rowBytes;
          return false;
        });
        const truncated = end >= 0;
        if (truncated) rows = rows.slice(0, end);
        return {
          columns,
          rows: rows.map((row) =>
            columns.map((column) => {
              const value = row[column];
              return value == null
                ? "Null"
                : typeof value === "number"
                  ? { Int: value }
                  : typeof value === "boolean"
                    ? { Bool: value }
                    : { Text: value };
            }),
          ),
          truncated,
          next_cursor: null,
        };
      },
    },
  });
  const snapshot = {
    ...overview,
    roster: {
      terminal_2: {
        kind: "codex",
        state: "waiting",
        conversation_id: "worker",
        needs_input: true,
        projects: ["project"],
      },
    },
  };
  await plugin.event({ Visible: true });
  await answer(plugin, snapshot);
  return { plugin, archives, messages, entry, run, snapshot };
}

test("Mother roster navigates, focuses and filters; reader renders Markdown and returns", async () => {
  const { plugin } = await fixture();
  let frame = await plugin.render(30, 100);
  assert(frame.indexOf("Mother") < frame.indexOf("terminal_2"));
  await key(plugin, "j");
  await key(plugin, "f");
  assert(JSON.stringify(plugin.commands).includes('"FocusTerminalPane":[2,true,false]'));
  assert((await plugin.render(30, 100)).includes("Worker retained answer"));
  await key(plugin, "a");
  assert((await plugin.render(30, 100)).includes("attention only"));
  await key(plugin, "k");
  await special(plugin, "Tab");
  await special(plugin, "Up");
  await special(plugin, "Enter");
  frame = await plugin.render(30, 100);
  assert(
    frame.includes("First reply") &&
      frame.includes("list item") &&
      frame.includes("let answer = 42"),
  );
  await key(plugin, "y");
  assert(JSON.stringify(plugin.commands).includes("CopyToClipboard"));
  await key(plugin, "v");
  assert((await plugin.render(30, 100)).includes('"revision"'));
  await special(plugin, "Esc");
  assert((await plugin.render(30, 100)).includes("Agents"));
  await key(plugin, "?");
  assert((await plugin.render(30, 100)).includes("pause/resume"));
});

test("Refresh and resize preserve reading position and follow only at tail", async () => {
  const { plugin, messages, entry, snapshot } = await fixture();
  await special(plugin, "Tab");
  await special(plugin, "Up");
  messages.push(entry(3, "THIRD NEW REPLY"));
  await plugin.advance(1000);
  await answer(plugin, snapshot);
  assert(!(await plugin.render(24, 70)).includes("THIRD NEW REPLY"));
  await special(plugin, "End");
  assert((await plugin.render(24, 100)).includes("THIRD NEW REPLY"));
  messages.push(entry(4, "FOURTH NEW REPLY"));
  await plugin.advance(1000);
  await answer(plugin, snapshot);
  assert((await plugin.render(24, 100)).includes("FOURTH NEW REPLY"));
});

test("Archives isolate host, session and conversation including pane reuse", async () => {
  const { plugin, archives, run, snapshot } = await fixture();
  archives.push(
    run("foreign", 2, "worker", { host_id: "foreign" }),
    run("other-session", 2, "worker", { yinz_session_name: "other" }),
  );
  await key(plugin, "j");
  assert((await plugin.render(30, 100)).includes("Worker retained answer"));
  await plugin.advance(1000);
  await answer(plugin, {
    ...snapshot,
    roster: { terminal_2: { ...snapshot.roster.terminal_2, conversation_id: "replacement" } },
  });
  await key(plugin, "j");
  const frame = await plugin.render(30, 100);
  assert(frame.includes("Missing archive"));
  assert(!frame.includes("Worker retained answer"));
});

test("Recorded predecessors, legacy payloads and gaps remain readable", async () => {
  const { plugin, archives, run } = await fixture();
  archives[0] = run("mother-run", 1, "mother", {
    header: JSON.stringify({ id: "mother-run", previous_run_id: "legacy" }),
  });
  archives.push(
    run("legacy", 8, "previous", {
      host_id: "",
      header: null,
      payload: JSON.stringify({
        id: "legacy",
        previous_run_id: "missing",
        entries: [{ revision: 1, kind: "prompt", text: "Legacy retained prompt" }],
      }),
    }),
  );
  await special(plugin, "Tab");
  await special(plugin, "End");
  await key(plugin, "n");
  const frame = await plugin.render(30, 100);
  assert(frame.includes("Incomplete ancestry"));
  assert(frame.includes("Legacy retained prompt"));
});

test("Search identifies workers and opens surrounding history; pages are bounded", async () => {
  const { plugin, messages, entry, snapshot } = await fixture();
  for (let index = 3; index <= 80; index += 1)
    messages.push(entry(index, `Retained reply ${index}`));
  await special(plugin, "Tab");
  await special(plugin, "End");
  assert((await plugin.render(30, 100)).includes("Retained reply 80"));
  await key(plugin, "n");
  assert(!(await plugin.render(30, 100)).includes("Retained reply 80"));
  await plugin.event({ Key: { bare_key: "Down", key_modifiers: [] } });
  await plugin.advance(1000);
  await answer(plugin, snapshot);
  assert(!(await plugin.render(30, 100)).includes("Retained reply 80"));
  await key(plugin, "/");
  await key(plugin, "Worker");
  await special(plugin, "Enter");
  let frame = await plugin.render(30, 100);
  assert(frame.includes("worker-run") && frame.includes("terminal_2"));
  await special(plugin, "Enter");
  assert((await plugin.render(30, 100)).includes("Worker retained answer"));
  await special(plugin, "Esc");
  frame = await plugin.render(30, 100);
  assert(frame.includes("conversation") && frame.includes("worker-run"));
});

for (const exit of ["Esc", "c", "Enter"] as const)
  test(`Roster reconciliation preserves search hits and their selection until ${exit}`, async () => {
    const { plugin, snapshot, messages, entry } = await fixture();
    messages.push(
      entry(2, "Worker second answer", "worker-run"),
      entry(3, "Worker third answer", "worker-run"),
    );
    await key(plugin, "a");
    await key(plugin, "j");
    await key(plugin, "/");
    await key(plugin, "Worker");
    await special(plugin, "Enter");
    await key(plugin, "j");
    await plugin.advance(1000);
    await answer(plugin, {
      ...snapshot,
      roster: {
        terminal_2: { ...snapshot.roster.terminal_2, state: "working", needs_input: false },
      },
    });
    let frame = await plugin.render(30, 100);
    assert(frame.includes("> Mother") && frame.includes("Search results"));
    assert(frame.includes("3 matches") && frame.includes("Worker second answer"));
    await key(plugin, "y");
    assertEquals(plugin.commands.at(-1), { CopyToClipboard: "Worker second answer" });
    await key(plugin, "j");
    await key(plugin, "y");
    assertEquals(plugin.commands.at(-1), { CopyToClipboard: "Worker retained answer" });
    await key(plugin, "k");
    if (exit === "Enter") {
      await special(plugin, "Enter");
      assert((await plugin.render(30, 100)).includes("Worker second answer"));
      await special(plugin, "Esc");
      frame = await plugin.render(30, 100);
      assert(
        frame.includes("> terminal_2") && frame.includes("conversation · terminal_2 · worker-run"),
      );
      assert(!frame.includes("attention only"));
    } else {
      if (exit === "Esc") await special(plugin, "Esc");
      else await key(plugin, "c");
      frame = await plugin.render(30, 100);
      assert(
        frame.includes("conversation · Mother · mother-run") && frame.includes("Newest reply"),
      );
      assert(!frame.includes("Worker second answer"));
    }
    assert(!frame.includes("Search results"));
  });

test("A failed previous-run fetch keeps the current page and retries the same run", async () => {
  const { plugin, archives, run, snapshot } = await fixture();
  archives[0] = run("mother-run", 1, "mother", {
    header: JSON.stringify({ id: "mother-run", previous_run_id: "previous-run" }),
  });
  archives.push(
    run("previous-run", 1, "previous", {
      header: JSON.stringify({ id: "previous-run", previous_run_id: "oldest-run" }),
    }),
    run("oldest-run", 1, "oldest"),
  );
  await plugin.advance(1000);
  await answer(plugin, snapshot);
  await special(plugin, "Tab");
  let attempts = 0;
  plugin.stub("DataFetch", ({ request }: { request: FetchRequest }) => {
    assertEquals(request.entity, "agent_run_entry");
    assert(matches({ run_id: "previous-run", revision: 1 }, request.filter));
    attempts += 1;
    if (attempts === 1) throw new Error("Archive offline");
    return {
      columns: ["run_id", "revision", "kind", "text"],
      rows: [
        [{ Text: "previous-run" }, { Int: 1 }, { Text: "message" }, { Text: "Previous answer" }],
      ],
      truncated: false,
      next_cursor: null,
    };
  });
  await key(plugin, "n");
  let frame = await plugin.render(30, 100);
  assert(frame.includes("Archive offline"));
  assert(frame.includes("conversation · Mother · mother-run") && frame.includes("Newest reply"));
  await key(plugin, "y");
  assertEquals(plugin.commands.at(-1), { CopyToClipboard: "Newest reply" });
  await key(plugin, "n");
  assertEquals(attempts, 2);
  frame = await plugin.render(30, 100);
  assert(
    frame.includes("conversation · Mother · previous-run") && frame.includes("Previous answer"),
  );
  assert(!frame.includes("Newest reply"));
});

test("Opening a search hit clears the attention filter and preserves the worker across refreshes", async () => {
  const { plugin, snapshot } = await fixture();
  const working = {
    ...snapshot,
    roster: {
      terminal_2: { ...snapshot.roster.terminal_2, state: "working", needs_input: false },
    },
  };
  await plugin.advance(1000);
  await answer(plugin, working);
  await key(plugin, "a");
  assert(!(await plugin.render(30, 100)).includes("terminal_2"));
  await key(plugin, "/");
  await key(plugin, "Worker");
  await special(plugin, "Enter");
  await special(plugin, "Enter");
  await plugin.advance(1000);
  await answer(plugin, working);
  assert((await plugin.render(30, 100)).includes("Worker retained answer"));
  await key(plugin, "f");
  assertEquals(plugin.commands.at(-1), { FocusTerminalPane: [2, true, false] });
  await special(plugin, "Esc");
  await plugin.advance(1000);
  await answer(plugin, working);
  const frame = await plugin.render(30, 100);
  assert(frame.includes("> terminal_2") && frame.includes("Worker retained answer"));
  assert(!frame.includes("attention only"));
  await key(plugin, "f");
  assertEquals(plugin.commands.at(-1), { FocusTerminalPane: [2, true, false] });
});

test("Search context retains its anchor when later messages exhaust the byte limit", async () => {
  const { plugin, messages, entry, snapshot } = await fixture();
  for (let revision = 3; revision < 60; revision += 1)
    messages.push(entry(revision, `Earlier reply ${revision}`));
  messages.push(entry(60, "Search anchor"));
  for (let revision = 61; revision <= 85; revision += 1)
    messages.push(entry(revision, "x".repeat(110_000)));
  await key(plugin, "/");
  await key(plugin, "Search anchor");
  await special(plugin, "Enter");
  await special(plugin, "Enter");
  assert((await plugin.render(30, 100)).includes("Search anchor"));
  await special(plugin, "Esc");
  await plugin.advance(1000);
  await answer(plugin, snapshot);
  await key(plugin, "y");
  assertEquals(plugin.commands.at(-1), { CopyToClipboard: "Search anchor" });
  const frame = await plugin.render(200, 100);
  assert(frame.includes("Earlier reply 11") && frame.includes("Search anchor"));
  assert(!frame.includes("Earlier reply 10"));
  await special(plugin, "Enter");
  assert((await plugin.render(30, 100)).includes("Search anchor"));
  await special(plugin, "Esc");
  await key(plugin, "n");
  await key(plugin, "y");
  assertEquals(plugin.commands.at(-1), { CopyToClipboard: "Earlier reply 10" });
});

test("An unavailable search context keeps the hit selected and shows the archive error", async () => {
  const { plugin } = await fixture();
  await key(plugin, "/");
  await key(plugin, "Worker");
  await special(plugin, "Enter");
  plugin.stub("DataFetch", () => {
    throw new Error("Archive offline");
  });
  await special(plugin, "Enter");
  await special(plugin, "Esc");
  const frame = await plugin.render(30, 100);
  assert(frame.includes("Archive offline") && frame.includes("Worker retained answer"));
  await key(plugin, "y");
  assertEquals(plugin.commands.at(-1), { CopyToClipboard: "Worker retained answer" });
});

test("Short layout and mouse keep roster and full-screen history reachable", async () => {
  const { plugin } = await fixture();
  let frame = await plugin.render(10, 45);
  assert(frame.includes("full-screen history"));
  const row = frame.split("\n").findIndex((line) => line.includes("terminal_2"));
  assert(row >= 0);
  await plugin.event({ Mouse: { LeftClick: [row, 2] } });
  await key(plugin, "f");
  assert(JSON.stringify(plugin.commands).includes('"FocusTerminalPane":[2,true,false]'));
  await special(plugin, "Tab");
  frame = await plugin.render(10, 45);
  assert(frame.includes("Worker retained answer"));
});

test("Warning headers leave short timelines and readers usable", async () => {
  const { plugin, snapshot } = await fixture();
  await plugin.advance(1000);
  await answer(plugin, {
    ...snapshot,
    observation_ready: false,
    delivery_blocked: "Waiting for lifecycle setup before delivering the next assignment.",
    gap: true,
  });
  let frame = await plugin.render(10, 45);
  assert(frame.includes("w: all warnings"));
  assert(frame.includes("full-screen history"));
  await special(plugin, "Tab");
  frame = await plugin.render(10, 45);
  assert(frame.includes("Newest reply") && frame.includes("Enter: read"));
  await special(plugin, "Enter");
  frame = await plugin.render(10, 45);
  assert(frame.includes("Newest reply") && frame.includes("Esc: timeline"));
  await special(plugin, "Esc");
  await key(plugin, "w");
  frame = await plugin.render(10, 45);
  assert(frame.includes("Supervision:") && frame.includes("Delivery held:"));
  await special(plugin, "End");
  frame = await plugin.render(10, 45);
  assert(frame.includes("Retention gap") && frame.includes("Esc: timeline"));
  await special(plugin, "Esc");
  assert((await plugin.render(10, 45)).includes("Newest reply"));
});

test("Receipt overflow stays readable across resizing", async () => {
  const { plugin, snapshot } = await fixture();
  await plugin.advance(1000);
  await answer(plugin, {
    ...snapshot,
    receipts: Array.from({ length: 30 }, (_, index) => ({
      submission: `pending-${index}`,
      settled: false,
      response: null,
    })),
  });
  await special(plugin, "Tab");
  for (const [rows, columns] of [
    [30, 100],
    [10, 45],
    [5, 45],
    [10, 24],
  ]) {
    const frame = await plugin.render(rows, columns);
    assert(frame.includes("Newest reply") && frame.includes("Tab: roster"), frame);
    assertEquals(frame.split("\n").length, rows);
  }
  await key(plugin, "w");
  await plugin.render(10, 45);
  await special(plugin, "End");
  assert((await plugin.render(10, 45)).includes("pending-29"));
  assert((await plugin.render(30, 100)).includes("pending-29"));
  await special(plugin, "Esc");
  assert((await plugin.render(10, 45)).includes("Newest reply"));
});

test("Refresh reconciles selection with the attention-filtered roster", async () => {
  const { plugin, snapshot } = await fixture();
  await key(plugin, "a");
  await key(plugin, "j");
  await plugin.advance(1000);
  await answer(plugin, snapshot);
  let frame = await plugin.render(30, 100);
  assert(frame.includes("> terminal_2") && frame.includes("Worker retained answer"));
  await plugin.advance(1000);
  await answer(plugin, {
    ...snapshot,
    roster: {
      terminal_2: { ...snapshot.roster.terminal_2, state: "working", needs_input: false },
    },
  });
  frame = await plugin.render(30, 100);
  assert(frame.includes("> Mother") && frame.includes("Newest reply"));
  assert(!frame.includes("terminal_2") && !frame.includes("Worker retained answer"));
  await key(plugin, "f");
  assertEquals(plugin.commands.at(-1), { FocusTerminalPane: [1, true, false] });
});

test("Refresh closes the reader only when the selected agent changes", async () => {
  const { plugin, snapshot } = await fixture();
  await key(plugin, "a");
  await key(plugin, "j");
  await special(plugin, "Tab");
  await special(plugin, "Enter");
  const reader = await plugin.render(30, 100);
  assert(reader.includes("Worker retained answer") && reader.includes("Esc: timeline"));
  await plugin.advance(1000);
  await answer(plugin, snapshot);
  assertEquals(await plugin.render(30, 100), reader);
  await key(plugin, "f");
  assertEquals(plugin.commands.at(-1), { FocusTerminalPane: [2, true, false] });
  await plugin.advance(1000);
  await answer(plugin, {
    ...snapshot,
    roster: {
      terminal_2: { ...snapshot.roster.terminal_2, state: "working", needs_input: false },
    },
  });
  const frame = await plugin.render(30, 100);
  assert(frame.includes("> Mother") && frame.includes("Newest reply"));
  assert(!frame.includes("Worker retained answer") && !frame.includes("Esc: timeline"));
  await key(plugin, "f");
  assertEquals(plugin.commands.at(-1), { FocusTerminalPane: [1, true, false] });
  await special(plugin, "Esc");
  assert((await plugin.render(30, 100)).includes("Newest reply"));
});

test("Recovery confirmation owns keys and keeps its original revision", async () => {
  const { plugin, snapshot } = await fixture();
  const pending = {
    ...snapshot,
    receipts: [
      { submission: "missing", settled: false, response: { outcome: "status_unavailable" } },
    ],
  };
  await plugin.advance(1000);
  await answer(plugin, pending);
  await key(plugin, "r");
  await plugin.render(10, 100);
  const count = plugin.commands.length;
  await key(plugin, "p");
  await key(plugin, "f");
  await key(plugin, "/");
  await key(plugin, "j");
  assertEquals(plugin.commands.length, count);
  await plugin.advance(1000);
  await answer(plugin, { ...pending, checkpoint: { revision: 99, value: null } });
  await key(plugin, "y");
  assertEquals(JSON.parse(requests(plugin).at(-1)?.request ?? "{}"), {
    Mother: { Resolve: { submission_id: "missing", expect_rev: 3 } },
  });
});

test("Archive errors leave controls usable and obsolete overview replies keep the chosen worker", async () => {
  const { plugin, snapshot } = await fixture();
  await plugin.advance(1000);
  await key(plugin, "j");
  await answer(plugin, snapshot);
  assert((await plugin.render(30, 100)).includes("Worker retained answer"));
  plugin.stub("DataFetch", () => {
    throw new Error("archive offline");
  });
  await special(plugin, "Tab");
  await special(plugin, "End");
  assert((await plugin.render(30, 100)).includes("Archive unavailable"));
  await key(plugin, "p");
  assertEquals(JSON.parse(requests(plugin).at(-1)?.request ?? "{}"), { Mother: "Pause" });
});

test("Journal identities retain departed workers and activity has a source reader", async () => {
  const { plugin, snapshot } = await fixture();
  await plugin.advance(1000);
  const record = {
    journal_cursor: 4,
    event_type: "pane_closed",
    pane_id: "terminal_2",
    conversation_id: "worker",
    observed_at_ms: 1000,
    reason: "Worker departed",
    state: "waiting",
    needs_input: true,
  };
  await answer(plugin, {
    ...snapshot,
    roster: {},
    recent: { ...snapshot.recent, records: [record], retention_gap: true },
  });
  await key(plugin, "j");
  let frame = await plugin.render(30, 100);
  assert(
    frame.includes("(retained)") &&
      frame.includes("Worker retained answer") &&
      frame.includes("Retention gap"),
  );
  const count = plugin.commands.length;
  await key(plugin, "f");
  assertEquals(plugin.commands.length, count);
  await key(plugin, "h");
  await answer(plugin, { ...snapshot.recent, records: [record] });
  frame = await plugin.render(30, 100);
  assert(
    frame.includes("pane closed") &&
      frame.includes("1970-01-01") &&
      !frame.includes("journal_cursor"),
  );
  await special(plugin, "Enter");
  await key(plugin, "v");
  frame = await plugin.render(30, 100);
  assert(frame.includes("journal_cursor") && frame.includes('"needs_input": true'));
});

for (const [event, departedState] of [
  ["pane_closed", "closed"],
  ["pane_replaced", "replaced"],
  ["pane_transferred", "transferred"],
  ["conversation_replaced", "replaced"],
])
  for (const state of ["waiting", "needs_input"])
    test(`${event} clears retained ${state} attention without hiding a live replacement`, async () => {
      const { plugin, snapshot } = await fixture();
      await plugin.advance(1000);
      await answer(plugin, {
        ...snapshot,
        roster: {},
        recent: {
          ...snapshot.recent,
          records: [
            {
              journal_cursor: 20,
              event_type: event,
              pane_id: "terminal_2",
              conversation_id: "worker",
              state,
              needs_input: true,
            },
          ],
        },
      });
      await key(plugin, "j");
      let frame = await plugin.render(30, 100);
      assert(frame.includes(`${departedState} (retained)`));
      assert(frame.includes("Worker retained answer"));
      assert(!frame.includes(`!${state}`));
      await key(plugin, "a");
      assert(!(await plugin.render(30, 100)).includes("terminal_2"));
      await plugin.advance(1000);
      await answer(plugin, {
        ...snapshot,
        roster: {
          terminal_2: { ...snapshot.roster.terminal_2, conversation_id: "replacement" },
        },
      });
      frame = await plugin.render(30, 100);
      assert(frame.includes("terminal_2") && frame.includes("!waiting"));
      assert(!frame.includes("(retained)"));
    });

test("Older journal pages cannot overwrite newer retained or live worker state", async () => {
  const { plugin, snapshot } = await fixture();
  const closed = {
    journal_cursor: 20,
    event_type: "pane_closed",
    pane_id: "terminal_2",
    conversation_id: "worker",
    state: "waiting",
    needs_input: true,
  };
  await plugin.advance(1000);
  await answer(plugin, {
    ...snapshot,
    roster: {},
    recent: { ...snapshot.recent, records: [closed] },
  });
  await key(plugin, "h");
  await answer(plugin, { ...snapshot.recent, records: [closed], next_cursor: 20, high_water: 20 });
  await key(plugin, "n");
  await answer(plugin, {
    ...snapshot.recent,
    records: [{ ...closed, journal_cursor: 10, event_type: "needs_input", state: "needs_input" }],
    high_water: 20,
  });
  await special(plugin, "Esc");
  await key(plugin, "c");
  let frame = await plugin.render(30, 100);
  assert(frame.includes("closed (retained)") && !frame.includes("!needs_input"));
  await key(plugin, "a");
  assert(!(await plugin.render(30, 100)).includes("terminal_2"));
  await plugin.advance(1000);
  await answer(plugin, snapshot);
  frame = await plugin.render(30, 100);
  assert(frame.includes("terminal_2") && frame.includes("!waiting"));
});

test("Search shows its limit and errors; empty archives stay explicit", async () => {
  const { plugin, messages, entry } = await fixture();
  for (let revision = 3; revision < 110; revision += 1)
    messages.push(entry(revision, "matching retained reply"));
  await key(plugin, "/");
  await key(plugin, "matching");
  await special(plugin, "Enter");
  assert((await plugin.render(30, 140)).includes("100-result limit"));
  plugin.stub("DataFetch", () => {
    throw new Error("index unavailable");
  });
  await key(plugin, "/");
  await key(plugin, "matching");
  await special(plugin, "Enter");
  assert((await plugin.render(30, 140)).includes("Search failed"));
});

test("Search previews the matching passage inside a long message", async () => {
  const { plugin, messages, entry } = await fixture();
  messages.push(entry(3, "Earlier text ".repeat(40) + "needle passage at the end"));
  await key(plugin, "/");
  await key(plugin, "needle");
  await special(plugin, "Enter");
  assert((await plugin.render(30, 100)).includes("needle passage at the end"));
});

test("Project headings keep the selected worker visible and narrow rows retain status", async () => {
  const { plugin, snapshot } = await fixture();
  const roster: Record<string, unknown> = {};
  for (let pane = 2; pane < 10; pane += 1)
    roster[`terminal_${pane}`] = {
      conversation_id: `worker-${pane}`,
      kind: "codex",
      state: "waiting",
      projects: [`project-${pane}`],
    };
  await plugin.advance(1000);
  await answer(plugin, { ...snapshot, roster });
  for (let index = 0; index < 8; index += 1) await key(plugin, "j");
  assert((await plugin.render(20, 80)).includes("> terminal_9"));
  assert((await plugin.render(10, 24)).includes("waiting"));
});
