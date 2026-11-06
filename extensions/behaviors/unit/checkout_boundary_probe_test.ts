// Standalone capability experiment for issue 26. A passing counterexample test
// proves a candidate boundary is insufficient; it does not certify containment.
// No providers, Yinz sessions, lifecycle methods, or remote Git services run here.
// Opt in with -- --checkout-boundary-probe; ordinary tests need no Landlock or cc.
const fixture = new URL(
  "./fixtures/checkout_boundary_probe.c",
  import.meta.url,
);
const supervisor = new URL(
  "./fixtures/checkout_probe_supervisor.py",
  import.meta.url,
);
const decoder = new TextDecoder();

function assert(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}

async function run(
  cwd: string,
  program: string,
  args: string[],
  timeoutMs = 30_000,
) {
  // The supervisor, rather than AbortSignal on the immediate child, owns the
  // deadline and reaps descendants before inherited output pipes can finish.
  const output = await new Deno.Command("python3", {
    args: [
      decodeURIComponent(supervisor.pathname),
      String(timeoutMs / 1000),
      program,
      ...args,
    ],
    cwd,
    clearEnv: true,
    env: {
      PATH: "/usr/bin:/bin",
      LC_ALL: "C",
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_CONFIG_SYSTEM: "/dev/null",
      GIT_AUTHOR_NAME: "Checkout proof",
      GIT_AUTHOR_EMAIL: "checkout-proof@example.invalid",
      GIT_COMMITTER_NAME: "Checkout proof",
      GIT_COMMITTER_EMAIL: "checkout-proof@example.invalid",
      GIT_TERMINAL_PROMPT: "0",
    },
    stdout: "piped",
    stderr: "piped",
  }).output();
  return {
    code: output.code,
    stdout: decoder.decode(output.stdout).trim(),
    stderr: decoder.decode(output.stderr).trim(),
  };
}

Deno.test({
  name: "checkout probe supervisor collects descendants before returning",
  ignore: !Deno.args.includes("--checkout-boundary-probe"),
  async fn(t) {
    assert(Deno.build.os === "linux", "The probe supervisor requires Linux");
    const temporary = await Deno.makeTempDir({ prefix: "yinz-probe-timeout-" });
    try {
      for (
        const scenario of [
          "retained pipes",
          "exited parent",
          "detached grandchild",
        ]
      ) {
        await t.step(scenario, async () => {
          const start = performance.now();
          const result = await run(temporary, "python3", [
            "-c",
            `import os, signal, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
print(os.getpid(), flush=True)
child = os.fork()
if child == 0:
    if sys.argv[1] == "detached grandchild":
        os.setsid()
        grandchild = os.fork()
        if grandchild != 0:
            print(os.getpid(), flush=True)
            os._exit(0)
    print(os.getpid(), flush=True)
    time.sleep(20)
    os._exit(0)
if sys.argv[1] != "retained pipes":
    os._exit(0)
time.sleep(20)
`,
            scenario,
          ], 1_000);
          assert(result.code === 124, JSON.stringify(result));
          assert(
            result.stderr.includes("all descendants collected"),
            JSON.stringify(result),
          );
          // Timing is the behavior under test: inherited pipes must not keep us
          // waiting for the fixture's 20-second sleep. Allow scheduling slack.
          assert(
            performance.now() - start < 10_000,
            "deadline was not bounded",
          );
          const pids = result.stdout.split(/\s+/).map(Number);
          assert(
            pids.length >= 2 && pids.every((pid) => pid > 0),
            result.stdout,
          );
          await collected(temporary, pids);
        });
      }
      await t.step(
        "successful descendants and command exit status",
        async () => {
          const result = await run(temporary, "python3", [
            "-c",
            `import os
child = os.fork()
print(os.getpid(), flush=True)
os._exit(0 if child == 0 else 7)
`,
          ]);
          assert(result.code === 7, JSON.stringify(result));
          const pids = result.stdout.split(/\s+/).map(Number);
          assert(pids.length === 2, result.stdout);
          await collected(temporary, pids);
        },
      );
      await t.step("supervisor interruption collects descendants", async () => {
        const result = await run(temporary, "python3", [
          "-c",
          `import os, signal, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
supervisor = os.getppid()
ready_read, ready_write = os.pipe()
child = os.fork()
print(os.getpid(), flush=True)
if child != 0:
    os.close(ready_write)
    os.read(ready_read, 1)
    os.kill(supervisor, signal.SIGTERM)
else:
    os.close(ready_read)
    os.write(ready_write, b"1")
time.sleep(20)
`,
        ]);
        assert(result.code === 143, JSON.stringify(result));
        const pids = result.stdout.split(/\s+/).map(Number);
        assert(
          pids.length === 2 && pids.every((pid) => pid > 0),
          result.stdout,
        );
        await collected(temporary, pids);
      });
      await t.step(
        "unavailable executable fails without leaking a child",
        async () => {
          const result = await run(temporary, `${temporary}/missing`, []);
          assert(result.code === 125, JSON.stringify(result));
          assert(
            result.stderr.includes("could not start"),
            JSON.stringify(result),
          );
        },
      );
    } finally {
      await Deno.remove(temporary, { recursive: true });
    }
  },
});

