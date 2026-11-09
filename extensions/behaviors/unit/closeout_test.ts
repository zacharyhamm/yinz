import {
  captureCloseoutOrigin,
  closeout,
  type CloseoutDependencies,
  closeoutPresentation,
  offerCloseout,
} from "../_lib/issue_closeout.ts";
import {
  type LifecycleState,
  rollbackState,
  StateSchema,
  transition,
} from "../_lib/lifecycle.ts";
import { type Context, GitOperationError } from "../_lib/runtime.ts";

function assert(value: unknown, message = "Assertion failed"): asserts value {
  if (!value) throw new Error(message);
}
function equal(a: unknown, b: unknown) {
  assert(
    JSON.stringify(a) === JSON.stringify(b),
    `${JSON.stringify(a)} != ${JSON.stringify(b)}`,
  );
}
async function rejects(fn: () => unknown, contains: string) {
  try {
    await fn();
  } catch (error) {
    assert(String(error).includes(contains), String(error));
    return;
  }
  throw new Error(`Expected ${contains}`);
}
const attemptId = "a11e72b8-2904-4c92-9abc-b3eb6a2f78af";
const head = "a".repeat(40);
const origin = {
  session: "original",
  paneId: "terminal_7",
  server_instance: "incarnation",
  provider: "codex",
  conversation: "conversation",
};
const initialFence = {
  server_instance: "incarnation",
  pane_id: "terminal_7",
  provider: "codex",
  conversation: "conversation",
  input_revision: 1,
  turn: "offering-turn",
};
function fixture() {
  const target = {
    worktree: "/tmp",
    branch: "issue-7",
    head,
    gitDir: "/control/.git/worktrees/issue-7",
    branchIdentity: "f".repeat(64),
    directoryIdentity: { device: "1", inode: "2" },
    registrationIdentity: { device: "1", inode: "3" },
    linkIdentity: { device: "1", inode: "4" },
    gitLink: "gitdir: /control/.git/worktrees/issue-7\n",
    owner: { issueKey: "yinz/7", worktree: "/tmp", branch: "issue-7" },
  };
  const receipt = {
    attemptId,
    originalCommit: head,
    rebasedCommit: head,
    baseCommit: "b".repeat(40),
    mergedCommit: head,
    worktree: "/tmp",
    branch: "issue-7",
    ciStatus: "passed" as const,
    mergedAt: "2026-11-09T12:00:00.000Z",
  };
  const initial: LifecycleState = {
    issue: {
      workspace: "yinz",
      itemId: 7,
      text: "Closeout",
      note: "",
      digest: "d".repeat(64),
    },
    phase: "done",
    planVersion: 0,
    workerWorktree: "/tmp",
    receipt,
    outcome: { outcomeMet: true, summary: "Delivered" },
  };
  const resources = new Map<string, unknown>([[
    "state",
    structuredClone(initial),
  ], ["controller-origin", { origin }]]);
  const events: string[] = [];
  let failWrite = "";
  let failGit = false;
  let lostGitAck = false;
  let gitRemoved = false;
  let worktreeRemoved = false;
  let failBranch = false;
  let reservedId: unknown;
  let closeLostAck = false;
  let paneAbsent = false;
  let failObserve = false;
  let snapshotReply: unknown;
  let otherPanes: unknown[] = [];
  let fence = { ...initialFence };
  let serial = 0;
  const gitRequests: Record<string, unknown>[] = [];
  const context: Context = {
    repoDir: "/control",
    modelId: "fixture",
    definition: { name: "issue-yinz-7" },
    globalArgs: { workspace: "yinz", itemId: 7 },
    signal: new AbortController().signal,
    logger: { info() {} },
    readResource: (name) =>
      Promise.resolve(structuredClone(resources.get(name) ?? null)),
    writeResource: (_spec, name, value) => {
      events.push(`write:${name.startsWith("event-") ? value.action : name}`);
      if (failWrite && (name === failWrite || value.action === failWrite)) {
        failWrite = "";
        throw new Error("Injected write interruption");
      }
      resources.set(name, structuredClone(value));
      return Promise.resolve({ name });
    },
    createFileWriter: () => ({
      writeStream: () => {
        throw new Error("No log expected");
      },
    }),
  };
  const deps: CloseoutDependencies = {
    id: () =>
      `a11e72b8-2904-4c92-9abc-${(++serial).toString(16).padStart(12, "0")}`,
    completed: () => Promise.resolve(),
    git: (_context, request) => {
      gitRequests.push(request);
      equal(request.failIfBusy, true);
      if (request.action === "receipt") {
        return Promise.resolve({
          ...receipt,
          issueKey: "yinz/7",
          issueDigest: initial.issue.digest,
          status: "merged",
          ci: { status: "passed", command: ["cargo", "x", "ci", "check"] },
        });
      }
      if (request.action === "closeout-preview") {
        events.push("observe:preview");
        if (worktreeRemoved) {
          throw new Error(
            "Closeout worktree is absent or not registered; cannot offer deletion",
          );
        }
        if (reservedId && request.cleanupId !== reservedId) {
          throw new Error("Ownership reserved for another cleanup");
        }
        return Promise.resolve(target);
      }
      assert(request.action === "closeout-cleanup");
      equal(request.target, target);
      if (failGit) throw new Error("Dirty worktree");
      if (reservedId) equal(request.cleanupId, reservedId);
      reservedId = request.cleanupId;
      if (!worktreeRemoved) {
        events.push("effect:worktree");
        worktreeRemoved = true;
      }
      if (failBranch) {
        throw new GitOperationError("Branch deletion failed", {
          closeout: {
            cleanupId: request.cleanupId,
            target,
            worktree: "removed",
            branch: "deleting",
          },
        });
      }
      if (!gitRemoved) {
        events.push("effect:branch");
        gitRemoved = true;
      }
      if (lostGitAck) {
        lostGitAck = false;
        throw new Error("Lost Git acknowledgement");
      }
      return Promise.resolve({
        cleanupId: request.cleanupId,
        target,
        worktree: "removed",
        branch: "removed",
      });
    },
    run: (args) => {
      if (args[2] !== "action") {
        equal(args.slice(0, 2), ["--session", "original"]);
      }
      let reply: unknown;
      if (args[2] === "action") reply = otherPanes;
      else if (args[3] === "closeout-support") {
        reply = {
          outcome: "ok",
          output: "agent-controller-closeout-v1",
          server_instance: fence.server_instance,
        };
      } else if (args[3] === "closeout-snapshot") {
        if (failObserve) throw new Error("Cannot observe server");
        reply = snapshotReply ?? {
          outcome: paneAbsent ? "target_not_found" : "ok",
          server_instance: fence.server_instance,
          ...(paneAbsent ? {} : { closeout_target: fence }),
        };
      } else {
        equal(args[3], "closeout-close");
        const state = StateSchema.parse(resources.get("state"));
        equal(state.closeout?.git, "complete");
        equal(state.closeout?.pane, "unknown");
        const lastWrite = events.filter((e) => e.startsWith("write:")).at(-1);
        equal(lastWrite, "write:state");
        equal(JSON.parse(args[5]), state.closeout?.fence);
        events.push("effect:pane");
        paneAbsent = true;
        if (closeLostAck) throw new Error("Lost pane acknowledgement");
        reply = { outcome: "ok", server_instance: fence.server_instance };
      }
      return Promise.resolve({
        code: 0,
        stdout: JSON.stringify(reply),
        stderr: "",
      });
    },
  };
  return {
    context,
    deps,
    resources,
    events,
    gitRequests,
    initial,
    state: () => StateSchema.parse(resources.get("state")),
    offer: () => closeout(context, {}, deps),
    accept: () =>
      closeout(context, {
        handoffId: StateSchema.parse(resources.get("state")).closeout!.offerId,
        decision: "accept",
        request: "Delete the offered resources and close this conversation",
      }, deps),
    failWrite: (value: string) => {
      failWrite = value;
    },
    failGit: (value: boolean) => {
      failGit = value;
    },
    failBranch: (value: boolean) => {
      failBranch = value;
    },
    snapshotReply: (value: unknown) => {
      snapshotReply = value;
    },
    loseGitAck: () => {
      lostGitAck = true;
    },
    losePaneAck: () => {
      closeLostAck = true;
    },
    paneAbsent: (value: boolean) => {
      paneAbsent = value;
    },
    observeFailure: (value: boolean) => {
      failObserve = value;
    },
    changeFence: (value: Partial<typeof fence>) => {
      fence = { ...fence, ...value };
    },
    otherPanes: (value: unknown[]) => {
      otherPanes = value;
    },
  };
}

