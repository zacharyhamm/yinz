import { HandoffSchema } from "../_lib/issue_driver.ts";
import { notifyHandoff } from "../_lib/issue_notifications.ts";
import { createState } from "../_lib/lifecycle.ts";
import type { Context } from "../_lib/runtime.ts";
import type { YinzRunner } from "../_lib/yinz_agent.ts";

function assert(value: unknown, message = "Assertion failed"): asserts value {
  if (!value) throw new Error(message);
}
const control = { session: "control session", paneId: "terminal_9" };
function fixture(decision: string) {
  const logs: Record<string, unknown>[] = [];
  const context: Context = {
    repoDir: "/repo",
    modelId: "issue",
    definition: { name: "issue" },
    globalArgs: { workspace: "default", itemId: 85 },
    signal: new AbortController().signal,
    logger: { info: (_message, values) => logs.push(values) },
    readResource: () => Promise.reject(new Error("Unexpected read")),
    writeResource: () => Promise.reject(new Error("Unexpected write")),
    createFileWriter: () => {
      throw new Error("Unexpected writer");
    },
  };
  const handoff = HandoffSchema.parse({
    id: "a6da10b0-c980-4b82-b920-000000000001",
    status: "current",
    decision,
    reason: "User input needed\nwith `quotes` and $(literal text)",
    createdAt: "2026-11-03T12:00:00.000Z",
    presentation: "Full evidence",
    state: createState({
      workspace: "default",
      itemId: 85,
      text: "Fix issue",
      note: "Acceptance criteria",
      digest: "d".repeat(64),
    }),
  });
  return { context, handoff, logs };
}
const human = new Set([
  "authorize_plan_revision",
  "approve_plan",
  "address_review_findings",
  "approve_review_fixes",
  "authorize_merge",
  "authorize_closeout",
  "blocked",
]);
for (const decision of HandoffSchema.shape.decision.options) {
  Deno.test(`notification eligibility: ${decision}`, async () => {
    const { context, handoff, logs } = fixture(decision);
    const calls: string[][] = [];
    const run: YinzRunner = (args, stdin, signal) => {
      calls.push(args);
      assert(stdin === undefined);
      assert(signal && !signal.aborted);
      return Promise.resolve({ code: 0, stdout: "", stderr: "" });
    };
    await notifyHandoff(context, handoff, control, run);
    assert(calls.length === (human.has(decision) ? 1 : 0));
    assert(logs.length === 0);
    if (calls.length) {
      assert(
        JSON.stringify(calls[0].slice(0, 7)) === JSON.stringify([
          "--session",
          control.session,
          "action",
          "notify",
          "--pane-id",
          control.paneId,
          "--",
        ]),
      );
      assert(calls[0][7].startsWith("default/85:"));
      assert(calls[0][7].includes("with `quotes` and $(literal text)"));
      assert(!calls[0][7].includes("\n"));
    }
  });
}
Deno.test("notification failures and missing control preserve the handoff", async () => {
  for (const mode of ["missing", "nonzero", "throws", "aborted"]) {
    const { context, handoff, logs } = fixture("blocked");
    const saved = JSON.stringify(handoff);
    let calls = 0;
    if (mode === "aborted") context.signal = AbortSignal.abort();
    await notifyHandoff(
      context,
      handoff,
      mode === "missing" ? undefined : control,
      (_args, _stdin, signal) => {
        calls++;
        signal?.throwIfAborted();
        if (mode === "throws") throw new Error("No executable");
        return Promise.resolve({ code: 1, stdout: "", stderr: "No session" });
      },
    );
    assert(calls === (mode === "missing" ? 0 : 1));
    assert(logs.length === 1);
    assert(JSON.stringify(handoff) === saved);
  }
});
Deno.test("notification delivery times out without losing the handoff", async () => {
  const { context, handoff, logs } = fixture("approve_plan");
  await notifyHandoff(
    context,
    handoff,
    control,
    (_args, _stdin, signal) =>
      new Promise((_resolve, reject) => {
        assert(signal);
        signal.addEventListener("abort", () => reject(signal.reason), {
          once: true,
        });
      }),
  );
  assert(logs.length === 1);
});
