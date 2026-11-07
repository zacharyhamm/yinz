import { join } from "node:path";
import { behavior } from "../issue_lifecycle.ts";
import {
  createState,
  digestIssue,
  type LifecycleState,
  transition,
} from "../_lib/lifecycle.ts";
import {
  type Context,
  EventSchema,
  gitOperation,
  issueName,
  loadState,
  normalizeReceipt,
  saveState,
  withIssue,
} from "../_lib/runtime.ts";

function assert(
  condition: unknown,
  message = "Assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}
function equal(actual: unknown, expected: unknown): void {
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    `${JSON.stringify(actual)} != ${JSON.stringify(expected)}`,
  );
}
async function rejects(
  fn: () => unknown,
  message: string,
): Promise<void> {
  try {
    await fn();
  } catch (error) {
    assert(
      error instanceof Error && error.message.includes(message),
      String(error),
    );
    return;
  }
  throw new Error(`Expected rejection containing: ${message}`);
}

const issue = {
  workspace: "default",
  itemId: 85,
  text: "Fix behavior",
  note: "Acceptance criteria",
  digest: "d".repeat(64),
};

function fakeContext(repoDir: string, initial: unknown = null) {
  const resources = new Map<string, unknown>([["state", initial]]);
  const writes: {
    spec: string;
    name: string;
    value: Record<string, unknown>;
  }[] = [];
  const files = new Map<string, { spec: string; text: string }>();
  const controller = new AbortController();
  const context: Context = {
    repoDir,
    modelId: "test-behavior",
    definition: { name: "issue-default-85" },
    globalArgs: { workspace: "default", itemId: 85 },
    signal: controller.signal,
    logger: { info() {} },
    readResource: (name) => Promise.resolve(resources.get(name) ?? null),
    writeResource: (spec, name, value) => {
      writes.push({ spec, name, value });
      resources.set(name, value);
      return Promise.resolve({ name });
    },
    createFileWriter: (spec, name) => ({
      writeStream: async (stream) => {
        files.set(name, { spec, text: await new Response(stream).text() });
        return { name };
      },
    }),
  };
  return { context, controller, resources, writes, files };
}

async function git(cwd: string, ...args: string[]): Promise<void> {
  const result = await new Deno.Command("git", {
    cwd,
    args,
    stdin: "null",
    stdout: "piped",
    stderr: "piped",
  }).output();
  assert(result.success, new TextDecoder().decode(result.stderr));
}

async function tempRepo(): Promise<string> {
  const repo = await Deno.makeTempDir({ prefix: "yinz-lifecycle-adapter-" });
  try {
    await git(repo, "init", "--initial-branch=main");
    await git(
      repo,
      "-c",
      "user.name=Lifecycle Test",
      "-c",
      "user.email=test@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "--allow-empty",
      "-m",
      "Fixture",
    );
    const helper = join(repo, "scripts", "issue_lifecycle");
    await Deno.mkdir(helper, { recursive: true });
    await Deno.copyFile(
      new URL("../../../scripts/issue_lifecycle/git_ops.py", import.meta.url),
      join(helper, "git_backend.py"),
    );
    await Deno.copyFile(
      new URL("../../../scripts/issue_lifecycle/workspace.py", import.meta.url),
      join(helper, "workspace.py"),
    );
    // Stub only the external vault command; exercise the real resolver and Git helper.
    const bin = join(repo, "vault-bin");
    await Deno.mkdir(bin);
    await Deno.writeTextFile(join(repo, "vault-workspace"), "default");
    await Deno.writeTextFile(
      join(bin, "swamp"),
      `#!/usr/bin/env python3\nimport json, pathlib, sys\nassert sys.argv[1:6] == ["vault", "read-secret", "default", "AGENDA_WORKSPACE", "--force"]\nprint(json.dumps({"value": pathlib.Path(${
        JSON.stringify(join(repo, "vault-workspace"))
      }).read_text()}))\n`,
    );
    await Deno.chmod(join(bin, "swamp"), 0o755);
    await Deno.writeTextFile(
      join(helper, "git_ops.py"),
      `import os, runpy\nos.environ["PATH"] = ${
        JSON.stringify(bin)
      } + os.pathsep + os.environ["PATH"]\nrunpy.run_path(${
        JSON.stringify(join(helper, "git_backend.py"))
      }, run_name="__main__")\n`,
    );
    await git(repo, "add", ".");
    await git(
      repo,
      "-c",
      "user.name=Lifecycle Test",
      "-c",
      "user.email=test@example.invalid",
      "commit",
      "-m",
      "Fixture helpers",
    );
    return repo;
  } catch (error) {
    await Deno.remove(repo, { recursive: true });
    throw error;
  }
}