async function succeeds(cwd: string, program: string, args: string[]) {
  const result = await run(cwd, program, args);
  assert(result.code === 0, JSON.stringify({ program, args, ...result }));
  return result.stdout;
}

async function collected(cwd: string, pids: number[]) {
  // Deno protects /proc behind --allow-all even with --allow-read. Use the
  // existing Python dependency to check collection without widening permissions.
  await succeeds(cwd, "python3", [
    "-c",
    'import os, sys; remaining = [pid for pid in sys.argv[1:] if os.path.exists(f"/proc/{pid}")]; assert not remaining, remaining',
    ...pids.map(String),
  ]);
}

async function absent(path: string) {
  try {
    await Deno.lstat(path);
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) return;
    throw error;
  }
  throw new Error(`Unexpected mutation: ${path}`);
}

async function contents(path: string): Promise<string> {
  const entries: Record<string, number[]> = {};
  async function visit(directory: string) {
    for await (const entry of Deno.readDir(directory)) {
      const child = `${directory}/${entry.name}`;
      if (entry.isDirectory) await visit(child);
      else entries[child] = Array.from(await Deno.readFile(child));
    }
  }
  await visit(path);
  return JSON.stringify(
    Object.entries(entries).sort(([a], [b]) => a.localeCompare(b)),
  );
}