Deno.test("closeout explicit accept refreshes accepting turn and persists Git before pane", async () => {
  const h = fixture();
  await h.offer();
  assert(!h.events.some((e) => e.startsWith("effect:")));
  h.changeFence({ input_revision: 2, turn: "accepting-turn" });
  await h.accept();
  equal(h.events.filter((e) => e.startsWith("effect:")), [
    "effect:worktree",
    "effect:branch",
    "effect:pane",
  ]);
  equal(h.state().closeout?.pane, "closed");
  equal(h.state().closeout?.fence.turn, "accepting-turn");
  equal(h.state().receipt, h.initial.receipt);
  equal(h.state().outcome, h.initial.outcome);
  await closeout(h.context, {}, h.deps);
  equal(h.events.filter((e) => e.startsWith("effect:")).length, 3);
});

function lockBusy(stage: string, progress?: unknown) {
  return new GitOperationError(
    `Optional closeout deferred during ${stage}: repository merge lock is busy; another lifecycle Git operation (possibly merge/CI) may be running. The completed outcome and recorded cleanup progress are unchanged. Let that operation finish, then retry closeout or resume-closeout.`,
    {
      code: "lock_busy",
      stage,
      lock: "repository merge",
      ...(progress ? { closeout: progress } : {}),
    },
  );
}