Deno.test("adapter resource writes use declared names and round-trip identity", async () => {
  const fixture = fakeContext("/unused");
  const state = createState(issue);
  const result = await saveState(fixture.context, "start", state);
  equal(fixture.writes.map(({ spec }) => spec), ["event", "state"]);
  assert(fixture.writes[0].name.startsWith("event-"));
  EventSchema.parse(fixture.writes[0].value);
  equal(fixture.writes[1].name, "state");
  equal(
    result.dataHandles.map(({ name }) => name),
    fixture.writes.map(({ name }) => name),
  );
  equal(await loadState(fixture.context), state);
  fixture.resources.set("state", { ...state, issue: { ...issue, itemId: 86 } });
  await rejects(() => loadState(fixture.context), "identity does not match");
  fixture.resources.set("state", null);
  await rejects(() => loadState(fixture.context), "Invalid input");
});

Deno.test("existing lifecycle start resumes without writing or consulting Agenda", async () => {
  const repo = await tempRepo();
  try {
    const state = createState(issue);
    const fixture = fakeContext(repo, state);
    const result = await behavior.methods.start.execute({
      workerWorktree: repo,
    }, fixture.context);
    equal(result.dataHandles, []);
    equal(fixture.writes, []);
    fixture.resources.set("state", {});
    await rejects(
      () =>
        behavior.methods.start.execute(
          { workerWorktree: repo },
          fixture.context,
        ),
      "Invalid input",
    );
    equal(fixture.writes, []);
  } finally {
    await Deno.remove(repo, { recursive: true });
  }
});

Deno.test("issue ownership rejects a second behavior before invoking its operation", async () => {
  const repo = await tempRepo();
  try {
    const { context } = fakeContext(repo);
    await withIssue(context, () => Promise.resolve("first"));
    let invoked = false;
    await rejects(
      () =>
        withIssue({ ...context, modelId: "different-behavior" }, () => {
          invoked = true;
          return Promise.resolve();
        }),
      "already belongs",
    );
    assert(!invoked);
  } finally {
    await Deno.remove(repo, { recursive: true });
  }
});

Deno.test("vault workspace mismatch blocks lifecycle work before Agenda or state writes", async () => {
  const repo = await tempRepo();
  try {
    const fixture = fakeContext(repo, createState(issue));
    await Deno.writeTextFile(join(repo, "vault-workspace"), "other");
    await rejects(
      () =>
        behavior.methods.start.execute(
          { workerWorktree: repo },
          fixture.context,
        ),
      "does not match",
    );
    equal(fixture.writes, []);
  } finally {
    await Deno.remove(repo, { recursive: true });
  }
});