Deno.test({
  name: "checkout boundary capability proof (not a production guard)",
  ignore: !Deno.args.includes("--checkout-boundary-probe"),
  async fn(t) {
    assert(
      Deno.build.os === "linux",
      "This candidate uses Linux Landlock; macOS enforcement remains unproved",
    );
    const temporary = await Deno.makeTempDir({
      prefix: "yinz-checkout-proof-",
    });
    try {
      const root = await Deno.realPath(temporary);
      const probe = `${root}/probe`;
      const scratch = `${root}/scratch`;
      const control = `${root}/control`;
      const assigned = `${root}/assigned`;
      const other = `${root}/other`;
      await Deno.mkdir(scratch);
      await succeeds(root, "cc", [
        "-std=c11",
        "-Wall",
        "-Wextra",
        "-Werror",
        decodeURIComponent(fixture.pathname),
        "-o",
        probe,
      ]);
      const abi = Number(await succeeds(root, probe, ["--abi"]));
      assert(abi >= 3, `Landlock ABI 3 required; available ABI: ${abi}`);
      console.log(`Landlock ABI ${abi}; disposable fixture: ${root}`);

      const git = (cwd: string, args: string[]) => succeeds(cwd, "git", args);
      await git(root, ["init", "--initial-branch=control", control]);
      await git(control, [
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "--allow-empty",
        "-m",
        "base",
      ]);
      await git(control, ["worktree", "add", "-b", "assigned", assigned]);
      await git(control, ["worktree", "add", "-b", "other", other]);
      await Deno.writeTextFile(`${other}/tracked`, "preserve this file\n");
      await Deno.mkdir(`${other}/target`);
      await Deno.writeTextFile(
        `${other}/target/artifact`,
        "preserve this build\n",
      );
      await git(other, ["add", "tracked", "target/artifact"]);
      await git(other, [
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "-m",
        "other state",
      ]);
      const common = `${control}/.git`;
      const otherGit = await git(other, ["rev-parse", "--absolute-git-dir"]);
      const otherHead = await git(other, ["rev-parse", "HEAD"]);
      const otherIndex = await Deno.readFile(`${otherGit}/index`);
      const initialOther = await contents(other);
      const initialMetadata = await contents(common);
      const bound = (args: string[], extra = "-", cwd = assigned) =>
        run(root, probe, [cwd, scratch, extra, ...args]);
      const ok = async (args: string[], extra = "-", cwd = assigned) => {
        const result = await bound(args, extra, cwd);
        assert(result.code === 0, JSON.stringify({ args, ...result }));
        return result;
      };
      const denied = async (args: string[]) => {
        const result = await bound(args);
        assert(
          result.code !== 0 && result.code !== 125,
          JSON.stringify(result),
        );
        assert(
          result.stderr.includes("Permission denied"),
          JSON.stringify(result),
        );
      };

      await t.step(
        "assigned edits, subdirectory builds and private scratch succeed",
        async () => {
          await Deno.writeTextFile(
            `${assigned}/main.c`,
            "int main(void) { return 0; }\n",
          );
          await ok([
            "sh",
            "-ec",
            'mkdir -p target; cc main.c -o target/proof; target/proof; printf edit > edited; printf scratch > "$1/proof"',
            "proof",
            scratch,
          ]);
          assert(
            await Deno.readTextFile(`${assigned}/edited`) === "edit",
            "assigned edit missing",
          );
          assert(
            await Deno.readTextFile(`${scratch}/proof`) === "scratch",
            "scratch write missing",
          );
        },
      );
      await t.step(
        "canonical aliases and ordinary subdirectories succeed",
        async () => {
          await Deno.symlink(assigned, `${root}/alias`);
          await ok(
            ["sh", "-ec", "printf alias > alias-edit"],
            "-",
            `${root}/alias`,
          );
          await ok(
            ["sh", "-ec", "printf subdir > subdir-edit"],
            "-",
            `${assigned}/target`,
          );
        },
      );
      await t.step(
        "shell cd changes process cwd but outside writes are denied",
        async () => {
          const result = await ok([
            "sh",
            "-ec",
            'cd "$1"; pwd',
            "proof",
            other,
          ]);
          assert(
            result.stdout === other,
            "Landlock unexpectedly prevented chdir",
          );
          await denied([
            "sh",
            "-ec",
            'cd "$1"; printf escape > escaped',
            "proof",
            other,
          ]);
          await absent(`${other}/escaped`);
        },
      );
      await t.step(
        "absolute paths and interpreter descendants cannot write outside",
        async () => {
          await denied([
            "sh",
            "-ec",
            'printf escape > "$1"',
            "proof",
            `${other}/absolute`,
          ]);
          await denied([
            "python3",
            "-c",
            'import pathlib,sys; pathlib.Path(sys.argv[1]).write_text("escape")',
            `${other}/interpreter`,
          ]);
          await absent(`${other}/absolute`);
          await absent(`${other}/interpreter`);
        },
      );
      await t.step("symlink escapes cannot write outside", async () => {
        await Deno.symlink(other, `${assigned}/escape`);
        await denied(["sh", "-ec", "printf escape > escape/symlink"]);
        await absent(`${other}/symlink`);
      });
      await t.step(
        "existing files and build output cannot be truncated or removed",
        async () => {
          await denied(["sh", "-ec", ': > "$1/tracked"', "proof", other]);
          await denied([
            "sh",
            "-ec",
            'printf changed > "$1/target/artifact"',
            "proof",
            other,
          ]);
          await denied(["rm", `${other}/tracked`]);
        },
      );
      await t.step(
        "Git -C, --git-dir and environment overrides cannot mutate other checkout",
        async () => {
          await denied(["git", "-C", other, "add", "."]);
          await denied([
            "git",
            `--git-dir=${common}`,
            `--work-tree=${assigned}`,
            "update-ref",
            "refs/heads/other",
            otherHead,
            otherHead,
          ]);
          await denied([
            "env",
            `GIT_DIR=${otherGit}`,
            `GIT_WORK_TREE=${assigned}`,
            `GIT_INDEX_FILE=${otherGit}/index`,
            "git",
            "add",
            "main.c",
          ]);
          assert(
            await git(other, ["rev-parse", "HEAD"]) === otherHead,
            "other HEAD changed",
          );
          const index = await Deno.readFile(`${otherGit}/index`);
          assert(
            index.length === otherIndex.length &&
              index.every((v, i) => v === otherIndex[i]),
            "other index changed",
          );
        },
      );
      await t.step(
        "read-only shared Git metadata also blocks legitimate assigned commits",
        async () => {
          await denied(["git", "add", "main.c"]);
          await denied([
            "git",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--allow-empty",
            "-m",
            "blocked",
          ]);
        },
      );
      await t.step(
        "denied operations preserved other checkout files, target, index, HEAD and refs",
        async () => {
          assert(
            await contents(other) === initialOther,
            "other checkout files changed",
          );
          assert(
            await contents(common) === initialMetadata,
            "Git metadata changed",
          );
        },
      );
      await t.step(
        "counterexample: writable common Git metadata permits foreign ref and index mutations",
        async () => {
          await ok(["git", "add", "main.c"], common);
          await ok([
            "git",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-m",
            "assigned change",
          ], common);
          const assignedHead = await git(assigned, ["rev-parse", "HEAD"]);
          assert(assignedHead !== otherHead, "assigned commit missing");
          await ok([
            "git",
            "update-ref",
            "refs/heads/other",
            assignedHead,
            otherHead,
          ], common);
          assert(
            await git(other, ["rev-parse", "HEAD"]) === assignedHead,
            "foreign ref counterexample missing",
          );
          await ok([
            "env",
            `GIT_INDEX_FILE=${otherGit}/index`,
            "git",
            "add",
            "main.c",
          ], common);
          const changed = await Deno.readFile(`${otherGit}/index`);
          assert(
            changed.length !== otherIndex.length ||
              changed.some((v, i) => v !== otherIndex[i]),
            "foreign index counterexample missing",
          );
        },
      );
      await t.step(
        "counterexample: control-root grant includes contained linked worktrees",
        async () => {
          const contained = `${control}/.worktrees/contained`;
          await git(control, ["worktree", "add", "-b", "contained", contained]);
          await ok(
            ["sh", "-ec", 'printf escape > "$1/escaped"', "proof", contained],
            "-",
            control,
          );
          await ok(["git", "-C", contained, "add", "escaped"], "-", control);
          await ok(
            [
              "git",
              "-C",
              contained,
              "-c",
              "core.hooksPath=/dev/null",
              "commit",
              "-m",
              "foreign change",
            ],
            "-",
            control,
          );
          assert(
            await Deno.readTextFile(`${contained}/escaped`) === "escape",
            "contained worktree counterexample missing",
          );
        },
      );
      await t.step(
        "counterexample: existing nested repository remains writable",
        async () => {
          const nested = `${assigned}/nested`;
          await git(assigned, ["init", "--initial-branch=nested", nested]);
          await ok(["sh", "-ec", "printf escape > nested/escaped"]);
          await ok(["git", "-C", nested, "add", "escaped"]);
          assert(
            await Deno.readTextFile(`${nested}/escaped`) === "escape",
            "nested counterexample missing",
          );
        },
      );
      await t.step(
        "counterexample: repository created after restriction remains writable",
        async () => {
          await ok([
            "sh",
            "-ec",
            "git init --initial-branch=new-nested new-nested; printf escape > new-nested/escaped; git -C new-nested add escaped",
          ]);
          assert(
            await Deno.readTextFile(`${assigned}/new-nested/escaped`) ===
              "escape",
            "new nested counterexample missing",
          );
        },
      );
      await t.step(
        "assignment disappearing after preflight fails before exec without cwd fallback",
        async () => {
          const disappearing = `${root}/disappearing`;
          await Deno.mkdir(disappearing);
          const preflight = await Deno.realPath(disappearing);
          await Deno.remove(disappearing);
          const result = await bound(
            ["sh", "-ec", "printf escape > unexpected"],
            "-",
            preflight,
          );
          assert(
            result.code === 125 &&
              result.stderr.includes("assigned cwd unavailable"),
            JSON.stringify(result),
          );
          await absent(`${root}/unexpected`);
        },
      );
    } finally {
      await Deno.remove(temporary, { recursive: true });
    }
  },
});