Deno.test("closeout receipt and preview busy defer without creating an offer or authority", async () => {
  for (
    const [action, stage] of [
      ["receipt", "merge-receipt validation"],
      ["closeout-preview", "cleanup preview"],
    ]
  ) {
    const h = fixture();
    const git = h.deps.git;
    h.deps.git = (context, request) => {
      equal(request.failIfBusy, true);
      if (request.action === action) throw lockBusy(stage);
      return git(context, request);
    };
    await rejects(h.offer, `${stage}: repository merge lock is busy`);
    await rejects(h.offer, "retry closeout or resume-closeout");
    equal(h.state(), StateSchema.parse(h.initial));
    equal(h.events, []);
    assert(!h.resources.has("handoff"));
    h.deps.git = git;
    await h.offer();
    equal(h.state().closeout?.decision, "offered");
    assert(!h.events.some((event) => event.startsWith("effect:")));
  }
});

Deno.test("closeout cleanup busy retains initial partial and unknown progress then resumes the same operation", async () => {
  for (const history of ["initial", "partial", "unknown"] as const) {
    const h = fixture();
    await h.offer();
    const git = h.deps.git;
    if (history === "partial") {
      h.failBranch(true);
      await rejects(h.accept, "Branch deletion failed");
      h.failBranch(false);
    } else if (history === "unknown") {
      h.loseGitAck();
      await rejects(h.accept, "Lost Git acknowledgement");
    }
    const before = h.state();
    const effects = h.events.filter((event) => event.startsWith("effect:"));
    h.deps.git = (context, request) => {
      equal(request.failIfBusy, true);
      if (request.action === "closeout-cleanup") {
        equal(request.cleanupId, before.closeout!.id);
        equal(request.target, before.closeout!.target);
        throw lockBusy("Git cleanup", before.closeout!.gitResult);
      }
      return git(context, request);
    };
    await rejects(
      history === "initial" ? h.accept : h.offer,
      "Git cleanup: repository merge lock is busy",
    );
    const busy = h.state();
    equal(busy.receipt, before.receipt);
    equal(busy.outcome, before.outcome);
    equal(busy.closeout!.id, before.closeout!.id);
    equal(busy.closeout!.target, before.closeout!.target);
    equal(busy.closeout!.gitResult, before.closeout!.gitResult);
    equal(busy.closeout!.decision, "accepted");
    equal(busy.closeout!.git, "pending");
    equal(busy.closeout!.pane, "pending");
    assert(busy.closeout!.authorizedRequest);
    if (history !== "initial") {
      equal(
        busy.closeout!.authorizedRequest,
        before.closeout!.authorizedRequest,
      );
      equal(busy.closeout!.fence, before.closeout!.fence);
    }
    equal(busy.closeout!.failure!.stage, "git");
    assert(busy.closeout!.failure!.message.includes("resume-closeout"));
    equal(h.events.filter((event) => event.startsWith("effect:")), effects);
    assert(!h.events.includes("write:closeout_pane_intent"));
    // A busy response without helper progress cannot erase saved partial facts.
    h.deps.git = (context, request) => {
      if (request.action === "closeout-cleanup") throw lockBusy("Git cleanup");
      return git(context, request);
    };
    await rejects(h.offer, "Git cleanup: repository merge lock is busy");
    equal(h.state().closeout!.gitResult, before.closeout!.gitResult);
    const saved = h.state();
    h.deps.git = () => {
      throw lockBusy("merge-receipt validation");
    };
    await rejects(h.offer, "merge-receipt validation");
    equal(h.state(), saved);
    h.deps.git = git;
    if (history === "partial") h.changeFence({ turn: "later-turn" });
    await h.offer();
    equal(h.state().closeout!.id, before.closeout!.id);
    if (history === "partial") {
      equal(h.state().closeout!.pane, "expired");
      equal(h.state().closeout!.decision, "offered");
      assert(!h.events.includes("effect:pane"));
      await h.accept();
    }
    equal(h.state().closeout!.git, "complete");
    equal(h.state().closeout!.pane, "closed");
    equal(h.events.filter((event) => event.startsWith("effect:")), [
      "effect:worktree",
      "effect:branch",
      "effect:pane",
    ]);
    assert(h.gitRequests.every((request) => request.failIfBusy === true));
  }
});
Deno.test("closeout decline retains completion and is terminal", async () => {
  const h = fixture();
  await h.offer();
  const c = h.state().closeout!;
  await closeout(h.context, {
    handoffId: c.offerId,
    decision: "decline",
    request: "Keep the resources",
  }, h.deps);
  await closeout(h.context, {}, h.deps);
  equal(h.state().closeout?.decision, "declined");
  equal(h.state().phase, "done");
  assert(!h.events.some((e) => e.startsWith("effect:")));
  await rejects(() => h.accept(), "declined");
});
Deno.test("closeout rejects stale offers receipts and replacement conversations", async () => {
  const h = fixture();
  await h.offer();
  await rejects(
    () =>
      closeout(h.context, {
        handoffId: crypto.randomUUID(),
        decision: "accept",
        request: "Yes",
      }, h.deps),
    "current exact",
  );
  h.changeFence({ conversation: "replacement" });
  await rejects(() => h.accept(), "changed");
  h.changeFence({ conversation: "conversation" });
  const state = h.state();
  state.receipt!.attemptId = crypto.randomUUID();
  h.resources.set("state", state);
  await rejects(() => h.accept(), "receipt no longer matches");
  assert(!h.events.some((e) => e.startsWith("effect:")));
});
Deno.test("closeout old states require explicit original target binding before consent", async () => {
  const h = fixture();
  h.resources.delete("controller-origin");
  await rejects(() => h.offer(), "identity unavailable");
  await rejects(
    () =>
      closeout(h.context, {
        decision: "accept",
        handoffId: crypto.randomUUID(),
        request: "Yes",
      }, h.deps),
    "Create and present",
  );
  await closeout(h.context, { bind: origin }, h.deps);
  equal(h.state().closeout?.origin, origin);
  assert(!h.events.some((e) => e.startsWith("effect:")));
});
Deno.test("closeout Git failure and lost acknowledgement resume without duplicate removal", async () => {
  const h = fixture();
  await h.offer();
  h.failGit(true);
  await rejects(() => h.accept(), "pane retained");
  equal(h.state().closeout?.failure?.stage, "git");
  assert(!h.events.some((e) => e.startsWith("effect:")));
  h.failGit(false);
  h.loseGitAck();
  await rejects(
    () => closeout(h.context, {}, h.deps),
    "Lost Git acknowledgement",
  );
  assert(!h.events.includes("effect:pane"));
  await closeout(h.context, {}, h.deps);
  equal(h.events.filter((e) => e.startsWith("effect:")), [
    "effect:worktree",
    "effect:branch",
    "effect:pane",
  ]);
});
Deno.test("closeout lost self-close reply reconciles absence from another invoking pane", async () => {
  const h = fixture();
  await h.offer();
  h.losePaneAck();
  await h.accept();
  equal(h.state().closeout?.pane, "unknown");
  // The recovery caller is deliberately absent from all saved target inputs.
  h.resources.set("driver", {
    agent: { control: { session: "recovery", paneId: "terminal_99" } },
  });
  await closeout(h.context, {}, h.deps);
  equal(h.state().closeout?.pane, "absent");
  equal(h.events.filter((e) => e === "effect:pane").length, 1);
});
Deno.test("closeout changed incarnation cannot reconcile absent original as success", async () => {
  const h = fixture();
  await h.offer();
  h.losePaneAck();
  await h.accept();
  h.changeFence({ server_instance: "replacement-server" });
  await closeout(h.context, {}, h.deps);
  equal(h.state().closeout?.pane, "expired");
  equal(h.state().closeout?.git, "complete");
  await rejects(() => h.accept(), "changed");
});
Deno.test("closeout expired accepting turn requires renewed exact offer consent", async () => {
  const h = fixture();
  await h.offer();
  h.failWrite("closeout_pane_intent");
  await rejects(() => h.accept(), "write interruption");
  const prior = h.state().closeout!.offerId;
  h.changeFence({ turn: "later-turn", input_revision: 9 });
  await closeout(h.context, {}, h.deps);
  equal(h.state().closeout?.pane, "expired");
  assert(h.state().closeout?.offerId !== prior);
  assert(!h.events.includes("effect:pane"));
  await h.accept();
  equal(h.state().closeout?.pane, "closed");
  equal(h.events.filter((e) => e === "effect:worktree").length, 1);
});
Deno.test("closeout unknown neighboring user blocks Git effects", async () => {
  for (
    const pane of [{ id: 9, is_plugin: false }, {
      id: 9,
      is_plugin: false,
      pane_cwd: "/tmp",
    }]
  ) {
    const h = fixture();
    await h.offer();
    h.otherPanes([pane]);
    await rejects(
      () => h.accept(),
      pane.pane_cwd ? "still uses" : "unknown working",
    );
    assert(!h.events.some((e) => e.startsWith("effect:")));
  }
});
Deno.test("closeout rollback preserves cleanup facts and invalidates unexecuted authority", async () => {
  const h = fixture();
  await h.offer();
  h.losePaneAck();
  await h.accept();
  const previous = h.state();
  const next = rollbackState(previous, "summarize", crypto.randomUUID());
  equal(next.receipt, previous.receipt);
  equal(next.closeout?.git, "complete");
  equal(next.closeout?.decision, "invalidated");
  h.resources.set("state", next);
  await rejects(
    () => closeout(h.context, {}, h.deps),
    "requires a confirmed merge",
  );
  equal(h.events.filter((e) => e === "effect:pane").length, 1);
});
Deno.test("closeout original binding is immutable across later placement", async () => {
  const h = fixture();
  h.resources.delete("controller-origin");
  await captureCloseoutOrigin(h.context, {
    session: "original",
    paneId: "terminal_7",
  }, h.deps);
  const first = structuredClone(h.resources.get("controller-origin"));
  await captureCloseoutOrigin(h.context, {
    session: "recovery",
    paneId: "terminal_99",
  }, h.deps);
  equal(h.resources.get("controller-origin"), first);
  await offerCloseout(h.context, h.state(), h.deps);
  equal(h.state().closeout?.origin, origin);
});