Deno.test("lifecycle origin and implementation adoption canonicalize aliases and reject conflicts", async () => {
  const repo = await tempRepo();
  const previousPath = Deno.env.get("PATH");
  try {
    const bin = join(repo, "bin");
    await Deno.mkdir(bin);
    const agenda = join(bin, "agenda");
    const envelope = {
      schema_version: 0,
      item: {
        id: issue.itemId,
        kind: "task",
        text: issue.text,
        note: issue.note,
        completed: false,
        blocked: false,
        discarded_at: null,
        assignments: [],
      },
    };
    await Deno.writeTextFile(
      agenda,
      `#!/bin/sh\ncat <<'AGENDA_JSON'\n${
        JSON.stringify(envelope)
      }\nAGENDA_JSON\n`,
    );
    await Deno.chmod(agenda, 0o755);
    Deno.env.set("PATH", `${bin}:${previousPath ?? ""}`);
    const parent = join(repo, "worktrees");
    const alias = join(repo, "alias");
    await Deno.mkdir(parent);
    await Deno.symlink(parent, alias);
    const args = { worktree: join(alias, "issue-85"), branch: "issue-85" };
    const plan = {
      summary: "Repair behavior",
      steps: [{ id: "fix", description: "Repair and test", files: [] }],
      testingStrategy: "Focused tests",
      checks: [{
        id: "focused",
        command: ["cargo", "x", "test", "-p", "demo"],
        covers: ["fix"],
      }],
    };
    const approve = (state: LifecycleState) => {
      state = transition(state, "plan", plan);
      state = transition(state, "review_plan", {
        planVersion: state.planVersion,
        findings: [],
      });
      return transition(state, "approve", {
        planVersion: state.planVersion,
        request: "Implement this plan",
      });
    };
    const initial = transition(
      createState({ ...issue, digest: await digestIssue(issue) }),
      "triage",
      { classification: "bug", summary: "Confirmed reproduction" },
    );
    const fixture = fakeContext(repo, approve(initial));
    // The first invocation creates a previously nonexistent worktree through
    // a symlinked parent; subsequent invocations must accept the same input.
    await behavior.methods.implement.execute(args, fixture.context);
    const canonical = await Deno.realPath(args.worktree);
    equal(
      (await loadState(fixture.context)).implementation?.worktree,
      canonical,
    );
    await behavior.methods.implement.execute(args, fixture.context);
    fixture.resources.set("state", approve(await loadState(fixture.context)));
    await behavior.methods.implement.execute(args, fixture.context);
    const state = await loadState(fixture.context);
    equal(state.planVersion, 2);
    equal(state.phase, "implementing");
    equal(state.implementation?.worktree, canonical);
    for (
      const changed of [
        { ...args, worktree: repo },
        { ...args, branch: "different-branch" },
      ]
    ) {
      await rejects(
        () => behavior.methods.implement.execute(changed, fixture.context),
        "Implementation must remain in its existing worktree and branch",
      );
      equal(await loadState(fixture.context), state);
    }
    // A fresh lifecycle captures a canonical linked worktree root, even when
    // a direct caller supplies an alias/subdirectory. Repeated start is a no-op.
    const nested = join(args.worktree, "nested");
    await Deno.mkdir(nested);
    fixture.resources.set("state", null);
    await behavior.methods.start.execute(
      { workerWorktree: nested },
      fixture.context,
    );
    const started = await loadState(fixture.context);
    equal(started.workerWorktree, canonical);
    const writeCount = fixture.writes.length;
    await behavior.methods.start.execute(
      { workerWorktree: repo },
      fixture.context,
    );
    equal(await loadState(fixture.context), started);
    equal(fixture.writes.length, writeCount);
    // A fresh start refuses the main checkout and worktrees owned by another
    // issue before writing any state or claim.
    const owners = join(repo, ".git", "yinz-lifecycle", "worktree-owners.json");
    const other = join(repo, "worktrees", "other");
    await git(repo, "worktree", "add", "-b", "other", other);
    const registry = JSON.parse(await Deno.readTextFile(owners));
    registry.owners.push({
      issueKey: "default/99",
      worktree: await Deno.realPath(other),
    });
    await Deno.writeTextFile(owners, JSON.stringify(registry));
    const ownershipBefore = await Deno.readTextFile(owners);
    fixture.resources.set("state", null);
    for (
      const [workerWorktree, message] of [
        [repo, "not the main checkout"],
        [other, "already belongs to issue default/99"],
      ]
    ) {
      await rejects(
        () =>
          behavior.methods.start.execute({ workerWorktree }, fixture.context),
        message,
      );
      equal(fixture.writes.length, writeCount);
      equal(await Deno.readTextFile(owners), ownershipBefore);
    }
    const approved = approve(transition(started, "triage", {
      classification: "bug",
      summary: "Confirmed reproduction",
    }));
    fixture.resources.set("state", approved);
    await rejects(
      () =>
        behavior.methods.implement.execute({
          worktree: repo,
          branch: "conflict",
        }, fixture.context),
      "Implementation must adopt the recorded worker worktree",
    );
    const missing = join(repo, "must-not-create");
    await rejects(
      () =>
        behavior.methods.implement.execute({
          worktree: missing,
          branch: "conflict",
        }, fixture.context),
      "No such file",
    );
    equal(await loadState(fixture.context), approved);
    equal(await Deno.readTextFile(owners), ownershipBefore);
    const branch = await new Deno.Command("git", {
      cwd: repo,
      args: ["show-ref", "--verify", "--quiet", "refs/heads/conflict"],
    }).output();
    assert(!branch.success);
    await behavior.methods.implement.execute(args, fixture.context);
    equal(
      (await loadState(fixture.context)).implementation?.worktree,
      canonical,
    );
    equal((await loadState(fixture.context)).workerWorktree, canonical);
    await git(repo, "show-ref", "--verify", "refs/heads/issue-85");
    // Another issue's start claim that never saved state is reclaimed, and
    // the new start confirms its own claim once its state is saved.
    const abandoned = join(repo, "worktrees", "abandoned");
    await git(repo, "worktree", "add", "-b", "abandoned", abandoned);
    const abandonedPath = await Deno.realPath(abandoned);
    const ownedBy = async (worktree: string) =>
      JSON.parse(await Deno.readTextFile(owners)).owners.filter((
        owner: { worktree: string },
      ) => owner.worktree === worktree);
    const seeded = JSON.parse(await Deno.readTextFile(owners));
    seeded.owners.push({
      issueKey: "default/99",
      worktree: abandonedPath,
      pending: true,
    });
    await Deno.writeTextFile(owners, JSON.stringify(seeded));
    fixture.resources.set("state", null);
    await behavior.methods.start.execute(
      { workerWorktree: abandoned },
      fixture.context,
    );
    const restarted = await loadState(fixture.context);
    equal(restarted.workerWorktree, abandonedPath);
    const confirmed = [{ issueKey: "default/85", worktree: abandonedPath }];
    equal(await ownedBy(abandonedPath), confirmed);
    // Repeated start confirms a claim left pending after its state was saved.
    const pending = JSON.parse(await Deno.readTextFile(owners));
    pending.owners = pending.owners.map((owner: { worktree: string }) =>
      owner.worktree === abandonedPath ? { ...owner, pending: true } : owner
    );
    await Deno.writeTextFile(owners, JSON.stringify(pending));
    const restartWrites = fixture.writes.length;
    await behavior.methods.start.execute(
      { workerWorktree: abandoned },
      fixture.context,
    );
    equal(fixture.writes.length, restartWrites);
    equal(await ownedBy(abandonedPath), confirmed);
  } finally {
    if (previousPath === undefined) Deno.env.delete("PATH");
    else Deno.env.set("PATH", previousPath);
    await Deno.remove(repo, { recursive: true });
  }
});

