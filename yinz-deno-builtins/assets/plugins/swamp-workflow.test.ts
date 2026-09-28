import { test, loadPlugin, assert, assertEquals } from "yinz:testing";
import type { BareKey, KeyModifier } from "yinz:plugin";
test("swamp-workflow starts unattached and supports scroll keys", async () => {
  const p = await loadPlugin("./swamp-workflow.ts", {
    replies: { SetSelfMouseSelectionSupport: null },
  });
  assert((await p.render()).includes("No run attached"));
  await p.event({ Key: { bare_key: "End", key_modifiers: [] } });
  await p.event({ Key: { bare_key: "Home", key_modifiers: [] } });
  assert((await p.render()).includes("No run attached"));
  await p.advance(2000);
  assertEquals(
    p.commands.filter((c) => typeof c === "object" && c !== null && "MessageToPlugin" in c).length,
    0,
  );
});
test("swamp-workflow attaches, renders a question and shows answer errors", async () => {
  const p = await loadPlugin("./swamp-workflow.ts", {
    replies: {
      SetSelfMouseSelectionSupport: null,
      GetPluginIds: { plugin_id: 1 },
      SetPaneSubtitle: null,
      MessageToPlugin: null,
      SubscribePaneOutput: null,
      ShowSelf: null,
    },
  });
  await p.pipe({ name: "show", payload: JSON.stringify({ directory: "/fixture", run: "run-1" }) });
  async function reply(result: unknown, error?: string) {
    const commands = p.commands as { MessageToPlugin?: { message_payload: string } }[];
    const request = JSON.parse(
      commands.filter((c) => c.MessageToPlugin).at(-1)!.MessageToPlugin!.message_payload,
    );
    await p.pipe({
      name: "yinz_rpc_reply",
      source: { Plugin: 2 },
      payload: JSON.stringify({
        version: 1,
        call_id: request.call_id,
        ...(error ? { error } : { result }),
      }),
    });
    return request;
  }
  await reply({
    run: "run-1",
    root: "/fixture",
    history: { status: "suspended" },
    questions: {
      results: [
        {
          model: "question",
          version: 1,
          attributes: {
            run: "run-1",
            question: "q",
            state: "pending",
            kind: "choice",
            prompt: "Continue the fixture?",
            answers: ["yes", "no"],
          },
        },
      ],
    },
  });
  assert((await p.render()).includes("Continue the fixture?"));
  await p.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
  const answer = await reply(null, "fixture answer rejected");
  assertEquals(answer.payload.method, "workflow_answer");
  assert((await p.render()).includes("fixture answer rejected"));
});

async function attached(snapshot: unknown) {
  const p = await loadPlugin("./swamp-workflow.ts", {
    replies: {
      SetSelfMouseSelectionSupport: null,
      GetPluginIds: { plugin_id: 1 },
      SetPaneSubtitle: null,
      MessageToPlugin: null,
      SubscribePaneOutput: null,
      ShowSelf: null,
    },
  });
  const request = () => {
    const commands = p.commands as { MessageToPlugin?: { message_payload: string } }[];
    return JSON.parse(
      commands.filter((c) => c.MessageToPlugin).at(-1)!.MessageToPlugin!.message_payload,
    );
  };
  const reply = async (result: unknown, progress = false) => {
    const sent = request();
    await p.pipe({
      name: progress ? "yinz_rpc_progress" : "yinz_rpc_reply",
      source: { Plugin: 2 },
      payload: JSON.stringify({
        version: 1,
        call_id: sent.call_id,
        ...(progress ? { payload: result } : { result }),
      }),
    });
    return sent;
  };
  const key = async (bare_key: BareKey, key_modifiers: KeyModifier[] = []) => {
    await p.event({ Key: { bare_key, key_modifiers } });
  };
  await p.pipe({ name: "show", payload: JSON.stringify({ directory: "/fixture", run: "parent" }) });
  await reply(snapshot);
  return { p, request, reply, key };
}