for (
  const boundary of [
    "handoff",
    "accept_closeout",
    "state",
    "closeout_git_complete",
    "closeout_pane_intent",
    "closeout_pane_closed",
    "closeout_pane_unknown",
  ]
) {
  Deno.test(`closeout interruption at ${boundary} recovers from saved progress without repeating effects`, async () => {
    const h = fixture();
    if (boundary === "handoff") {
      h.failWrite(boundary);
      await rejects(() => h.offer(), "write interruption");
      await h.offer();
    } else {
      await h.offer();
      h.failWrite(boundary);
      if (boundary === "closeout_pane_unknown") h.losePaneAck();
      try {
        await h.accept();
      } catch (error) {
        assert(String(error).includes("write interruption"), String(error));
      }
    }
    if (h.state().closeout!.decision === "offered") {
      await h.offer();
      await h.accept();
    } else await closeout(h.context, {}, h.deps);
    assert(["closed", "absent"].includes(h.state().closeout!.pane));
    equal(h.events.filter((e) => e === "effect:worktree").length, 1);
    equal(h.events.filter((e) => e === "effect:branch").length, 1);
    equal(h.events.filter((e) => e === "effect:pane").length, 1);
  });
}
Deno.test("closeout observation failure keeps completed Git and an actionable unconfirmed result", async () => {
  const h = fixture();
  await h.offer();
  h.losePaneAck();
  await h.accept();
  h.observeFailure(true);
  await closeout(h.context, {}, h.deps);
  equal(h.state().closeout?.pane, "unknown");
  equal(h.state().closeout?.git, "complete");
  assert(h.state().closeout?.failure?.message.includes("resume-closeout"));
  h.observeFailure(false);
  await closeout(h.context, {}, h.deps);
  equal(h.state().closeout?.pane, "absent");
  equal(h.events.filter((e) => e === "effect:pane").length, 1);
});