Deno.test("verify runs approved checks at the reviewed commit and refuses a moved or dirty worktree", async () => {
  const repo = await tempRepo();
  const previousPath = Deno.env.get("PATH");
  try {
    const bin = join(repo, "bin");
    await Deno.mkdir(bin);
    const agenda = join(bin, "agenda");
    const envelope = {
      schema_version: 0,
      item: {
        id: issue.itemId,
        kind: "task",
        text: issue.text,
        note: issue.note,
        completed: false,
        blocked: false,
        discarded_at: null,
        assignments: [],
      },
    };
    await Deno.writeTextFile(
      agenda,
      `#!/bin/sh\ncat <<'AGENDA_JSON'\n${
        JSON.stringify(envelope)
      }\nAGENDA_JSON\n`,
    );
    await Deno.chmod(agenda, 0o755);
    Deno.env.set("PATH", `${bin}:${previousPath ?? ""}`);
    let state = transition(
      createState({ ...issue, digest: await digestIssue(issue) }),
      "triage",
      { classification: "bug", summary: "Confirmed reproduction" },
    );
    state = transition(state, "plan", {
      summary: "Add a marker",
      steps: [{ id: "marker", description: "Add marker", files: ["m.txt"] }],
      testingStrategy: "Inspect the marker",
      checks: [
        {
          id: "contents",
          command: ["grep", "-c", "marker", "m.txt"],
          covers: ["marker"],
        },
        {
          id: "quoting",
          command: ["sh", "-c", 'printf "%s\\n" "$0"; exit 3', "a b; c"],
          covers: ["marker"],
        },
      ],
    });
    state = transition(state, "review_plan", { planVersion: 1, findings: [] });
    state = transition(state, "approve", { planVersion: 1, request: "Go" });
    const fixture = fakeContext(repo, state);
    const worktree = join(repo, "worktrees", "issue-85");
    await behavior.methods.implement.execute(
      { worktree, branch: "issue-85" },
      fixture.context,
    );
    await Deno.writeTextFile(join(worktree, "m.txt"), "marker\n");
    await git(worktree, "add", "m.txt");
    await git(
      worktree,
      "-c",
      "user.name=Lifecycle Test",
      "-c",
      "user.email=test@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "-m",
      "Add marker",
    );
    const head = new TextDecoder().decode(
      (await new Deno.Command("git", {
        cwd: worktree,
        args: ["rev-parse", "HEAD"],
        stdout: "piped",
      }).output()).stdout,
    ).trim();
    fixture.resources.set(
      "state",
      transition(await loadState(fixture.context), "review_code", {
        planVersion: 1,
        commit: head,
        steps: [{ id: "marker", status: "implemented" }],
        findings: [],
      }),
    );
    await behavior.methods.verify.execute({}, fixture.context);
    const verification = (await loadState(fixture.context)).verification!;
    equal(verification.commit, head);
    equal(verification.status, "failed");
    equal(
      verification.checks.map((check) => [check.exitCode, check.status]),
      [[0, "passed"], [3, "failed"]],
    );
    const contents = fixture.files.get(verification.checks[0].log!)!;
    equal(contents.spec, "check-log");
    assert(
      contents.text.includes("$ grep -c marker m.txt\n1\n"),
      contents.text,
    );
    // argv reaches the command verbatim, without shell splitting.
    const quoting = fixture.files.get(verification.checks[1].log!)!.text;
    assert(quoting.includes("\na b; c\nexit 3 after"), quoting);
    // An explicit rerun replaces the failed record at the same commit.
    await behavior.methods.verify.execute({}, fixture.context);
    const rerun = (await loadState(fixture.context)).verification!;
    assert(rerun.id !== verification.id);
    const before = await loadState(fixture.context);
    await Deno.writeTextFile(join(worktree, "untracked.txt"), "dirty\n");
    await rejects(
      () => behavior.methods.verify.execute({}, fixture.context),
      "clean implementation worktree's current commit",
    );
    await Deno.remove(join(worktree, "untracked.txt"));
    await git(
      worktree,
      "-c",
      "user.name=Lifecycle Test",
      "-c",
      "user.email=test@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "--allow-empty",
      "-m",
      "Unreviewed change",
    );
    await rejects(
      () => behavior.methods.verify.execute({}, fixture.context),
      "clean implementation worktree's current commit",
    );
    equal(await loadState(fixture.context), before);
  } finally {
    if (previousPath === undefined) Deno.env.delete("PATH");
    else Deno.env.set("PATH", previousPath);
    await Deno.remove(repo, { recursive: true });
  }
});