test("cancel targets the active continuation and reports an already finished target", async () => {
  const { p, request, reply, key } = await attached({
    run: "parent",
    root: "/fixture",
    history: { id: "parent", status: "succeeded" },
    children: [{ id: "child", workflowName: "review", status: "running" }],
  });
  await key("Enter");
  assertEquals(request().payload, {
    method: "workflow_cancel",
    directory: "/fixture",
    run: "child",
  });
  await reply({ operations: [], alreadyFinished: true });
  assert((await p.render()).includes("Run is no longer running"));
});

test("multiple active runs have explicit cancellation choices", async () => {
  const { p, request, key } = await attached({
    run: "parent",
    root: "/fixture",
    history: { id: "parent", status: "succeeded" },
    children: [
      { id: "review-a", workflowName: "review", status: "running" },
      { id: "tests-b", workflowName: "tests", status: "running" },
    ],
  });
  const frame = await p.render(24, 160);
  assert(frame.includes("Cancel review (review-a)"));
  assert(frame.includes("Cancel tests (tests-b)"));
  await key("Tab");
  await key("Enter");
  assertEquals(request().payload.run, "tests-b");
});

test("a finished cancellation target never silently retargets the next run", async () => {
  const snapshot = (id: string) => ({
    run: "parent",
    root: "/fixture",
    history: { id: "parent", status: "succeeded" },
    children: [{ id, workflowName: "review", status: "running" }],
  });
  const { p, request, reply, key } = await attached(snapshot("first"));
  await p.render();
  await p.pipe({ name: "workflow-changed" });
  await reply(snapshot("second"));
  // An Enter intended for the old visible target must not cancel the replacement.
  await key("Enter");
  assertEquals(request().payload.method, "workflow_show");
  await p.render();
  await key("Tab");
  await key("Enter");
  assertEquals(request().payload.run, "second");
});

test("recorded answers expose recovery and visible admission progress without another answer", async () => {
  const { p, request, reply, key } = await attached({
    run: "parent",
    root: "/fixture",
    history: { id: "parent", status: "suspended" },
    questions: {
      results: [
        {
          model: "question-model",
          version: 2,
          attributes: {
            run: "parent",
            question: "choose",
            state: "recorded",
            kind: "choice",
            prompt: "Continue?",
            answers: ["yes", "no"],
            answer: "yes",
          },
        },
      ],
    },
  });
  assert((await p.render()).includes("Recover saved answer"));
  await key("Enter");
  assertEquals(request().payload, {
    method: "workflow_recover",
    directory: "/fixture",
    model: "question-model",
    inputs: { run: "parent", question: "choose" },
  });
  await reply({ status: "waiting" }, true);
  await p.advance(180000);
  assert((await p.render(24, 160)).includes("Waiting for active workflows"));
  await reply({ status: "reloading" }, true);
  assert((await p.render(24, 160)).includes("Loading updated extensions"));
  await reply({ status: "ready" }, true);
  assert((await p.render(24, 160)).includes("Delivering answer"));
  await reply({});
  assert((await p.render(24, 160)).includes("Recorded answer delivered"));
});