Deno.test("closeout inspects other known worker sessions without substituting their panes", async () => {
  const h = fixture();
  await h.offer();
  h.resources.set("driver", { agent: { session: "another" } });
  h.otherPanes([{ id: 7, is_plugin: false, pane_cwd: "/tmp" }]);
  await rejects(() => h.accept(), "another/terminal_7 still uses");
  assert(!h.events.some((e) => e.startsWith("effect:")));
});

Deno.test("closeout removed worktree blocks restart until an explicit new valid worktree selection", async () => {
  const h = fixture();
  await h.offer();
  await h.accept();
  const state = h.state();
  state.phase = "approved";
  state.planVersion = 1;
  state.plan = {
    version: 1,
    summary: "Reopened",
    steps: [{ id: "fix", description: "Fix", files: [] }],
    testingStrategy: "Focused",
  };
  state.approval = { planVersion: 1, request: "Implement the reopened issue" };
  state.implementation = { worktree: "/tmp", branch: "issue-7" };
  state.closeout!.decision = "invalidated";
  delete state.receipt;
  await rejects(
    () =>
      transition(state, "implement", { worktree: "/tmp", branch: "issue-7" }),
    "select a new valid worktree",
  );
  const next = transition(state, "implement", {
    worktree: "/tmp/new-valid-worktree",
    branch: "new-issue-7",
  });
  equal(next.workerWorktree, "/tmp/new-valid-worktree");
  equal(next.implementation?.branch, "new-issue-7");
  equal(next.closeout?.git, "complete");
});