Deno.test("long Agenda workspace names produce stable bounded behavior names", async () => {
  const workspace = "a".repeat(64);
  const name = await issueName(workspace, Number.MAX_SAFE_INTEGER);
  assert(name.length <= 64);
  equal(name, await issueName(workspace, Number.MAX_SAFE_INTEGER));
  assert(name !== await issueName("b".repeat(64), Number.MAX_SAFE_INTEGER));
});

Deno.test("existing merge receipt is bound to stored attempt and commit evidence", async () => {
  const receipt = {
    attemptId: "a6da10b0-c980-4b82-b920-640238095f1a",
    originalCommit: "a".repeat(40),
    rebasedCommit: "b".repeat(40),
    baseCommit: "c".repeat(40),
    mergedCommit: "b".repeat(40),
    worktree: "/tmp/issue-85",
    branch: "issue-85",
    ciStatus: "passed" as const,
    mergedAt: "2026-11-03T13:00:00.000Z",
  };
  const state: LifecycleState = {
    ...createState(issue),
    phase: "merged",
    receipt,
  };
  const raw = {
    ...receipt,
    status: "merged",
    issueKey: "default/85",
    issueDigest: issue.digest,
    ci: { status: "passed", command: ["cargo", "x", "ci", "check"] },
  };
  equal(normalizeReceipt(raw, state), receipt);
  await rejects(
    () =>
      normalizeReceipt({
        ...raw,
        attemptId: "b6da10b0-c980-4b82-b920-640238095f1a",
      }, state),
    "receipt",
  );
  await rejects(
    () => normalizeReceipt({ ...raw, originalCommit: "e".repeat(40) }, state),
    "receipt",
  );
  await rejects(
    () => normalizeReceipt({ ...raw, mergedCommit: "f".repeat(40) }, state),
    "receipt",
  );
  await rejects(
    () => normalizeReceipt({ ...raw, issueKey: "default/86" }, state),
    "different issue",
  );
});