test("an uncertain answer stays blocked until its recorded state can be recovered", async () => {
  const snapshot = {
    run: "parent",
    root: "/fixture",
    history: { id: "parent", status: "suspended" },
    questions: {
      results: [
        {
          model: "question-model",
          version: 1,
          attributes: {
            run: "parent",
            question: "choose",
            state: "pending",
            kind: "choice",
            prompt: "Continue?",
            answers: ["yes", "no"],
            answer: null as string | null,
          },
        },
      ],
    },
  };
  const { p, request, reply, key } = await attached(snapshot);
  await key("Enter");
  assertEquals(request().payload.method, "workflow_answer");
  await p.advance(90000);
  assertEquals(request().payload.method, "workflow_show");
  await key("Enter");
  assertEquals(request().payload.method, "workflow_show");
  await reply(snapshot);
  await key("Enter");
  assertEquals(request().payload.method, "workflow_show");
  assert((await p.render(24, 160)).includes("Answer delivery is uncertain"));
  snapshot.questions.results[0]!.version = 2;
  await p.pipe({ name: "workflow-changed" });
  await reply(snapshot);
  await key("Enter");
  assertEquals(request().payload.method, "workflow_show");
  snapshot.questions.results[0]!.version = 3;
  snapshot.questions.results[0]!.attributes.state = "recorded";
  snapshot.questions.results[0]!.attributes.answer = "yes";
  await p.pipe({ name: "workflow-changed" });
  await reply(snapshot);
  assert((await p.render(24, 160)).includes("Recover saved answer"));
  await p.event({ Mouse: { LeftClick: [23, 2] } });
  await p.event({ Mouse: { Release: [23, 2] } });
  assertEquals(request().payload.method, "workflow_recover");
});

test("launch errors and failed attached runs stay visible without details", async () => {
  const { p, reply } = await attached({
    run: "parent",
    root: "/fixture",
    history: { id: "parent", status: "failed" },
    children: [{ id: "child", status: "succeeded" }],
    problem: "Workflow validation failed: missing model",
    logs: "Hidden execution details",
  });
  const frame = await p.render(24, 160);
  assert(frame.includes("Workflow validation failed: missing model"));
  assert(!frame.includes("Result: succeeded") && !frame.includes("Hidden execution details"));
  await p.pipe({ name: "workflow-changed" });
  await reply({
    run: "parent",
    root: "/fixture",
    history: { id: "parent", status: "failed" },
    children: [{ id: "child", status: "succeeded" }],
  });
  assert((await p.render()).includes("Attached run: failed"));
  await p.pipe({ name: "workflow-changed" });
  await reply({
    run: "parent",
    root: "/fixture",
    history: { id: "parent", status: "failed" },
    children: [{ id: "child", status: "running" }],
  });
  assert((await p.render(8, 80)).includes("Attached run: failed"));
});

test("summary stays pinned, merge is disabled, and details and the entire question remain readable", async () => {
  const { p, request, key } = await attached({
    run: "parent",
    root: "/fixture",
    history: { id: "parent", workflowName: "review", status: "suspended" },
    questions: {
      results: [
        {
          model: "question",
          version: 1,
          attributes: {
            run: "parent",
            question: "choose",
            state: "pending",
            kind: "choice",
            prompt: "Question start\n" + "Question detail\n".repeat(50) + "QUESTION END",
            answers: ["merge", "show-diff"],
          },
        },
      ],
    },
    outputs: {
      results: [
        {
          model: "checks",
          name: "checks-current",
          version: 1,
          attributes: {
            tests: "missing",
            codeReview: "current",
            ready: false,
            reason: "Run tests before merging.",
            evidence: {
              source: { branch: "topic", commit: "a" },
              target: { branch: "main", commit: "b" },
            },
          },
        },
      ],
    },
    logs: "INTERNAL LOG",
  });
  let frame = await p.render(24, 100);
  assert(frame.includes("topic → main") && frame.includes("Review: current · Tests: missing"));
  assert(frame.includes("Run tests before merging.") && !frame.includes("INTERNAL LOG"));
  // Disabled Merge is skipped: reverse-tab from Show diff reaches Details.
  await key("Tab", ["Shift"]);
  await key("Enter");
  await key("End");
  frame = await p.render(24, 100);
  assert(frame.includes("INTERNAL LOG") && frame.includes("topic → main"));
  await key("Tab", ["Shift"]);
  await key("Enter");
  await key("End");
  frame = await p.render(10, 24);
  assert(frame.includes("QUESTION END") && frame.includes("topic → main"));
  assert(!frame.includes("INTERNAL LOG"));
  // Reading and navigating do not dispatch mutations.
  assertEquals(request().payload.method, "workflow_show");
});