Deno.test("closeout records correlated partial Git progress in Swamp while retaining its pane", async () => {
  for (const matching of [true, false]) {
    const h = fixture();
    await h.offer();
    const git = h.deps.git;
    h.deps.git = (context, request, ...rest) => {
      if (request.action !== "closeout-cleanup") {
        return git(context, request, ...rest);
      }
      throw new GitOperationError("Branch deletion failed", {
        closeout: {
          cleanupId: matching ? request.cleanupId : crypto.randomUUID(),
          target: request.target,
          worktree: "removed",
          branch: "deleting",
        },
      });
    };
    await rejects(() => h.accept(), "Branch deletion failed");
    equal(h.state().phase, "done");
    equal(h.state().closeout?.pane, "pending");
    equal(
      h.state().closeout?.gitResult?.worktree,
      matching ? "removed" : undefined,
    );
    equal(
      h.state().closeout?.gitResult?.branch,
      matching ? "deleting" : undefined,
    );
    assert(!h.events.includes("effect:pane"));
  }
});

Deno.test("closeout explicit legacy binding survives rollback without permitting a replacement origin", async () => {
  const h = fixture();
  h.resources.delete("controller-origin");
  await closeout(h.context, { bind: origin }, h.deps);
  const state = h.state();
  state.closeout!.decision = "invalidated";
  h.resources.set("state", state);
  await rejects(
    () =>
      closeout(
        h.context,
        { bind: { ...origin, paneId: "terminal_99" } },
        h.deps,
      ),
    "cannot replace",
  );
  await closeout(h.context, {}, h.deps);
  equal(h.state().closeout?.origin, origin);
  assert(!h.events.some((e) => e.startsWith("effect:")));
});

async function recomplete(h: ReturnType<typeof fixture>) {
  const rolled = rollbackState(h.state(), "summarize", crypto.randomUUID());
  h.resources.set("state", rolled);
  await rejects(() => h.offer(), "requires a confirmed merge");
  h.resources.set(
    "state",
    transition(rolled, "done", {
      outcomeMet: true,
      summary: "Corrected outcome",
    }),
  );
}