Deno.test("subprocess log failure waits for supervisor shutdown before releasing the caller", async () => {
  const repo = await Deno.makeTempDir({ prefix: "yinz-lifecycle-supervisor-" });
  try {
    const helper = join(repo, "scripts", "issue_lifecycle");
    await Deno.mkdir(helper, { recursive: true });
    await Deno.writeTextFile(
      join(helper, "git_ops.py"),
      `import json, pathlib, signal, sys
def stop(signum, frame):
    pathlib.Path("stopped").write_text("reaped")
    sys.exit(130)
signal.signal(signal.SIGTERM, stop)
json.load(sys.stdin)
print("ready", file=sys.stderr, flush=True)
while True:
    signal.pause()
`,
    );
    const { context } = fakeContext(repo);
    context.createFileWriter = () => ({
      writeStream: async (stream) => {
        const reader = stream.getReader();
        try {
          const chunk = await reader.read();
          assert(
            !chunk.done &&
              new TextDecoder().decode(chunk.value).includes("ready"),
          );
          throw new Error("Fixture log storage failed");
        } finally {
          reader.releaseLock();
        }
      },
    });
    await rejects(
      () => gitOperation(context, { action: "inspect" }, "fixture-log"),
      "Fixture log storage failed",
    );
    equal(await Deno.readTextFile(join(repo, "stopped")), "reaped");
  } finally {
    await Deno.remove(repo, { recursive: true });
  }
});

Deno.test("synchronous log-writer creation failure also reaps the supervisor", async () => {
  const repo = await Deno.makeTempDir({ prefix: "yinz-lifecycle-log-create-" });
  try {
    const helper = join(repo, "scripts", "issue_lifecycle");
    await Deno.mkdir(helper, { recursive: true });
    await Deno.writeTextFile(
      join(helper, "git_ops.py"),
      "import signal\nwhile True:\n    signal.pause()\n",
    );
    const { context } = fakeContext(repo);
    context.createFileWriter = () => {
      throw new Error("Fixture writer creation failed");
    };
    // Deno's resource sanitizer also verifies that no subprocess or pipe leaks.
    await rejects(
      () => gitOperation(context, { action: "inspect" }, "fixture-log"),
      "Fixture writer creation failed",
    );
  } finally {
    await Deno.remove(repo, { recursive: true });
  }
});