for (
  const interrupted of [
    "pane-pending",
    "git-partial",
    "git-unknown",
    "pane-unknown",
    "closed",
  ]
) {
  Deno.test(`closeout rollback re-completion recovers ${interrupted} with original operation and fresh authority`, async () => {
    const h = fixture();
    await h.offer();
    const firstOffer = h.state().closeout!.offerId;
    if (interrupted === "pane-pending") h.failWrite("closeout_pane_intent");
    if (interrupted === "git-partial") h.failBranch(true);
    if (interrupted === "git-unknown") h.loseGitAck();
    if (interrupted === "pane-unknown") h.losePaneAck();
    try {
      await h.accept();
    } catch (error) {
      assert(
        /write interruption|Branch deletion failed|Lost Git acknowledgement/
          .test(String(error)),
        String(error),
      );
    }
    const prior = h.state().closeout!;
    await recomplete(h);
    // summarize calls offerCloseout directly; exercise it as well as resume-closeout.
    await offerCloseout(h.context, h.state(), h.deps);
    const recovered = h.state().closeout!;
    equal(recovered.id, prior.id);
    equal(recovered.target, prior.target);
    equal(recovered.origin, prior.origin);
    equal(recovered.gitResult, prior.gitResult);
    equal(recovered.authorizedRequest, prior.authorizedRequest);
    equal(recovered.rollbackId, h.state().rollbackId);
    assert(recovered.offerId !== firstOffer);
    equal(h.events.filter((e) => e === "observe:preview").length, 1);
    if (["closed", "pane-unknown"].includes(interrupted)) {
      equal(recovered.pane, interrupted === "closed" ? "closed" : "absent");
      await h.offer();
    } else {
      equal(recovered.decision, "offered");
      const effects = h.events.filter((e) => e.startsWith("effect:"));
      await h.offer();
      equal(h.events.filter((e) => e.startsWith("effect:")), effects);
      await rejects(() =>
        closeout(h.context, {
          handoffId: firstOffer,
          decision: "accept",
          request: "Old approval",
        }, h.deps), "current exact");
      h.failBranch(false);
      await h.accept();
      equal(h.state().closeout?.pane, "closed");
    }
    equal(h.events.filter((e) => e === "effect:worktree").length, 1);
    equal(h.events.filter((e) => e === "effect:branch").length, 1);
    equal(h.events.filter((e) => e === "effect:pane").length, 1);
  });
}

Deno.test("closeout renewed pane-only offer can be declined terminally with Git facts intact", async () => {
  const h = fixture();
  await h.offer();
  h.failWrite("closeout_pane_intent");
  await rejects(() => h.accept(), "write interruption");
  const prior = h.state().closeout!;
  h.changeFence({ turn: "keep-conversation", input_revision: 10 });
  await h.offer();
  const current = h.state().closeout!;
  const text = closeoutPresentation(h.state());
  assert(text.includes("already removed"));
  assert(text.includes("Decline keeps that pane"));
  assert(!text.includes("Decline retains all resources"));
  await rejects(() =>
    closeout(h.context, {
      handoffId: prior.offerId,
      decision: "decline",
      request: "Keep pane",
    }, h.deps), "current exact");
  await closeout(h.context, {
    handoffId: current.offerId,
    decision: "decline",
    request: "Keep this conversation",
  }, h.deps);
  equal(h.state().closeout?.decision, "declined");
  equal(h.state().closeout?.gitResult, prior.gitResult);
  equal(h.state().closeout?.authorizedRequest, prior.authorizedRequest);
  const writes = h.events.length;
  await h.offer();
  await offerCloseout(h.context, h.state(), h.deps);
  equal(h.events.length, writes);
  assert(!h.events.includes("effect:pane"));
});

Deno.test("closeout partial rollback recovery decline preserves reservation and stops further effects", async () => {
  const h = fixture();
  await h.offer();
  h.failBranch(true);
  await rejects(() => h.accept(), "Branch deletion failed");
  const prior = h.state().closeout!;
  await recomplete(h);
  await h.offer();
  assert(
    closeoutPresentation(h.state()).includes(
      "unresolved ownership reservation",
    ),
  );
  await closeout(h.context, {
    handoffId: h.state().closeout!.offerId,
    decision: "decline",
    request: "Stop remaining cleanup",
  }, h.deps);
  h.failBranch(false);
  await h.offer();
  equal(h.state().closeout?.id, prior.id);
  equal(h.state().closeout?.gitResult, prior.gitResult);
  equal(h.state().closeout?.git, "pending");
  assert(
    closeoutPresentation(h.state()).includes(
      "worktree removed, branch deleting",
    ),
  );
  equal(h.events.filter((e) => e.startsWith("effect:")), ["effect:worktree"]);
});

Deno.test("closeout unused rollback offer gets new identity but executed changed targets cannot be rebound", async () => {
  const unused = fixture();
  await unused.offer();
  const first = unused.state().closeout!.id;
  await recomplete(unused);
  await unused.offer();
  assert(unused.state().closeout!.id !== first);
  equal(unused.state().closeoutHistory?.[0].id, first);
  for (const change of ["digest", "attempt", "target"]) {
    const h = fixture();
    await h.offer();
    h.loseGitAck();
    await rejects(() => h.accept(), "Lost Git acknowledgement");
    await recomplete(h);
    const state = h.state();
    // Tamper the historical binding, leaving the independently validated receipt unchanged.
    if (change === "digest") state.closeout!.issueDigest = "changed";
    if (change === "attempt") state.closeout!.attemptId = crypto.randomUUID();
    if (change === "target") state.closeout!.target.branch = "replacement";
    h.resources.set("state", state);
    await rejects(() => h.offer(), "historical progress");
    equal(h.state(), state);
    equal(h.events.filter((e) => e === "observe:preview").length, 1);
    assert(!h.events.includes("effect:pane"));
  }
});

for (const outcome of ["rejected", "target_not_found"]) {
  Deno.test(`closeout ${outcome} snapshot preserves actionable diagnostic during capture and offer`, async () => {
    const h = fixture();
    h.resources.delete("controller-origin");
    const message = "Controller closeout would affect other panes";
    h.snapshotReply({ outcome, message, server_instance: "incarnation" });
    await captureCloseoutOrigin(h.context, {
      session: "original",
      paneId: "terminal_7",
    }, h.deps);
    assert(
      JSON.stringify(h.resources.get("controller-origin")).includes(message),
    );
    await rejects(() => h.offer(), message);
    h.resources.set("controller-origin", { origin });
    await rejects(() => h.offer(), message);
    equal(
      h.events.filter((e) =>
        e.startsWith("effect:") || e === "observe:preview"
      ),
      [],
    );
  });
}
Deno.test("closeout malformed successful snapshot fails closed", async () => {
  const h = fixture();
  h.snapshotReply({ outcome: "ok", server_instance: "incarnation" });
  await rejects(() => h.offer(), "expected object");
  assert(
    !h.events.some((e) => e.startsWith("effect:") || e === "observe:preview"),
  );
});

Deno.test("closeout recovery offer can reconcile delayed exact absence without renewed consent", async () => {
  const h = fixture();
  await h.offer();
  h.losePaneAck();
  await h.accept();
  h.paneAbsent(false);
  await recomplete(h);
  await h.offer();
  equal(h.state().closeout?.decision, "offered");
  equal(h.state().closeout?.pane, "unknown");
  h.paneAbsent(true);
  await h.offer();
  equal(h.state().closeout?.pane, "absent");
  equal(h.events.filter((e) => e === "effect:pane").length, 1);
});

Deno.test("closeout legacy accepted expired state renews a declinable offer without losing authorization", async () => {
  const h = fixture();
  await h.offer();
  h.failWrite("closeout_pane_intent");
  await rejects(() => h.accept(), "write interruption");
  const legacy = h.state();
  legacy.closeout!.pane = "expired";
  h.resources.set("state", legacy);
  await h.offer();
  const renewed = h.state().closeout!;
  equal(renewed.decision, "offered");
  assert(renewed.offerId !== legacy.closeout!.offerId);
  equal(renewed.authorizedRequest, legacy.closeout!.authorizedRequest);
  await closeout(h.context, {
    handoffId: renewed.offerId,
    decision: "decline",
    request: "Keep the original pane",
  }, h.deps);
  equal(h.state().closeout?.decision, "declined");
  assert(!h.events.includes("effect:pane"));
});
