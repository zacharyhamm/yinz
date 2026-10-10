import { Evidence, extension } from "./git_workflow.ts";
import { locked } from "./_lib/git_checkout.ts";
import { checked, Claim, claimed, Context } from "./_lib/workflow.ts";
const methods = extension.methods[0];
const originalCommand = Deno.Command;

function assert(value: unknown, message = "assertion failed"): asserts value {
    if (!value) throw new Error(message);
}
async function rejects(action: () => Promise<unknown>, pattern: RegExp) {
    try {
        await action();
    } catch (e) {
        assert(pattern.test(String(e)), String(e));
        return;
    }
    throw new Error("Expected refusal");
}
function context(repoDir: string, repoPath: string) {
    const saved = new Map<string, Record<string, unknown>>();
    const c: Context & {
        logger: {
            info(s: string): void;
            debug(s: string): void;
            warn(s: string): void;
            error(s: string): void;
        };
    } = {
        repoDir,
        modelType: { normalized: "@swamp/git" },
        modelId: "test-git",
        globalArgs: {
            repoPath,
            authorName: "Test",
            authorEmail: "test@example.invalid",
        },
        signal: new AbortController().signal,
        logger: { info() {}, debug() {}, warn() {}, error() {} },
        readResource: (name) => Promise.resolve(saved.get(name) ?? null),
        writeResource: (spec, name, value) => {
            const schema = extension
                .resources[spec as keyof typeof extension.resources]?.schema;
            if (schema) schema.parse(value);
            saved.set(name, structuredClone(value));
            return Promise.resolve({ name, version: 1 });
        },
    };
    return { c, saved };
}
Deno.test("foreground Git commands preserve results, dismissal, and dispatch guards", async () => {
    const dir = await Deno.makeTempDir();
    const main = dir + "/main", source = dir + "/topic ' $(touch INJECTED)";
    const { c, saved } = context(Deno.cwd(), source);
    const message =
        "Complete 'quoted' work $(touch INJECTED) — é\n\nExplain the changes.\nPreserve the second body line.";
    const args = {
        source,
        target: main,
        targetBranch: "main",
        gate: ["/usr/bin/true"],
        binary: "yinz",
        work: "work",
        session: "session",
        key: "dirty",
        message,
    };
    let controller = new AbortController();
    let dispatches = 0;
    let forcedOutcome = "";
    let forcedExit = 130;
    let fault = "";
    let started = Promise.withResolvers<void>();
    let dismissal = Promise.withResolvers<void>();
    let atDismissal = Promise.withResolvers<void>();
    let outputDirectory = "";
    const outputDirectories = new Set<string>();
    let pane: Deno.ChildProcess;
    let current: Record<string, unknown>;
    let completion: Promise<void>;
    const git = (cwd: string, ...argv: string[]) =>
        checked(c, cwd, ["git", ...argv]);
    const gitWithFreshSignal = (cwd: string, ...argv: string[]) =>
        checked({ signal: new AbortController().signal }, cwd, [
            "git",
            ...argv,
        ]);
    class PaneCommand extends originalCommand {
        constructor(command: string | URL, options: Deno.CommandOptions) {
            super(command, options);
            if (command !== "yinz") return;
            let input = "";
            const reply = Promise.withResolvers<Deno.CommandOutput>();
            this.spawn = () => ({
                stdin: new WritableStream<Uint8Array>({
                    write(bytes) {
                        input += new TextDecoder().decode(bytes);
                    },
                    async close() {
                        try {
                            const request = JSON.parse(input);
                            if (request.method === "command") {
                                dispatches++;
                                assert(request.foreground === true);
                                assert(
                                    request.cwd ===
                                        (JSON.parse(request.argv.at(-1))
                                                .mode === "cleanup"
                                            ? main
                                            : source),
                                );
                                assert(request.session === args.session);
                                current = {
                                    work: request.work,
                                    key: request.key,
                                    incarnation: "test",
                                    request: input,
                                    outcome: "pending",
                                    pane: 1,
                                    log: "terminal_1",
                                };
                                outputDirectory =
                                    JSON.parse(request.argv.at(-1)).directory;
                                outputDirectories.add(outputDirectory);
                                if (
                                    ["early abort", "bridge failure"].includes(
                                        fault,
                                    )
                                ) {
                                    completion = Promise.resolve();
                                    if (fault === "early abort") {
                                        controller.abort(new Error(fault));
                                    }
                                    throw new Error(fault);
                                }
                                if (forcedOutcome) {
                                    current.outcome = forcedOutcome;
                                    current.exitCode =
                                        forcedOutcome === "failed"
                                            ? forcedExit
                                            : null;
                                    completion = Promise.resolve();
                                } else {
                                    const child = pane = new originalCommand(
                                        request.argv[0],
                                        {
                                            args: request.argv.slice(1),
                                            cwd: request.cwd,
                                            stdin: "piped",
                                            stdout: "piped",
                                            stderr: "piped",
                                            // A foreground pane inherits the server environment.
                                            env: {
                                                GIT_DIR: main + "/.git",
                                                GIT_WORK_TREE: main,
                                            },
                                        },
                                    ).spawn();
                                    completion = (async () => {
                                        let exited = false,
                                            dismissed = false,
                                            output = "";
                                        const status = child.status.then(
                                            (s) => {
                                                exited = true;
                                                return s;
                                            },
                                        );
                                        const errors = new Response(
                                            child.stderr,
                                        ).text();
                                        for await (
                                            const text of child.stdout
                                                .pipeThrough(
                                                    new TextDecoderStream(),
                                                )
                                        ) {
                                            output += text;
                                            if (
                                                output.includes("BLOCKED HOOK")
                                            ) started.resolve();
                                            if (
                                                !dismissed && output.includes(
                                                    "Press Enter to return.",
                                                )
                                            ) {
                                                assert(
                                                    !exited,
                                                    "Pane exited before dismissal",
                                                );
                                                dismissed = true;
                                                atDismissal.resolve();
                                                if (fault) {
                                                    await dismissal.promise;
                                                }
                                                const writer = child.stdin
                                                    .getWriter();
                                                await writer.write(
                                                    new TextEncoder().encode(
                                                        "\n",
                                                    ),
                                                );
                                                await writer.close();
                                            }
                                        }
                                        const { code } = await status;
                                        current = {
                                            ...current,
                                            exitCode: code,
                                            outcome: code === 0
                                                ? "completed"
                                                : "failed",
                                            output: output + await errors,
                                        };
                                    })();
                                }
                                if (fault === "lost reply") {
                                    await started.promise;
                                    throw new Error(fault);
                                }
                            } else {
                                assert(request.method === "observe");
                                assert(
                                    request.includeOutput === false,
                                    "Worker output must not cross the bridge",
                                );
                                if (fault) {
                                    await started.promise;
                                    if (fault === "cancelled") {
                                        controller.abort(new Error(fault));
                                    }
                                    throw new Error(fault);
                                }
                                await completion;
                            }
                            reply.resolve({
                                code: 0,
                                success: true,
                                signal: null,
                                stdout: new TextEncoder().encode(
                                    JSON.stringify({ result: current }),
                                ),
                                stderr: new Uint8Array(),
                            });
                        } catch (error) {
                            reply.reject(error);
                        }
                    },
                }),
                output: () => reply.promise,
            } as Deno.ChildProcess);
        }
    }
    Deno.Command = PaneCommand;
    // Fixture workers name no live session; only the stranded scenario may notice.
    Deno.env.set("YINZ_COMMIT_SESSION_POLL_SECONDS", "3600");
    try {
        await Deno.mkdir(main);
        await git(main, "init", "-b", "main");
        await git(main, "config", "user.name", "Fixture");
        await git(main, "config", "user.email", "fixture@example.invalid");
        await Deno.writeTextFile(main + "/file", "base\n");
        await git(main, "add", ".");
        await git(main, "commit", "-m", "base");
        await git(main, "worktree", "add", "-b", "topic", source);
        await Deno.writeTextFile(main + "/target", "advance\n");
        await git(main, "add", ".");
        await git(main, "commit", "-m", "advance");
        await Deno.writeTextFile(source + "/file", "topic\n");
        const run = (key: string) =>
            methods.commit_and_rebase.execute({ ...args, key }, c);
        const result = () => saved.get("rebaseResult-current")!;
        // Cancel a real worker while another owner holds both checkout locks.
        const cancelledDirectory = dir + "/cancelled-worker";
        await Deno.mkdir(cancelledDirectory);
        const beforeCancelled = await git(source, "rev-parse", "HEAD");
        const targetBeforeCancelled = await git(main, "rev-parse", "HEAD");
        const waiting = Promise.withResolvers<void>();
        const interrupted = Promise.withResolvers<void>();
        let cancelledOutput = "";
        await locked(c, [source, main], async () => {
            pane = new originalCommand(Deno.execPath(), {
                args: [
                    "eval",
                    `const lock = Deno.FsFile.prototype.lock;
Deno.FsFile.prototype.lock = function(...args) {
    console.log("WAITING FOR LOCK");
    return lock.apply(this, args);
};
Deno.addSignalListener("SIGINT", () => console.log("SIGINT RECEIVED"));
await import(${
                        JSON.stringify(
                            new URL("./_lib/commit_worker.ts", import.meta.url)
                                .href,
                        )
                    });`,
                    JSON.stringify({
                        ...args,
                        mode: "commit",
                        binary: "/usr/bin/true",
                        author: [],
                        directory: cancelledDirectory,
                        repoDir: Deno.cwd(),
                    }),
                ],
                stdin: "piped",
                stdout: "piped",
                stderr: "inherit",
            }).spawn();
            const timeout = setTimeout(() => pane.kill("SIGKILL"), 30000);
            completion = (async () => {
                try {
                    for await (
                        const text of pane.stdout.pipeThrough(
                            new TextDecoderStream(),
                        )
                    ) {
                        cancelledOutput += text;
                        if (cancelledOutput.includes("WAITING FOR LOCK")) {
                            waiting.resolve();
                        }
                        if (cancelledOutput.includes("SIGINT RECEIVED")) {
                            interrupted.resolve();
                        }
                    }
                    assert((await pane.status).code === 130, cancelledOutput);
                } finally {
                    clearTimeout(timeout);
                }
            })();
            const writer = pane.stdin.getWriter();
            await writer.write(new TextEncoder().encode("\n"));
            await writer.close();
            await Promise.race([waiting.promise, completion]);
            pane.kill("SIGINT");
            await Promise.race([interrupted.promise, completion]);
        });
        await completion!;
        const cancelledResult = JSON.parse(
            await Deno.readTextFile(cancelledDirectory + "/result.json"),
        );
        assert(cancelledResult.cancelled && cancelledResult.exitCode === 130);
        assert(
            String(cancelledResult.error).includes("Step cancelled"),
            JSON.stringify(cancelledResult),
        );
        assert(
            !/\$ git (add|commit|rebase)\b/.test(cancelledOutput),
            cancelledOutput,
        );
        assert(await git(source, "rev-parse", "HEAD") === beforeCancelled);
        assert(await git(main, "rev-parse", "HEAD") === targetBeforeCancelled);
        assert(await git(source, "diff", "--cached") === "");
        assert(await Deno.readTextFile(source + "/file") === "topic\n");
        assert(!Evidence.parse(cancelledResult.evidence).source.rebasing);
        const lockPaths = await Promise.all(
            [source, main].map(async (path) =>
                (await gitWithFreshSignal(
                    path,
                    "rev-parse",
                    "--absolute-git-dir",
                )) + "/yinz-workflow.lock"
            ),
        );
        // A released waiter's kernel request can be granted and dropped a moment
        // later, so the expectation is polled rather than sampled once.
        const checkLocks = async (held: boolean) => {
            let check!: Deno.CommandOutput;
            for (let attempt = 0; attempt < 50; attempt++) {
                check = await new originalCommand("python3", {
                    args: [
                        "-c",
                        `import fcntl,sys
for path in sys.argv[2:]:
    with open(path, 'a') as f:
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            locked = False
        except BlockingIOError:
            locked = True
        assert locked == (sys.argv[1] == 'true'), path
`,
                        String(held),
                        ...lockPaths,
                    ],
                }).output();
                if (check.success) return;
                await new Promise((resolve) => setTimeout(resolve, 100));
            }
            assert(check.success, new TextDecoder().decode(check.stderr));
        };
        const spawnWorker = (
            directory: string,
            extra: Record<string, unknown>,
            env: Record<string, string>,
        ) => {
            const child = new originalCommand(Deno.execPath(), {
                args: [
                    "run",
                    "--no-config",
                    "--allow-all",
                    new URL("./_lib/commit_worker.ts", import.meta.url)
                        .pathname,
                    JSON.stringify({
                        ...args,
                        mode: "commit",
                        binary: "/usr/bin/true",
                        author: [],
                        directory,
                        repoDir: Deno.cwd(),
                        ...extra,
                    }),
                ],
                cwd: source,
                stdin: "piped",
                stdout: "piped",
                stderr: "inherit",
                env,
            }).spawn();
            const timeout = setTimeout(() => child.kill("SIGKILL"), 60000);
            let output = "";
            const reading = (async () => {
                try {
                    for await (
                        const text of child.stdout.pipeThrough(
                            new TextDecoderStream(),
                        )
                    ) output += text;
                } finally {
                    clearTimeout(timeout);
                }
            })();
            const waitFor = async (text: string) => {
                for (let attempt = 0; attempt < 300; attempt++) {
                    if (output.includes(text)) return;
                    await new Promise((resolve) => setTimeout(resolve, 100));
                }
                throw new Error(`Worker never printed ${text}:\n${output}`);
            };
            const dismiss = async () => {
                const writer = child.stdin.getWriter();
                await writer.write(new TextEncoder().encode("\n"));
                await writer.close();
                await reading;
                return (await child.status).code;
            };
            return { child, waitFor, dismiss, reading, output: () => output };
        };
        await methods.verify_checkout.execute(args, c);
        const current = Evidence.parse(saved.get("evidence-current"));
        // A mismatch refuses the mutation after the lock, keeping later checks intact.
        const staleEvidence = {
            ...current,
            source: { ...current.source, contents: "0".repeat(64) },
        };
        // A contended lock names its holder on screen and is never a blank pane.
        const visibleDirectory = dir + "/visible-worker";
        await Deno.mkdir(visibleDirectory);
        let visible!: ReturnType<typeof spawnWorker>;
        await locked(c, [source, main], async () => {
            visible = spawnWorker(visibleDirectory, {
                expectedEvidence: staleEvidence,
            }, { YINZ_COMMIT_LOCK_NOTICE_SECONDS: "0" });
            await visible.waitFor("no longer exists");
            const text = visible.output();
            assert(text.startsWith("Commit and rebase: " + source), text);
            assert(text.includes(`held by pid ${Deno.pid} since`), text);
            assert(
                text.includes("session other-session, step other-key"),
                text,
            );
            assert(text.includes("press Ctrl-C to cancel this step"), text);
            assert(!/\$ git/.test(text), text);
        }, {
            identity: {
                pid: Deno.pid,
                session: "other-session",
                work: "other-work",
                key: "other-key",
            },
        });
        await visible.waitFor("Press Enter to return.");
        for (const path of lockPaths) {
            const holder = JSON.parse(await Deno.readTextFile(path));
            assert(
                holder.pid === visible.child.pid &&
                    holder.session === args.session &&
                    holder.work === args.work && holder.key === args.key,
                path,
            );
        }
        await checkLocks(true);
        assert(await visible.dismiss() === 124, visible.output());
        const visibleResult = JSON.parse(
            await Deno.readTextFile(visibleDirectory + "/result.json"),
        );
        assert(
            String(visibleResult.error).includes("Checkout changed since"),
            JSON.stringify(visibleResult),
        );
        assert(!/\$ git/.test(visible.output()), visible.output());
        await checkLocks(false);
        assert(
            (await Deno.readTextFile(visibleDirectory + "/output.log")) ===
                visible.output(),
        );
        // A holder that never yields fails the step without leaving a lock behind.
        const expiredDirectory = dir + "/expired-worker";
        await Deno.mkdir(expiredDirectory);
        let expired!: ReturnType<typeof spawnWorker>;
        await locked(c, [source, main], async () => {
            // Anonymous acquisition clears the previous worker's identity.
            for (const path of lockPaths) {
                assert(await Deno.readTextFile(path) === "", path);
            }
            expired = spawnWorker(expiredDirectory, {}, {
                YINZ_COMMIT_LOCK_NOTICE_SECONDS: "0",
                YINZ_COMMIT_LOCK_WAIT_SECONDS: "1",
            });
            await expired.waitFor("Press Enter to return.");
        });
        assert(
            expired.output().includes("still held after 1s"),
            expired.output(),
        );
        assert(expired.output().includes("no holder record"), expired.output());
        assert(!expired.output().includes("by pid"), expired.output());
        assert(
            !expired.output().includes("no longer exists"),
            expired.output(),
        );
        assert(!/\$ git/.test(expired.output()), expired.output());
        await checkLocks(false);
        assert(await expired.dismiss() === 124, expired.output());
        const expiredResult = JSON.parse(
            await Deno.readTextFile(expiredDirectory + "/result.json"),
        );
        assert(
            expiredResult.exitCode === 124 &&
                /Checkout lock .* still held/.test(String(expiredResult.error)),
            JSON.stringify(expiredResult),
        );
        // Output written so far is readable while the worker is still alive.
        assert(
            (await Deno.readTextFile(expiredDirectory + "/output.log"))
                .includes(
                    "still held after 1s",
                ),
        );
        // Session probes resolve the binary from repoDir, even in a different cwd.
        const strandedDirectory = dir + "/stranded-worker";
        await Deno.mkdir(strandedDirectory);
        await Deno.symlink("/usr/bin/true", strandedDirectory + "/yinz");
        const stranded = spawnWorker(strandedDirectory, {
            binary: "./yinz",
            repoDir: strandedDirectory,
            expectedEvidence: staleEvidence,
        }, { YINZ_COMMIT_SESSION_POLL_SECONDS: "0.2" });
        await stranded.waitFor("is gone; releasing checkout locks");
        await stranded.reading;
        assert((await stranded.child.status).code === 124, stranded.output());
        assert(stranded.output().includes("Press Enter to return."));
        await checkLocks(false);
        // Through the model method, an expired lock wait is an ordinary failed result.
        Deno.env.set("YINZ_COMMIT_LOCK_WAIT_SECONDS", "1");
        Deno.env.set("YINZ_COMMIT_LOCK_NOTICE_SECONDS", "0");
        try {
            await locked(c, [source, main], () => run("lock-expired"));
        } finally {
            Deno.env.delete("YINZ_COMMIT_LOCK_WAIT_SECONDS");
            Deno.env.delete("YINZ_COMMIT_LOCK_NOTICE_SECONDS");
        }
        assert(
            result().exitCode === 124 &&
                /still held after 1s/.test(String(result().stderr)) &&
                String(result().stdout).includes("Waiting for checkout lock"),
            JSON.stringify(result()),
        );
        const dispatchesBeforeDirty: number = dispatches;
        await run("dirty");
        assert(result().exitCode === 0);
        assert(String(result().stdout).includes("$ git add -A"));
        assert(String(result().stdout).includes("$ git rebase "));
        assert(
            await git(source, "log", "-1", "--format=%B") === message + "\n",
        );
        assert(
            await git(source, "log", "-1", "--format=%an <%ae>") ===
                "Test <test@example.invalid>",
        );
        assert(
            await git(source, "rev-parse", "HEAD^") ===
                await git(main, "rev-parse", "HEAD"),
        );
        assert(Evidence.parse(result().evidence).source.clean);
        assert(
            (result().operation as Record<string, unknown>).log ===
                "terminal_1",
        );
        assert(!await Deno.stat(source + "/INJECTED").catch(() => null));
        const head = await git(source, "rev-parse", "HEAD");
        await run("dirty");
        assert(
            dispatches === dispatchesBeforeDirty + 1,
            "Repeated a completed mutation",
        );
        await run("clean");
        assert(await git(source, "rev-parse", "HEAD") === head);
        assert(String(result().stdout).includes("No changes to commit."));
        await methods.verify_checkout.execute(args, c);
        const expectedEvidence = Evidence.parse(saved.get("evidence-current"));
        for (const side of [source, main]) {
            await Deno.writeTextFile(
                side + "/changed-after-message",
                "new contents\n",
            );
            await methods.commit_and_rebase.execute({
                ...args,
                key: "stale-" + (side === source ? "source" : "target"),
                expectedEvidence,
            }, c);
            assert(
                String(saved.get("rebaseResult-current")?.stderr).includes(
                    "Checkout changed since commit message",
                ),
            );
            assert(await git(source, "rev-parse", "HEAD") === head);
            assert(await git(source, "diff", "--cached") === "");
            await Deno.remove(side + "/changed-after-message");
        }
        await Deno.writeTextFile(source + "/file", "commit should fail\n");
        const hooks = dir + "/hooks";
        await Deno.mkdir(hooks);
        await Deno.symlink("/usr/bin/false", hooks + "/pre-commit");
        await git(main, "config", "core.hooksPath", hooks);
        await run("commit-failed");
        assert(result().exitCode !== 0);
        assert(String(result().stdout).includes("Press Enter to return."));
        assert(!String(result().stdout).includes("$ git rebase "));
        assert(await git(source, "rev-parse", "HEAD") === head);
        await git(main, "config", "--unset", "core.hooksPath");
        await Deno.remove(hooks + "/pre-commit");
        await Deno.writeTextFile(main + "/file", "target conflict\n");
        await git(main, "commit", "-am", "conflict");
        await run("conflict");
        assert(result().exitCode !== 0);
        assert(Evidence.parse(result().evidence).source.rebasing);
        assert(
            Evidence.parse(result().evidence).source.conflicts.includes("file"),
        );
        await run("already-rebasing");
        assert(String(result().stderr).includes("Resolve the rebase"));
        await methods.rebase_continue.execute(
            { ...args, key: "unresolved" },
            c,
        );
        assert(String(result().stderr).includes("not ready"));
        await Deno.writeTextFile(source + "/file", "resolved file\n");
        await git(source, "add", "file");
        await methods.verify_checkout.execute(args, c);
        const staged = Evidence.parse(saved.get("evidence-current"));
        const continueArgs = {
            ...args,
            key: "continue-explicit",
            evidence: staged,
        };
        const beforeContinue = await git(source, "rev-parse", "HEAD");
        await Deno.writeTextFile(source + "/late", "invalidate choice");
        await methods.rebase_continue.execute({
            ...continueArgs,
            key: "continue-stale",
        }, c);
        assert(String(result().stderr).includes("Stale rebase choice"));
        assert(await git(source, "rev-parse", "HEAD") === beforeContinue);
        await Deno.remove(source + "/late");
        // No additional resolver edits are required after the staged decision.
        await methods.rebase_continue.execute(continueArgs, c);
        assert(
            Evidence.parse(result().evidence).source.conflicts.includes("file"),
        );
        assert(String(result().stdout).includes("--continue"));
        const continuedDispatches = dispatches;
        await methods.rebase_continue.execute(continueArgs, c);
        assert(dispatches === continuedDispatches);
        await Deno.writeTextFile(source + "/file", "resolved again\n");
        await git(source, "add", "file");
        await methods.rebase_continue.execute({
            ...args,
            key: "continue-again",
        }, c);
        assert(
            result().exitCode === 0 &&
                !Evidence.parse(result().evidence).source.rebasing,
            JSON.stringify(result()),
        );
        await Deno.writeTextFile(main + "/file", "dirty target\n");
        await run("dirty-target");
        assert(String(result().stderr).includes("Target checkout is dirty"));
        await git(main, "restore", "file");
        // A real worker keeps both kernel locks after its observer goes away.
        for (
            const reason of [
                "cancelled",
                "observation error",
                "lost reply",
                "Observation deadline reached",
            ]
        ) {
            fault = reason;
            started = Promise.withResolvers<void>();
            dismissal = Promise.withResolvers<void>();
            atDismissal = Promise.withResolvers<void>();
            controller = new AbortController();
            c.signal = controller.signal;
            const release = dir + "/release";
            await Deno.writeTextFile(
                hooks + "/pre-commit",
                "#!/bin/sh\necho BLOCKED HOOK\nwhile [ ! -e " + release +
                    " ]; do sleep 0.01; done\n",
            );
            await Deno.chmod(hooks + "/pre-commit", 0o700);
            await git(main, "config", "core.hooksPath", hooks);
            await Deno.writeTextFile(source + "/new-file", reason);
            const key = "failure-" + reason.replaceAll(" ", "-");
            await rejects(() => run(key), new RegExp(reason));
            await checkLocks(true);
            const count: number = dispatches;
            c.signal = new AbortController().signal;
            await rejects(() => run(key), /is unknown/);
            assert(dispatches === count, "Uncertain dispatch repeated");
            await Deno.writeTextFile(release, "");
            await atDismissal.promise;
            await checkLocks(true);
            dismissal.resolve();
            await completion!;
            await checkLocks(false);
            assert(saved.get("claim-" + key)?.outcome === "unknown");
            await Deno.remove(release);
            // The worker wrote while its outcome was uncertain, so its output is
            // retained for reconciliation rather than removed with the run.
            assert(
                (await Deno.stat(outputDirectory + "/output.log")).isFile,
                "uncertain worker output must be retained",
            );
            await Deno.remove(outputDirectory, { recursive: true });
            if (await git(source, "diff", "--name-only", "--diff-filter=U")) {
                await git(source, "rebase", "--abort");
            }
        }
        fault = "";
        await git(main, "config", "--unset", "core.hooksPath");
        // Retain more than the bridge frame can safely carry, including multibyte text.
        const full = "BEGIN\n" + "😀é𝄞\n".repeat(65000) + "END\n";
        await Deno.writeTextFile(
            hooks + "/pre-commit",
            "#!/bin/sh\ncat '" + dir + "/large-output'\n",
        );
        await Deno.writeTextFile(dir + "/large-output", full);
        await git(main, "config", "core.hooksPath", hooks);
        await Deno.writeTextFile(source + "/new-file", "large");
        await run("large-output");
        const ref = result().outputRef as { name: string };
        assert(String(saved.get(ref.name)?.output).includes(full));
        const preview = String(result().stdout);
        assert(new TextEncoder().encode(preview).length <= 16384);
        assert(
            preview.includes("BEGIN") && preview.includes("END") &&
                preview.includes("truncated"),
        );
        assert(!preview.includes("\ufffd"));
        assert(!(result().operation as Record<string, unknown>).output);
        const claim = JSON.parse(
            String(saved.get("claim-large-output")?.result),
        );
        assert(claim.stdout === preview && !claim.operation.output);
        assert(new TextEncoder().encode(JSON.stringify(claim)).length < 24000);
        await rejects(() => Deno.stat(outputDirectory), /NotFound/);
        for (const reason of ["early abort", "bridge failure"]) {
            fault = reason;
            controller = new AbortController();
            c.signal = controller.signal;
            await rejects(
                () => run(reason.replace(" ", "-")),
                new RegExp(reason),
            );
            assert(
                (await Array.fromAsync(Deno.readDir(outputDirectory)))
                    .length === 0,
            );
            // A worker can start writing after the caller has already failed.
            await Deno.writeTextFile(
                outputDirectory + "/output.log",
                "late worker output",
            );
            assert(
                await Deno.readTextFile(outputDirectory + "/output.log") ===
                    "late worker output",
            );
        }
        fault = "";
        controller = new AbortController();
        c.signal = controller.signal;
        const readTextFile = Deno.readTextFile;
        try {
            Deno.readTextFile = async (path, options) => {
                const text = await readTextFile(path, options);
                if (path === outputDirectory + "/result.json") {
                    controller.abort(new Error("final abort"));
                }
                return text;
            };
            await rejects(() => run("final-abort"), /final abort/);
        } finally {
            Deno.readTextFile = readTextFile;
            c.signal = new AbortController().signal;
        }
        assert(
            JSON.parse(
                await Deno.readTextFile(outputDirectory + "/result.json"),
            ).exitCode === 0,
        );
        forcedOutcome = "unknown";
        await rejects(() => run("unknown"), /completion is uncertain/);
        const count: number = dispatches;
        await rejects(() => run("unknown"), /Operation unknown is unknown/);
        assert(dispatches === count);
        forcedOutcome = "failed";
        await rejects(() => run("interrupted"), /uncertain/);
        forcedExit = 125;
        await rejects(() => run("input-closed"), /uncertain/);

        await rejects(
            () =>
                methods.commit_and_rebase.execute({
                    ...args,
                    message: "changed",
                }, c),
            /Conflicting request/,
        );
        forcedOutcome = "";
        await git(main, "config", "--unset", "core.hooksPath");
        await methods.verify_checkout.execute(args, c);
        const mergeArgs = {
            ...args,
            key: "foreground-merge",
            approved: true as const,
            evidence: Evidence.parse(saved.get("evidence-current")),
            review: {
                evidence: Evidence.parse(saved.get("evidence-current")),
                outcome: "process-exit",
                findings: [],
                reviewer: "reviewer",
                valid: true,
                resolved: true,
            },
            test: {
                evidence: Evidence.parse(saved.get("evidence-current")),
                outcome: "completed",
                exitCode: 0,
                log: "test log",
                passed: true,
            },
        };
        for (const status of ["unknown", "failed"] as const) {
            forcedOutcome = status;
            forcedExit = 130;
            for (const mode of ["continue", "integrate", "cleanup"] as const) {
                const key = `uncertain-${mode}-${status}`;
                const execute = () =>
                    mode === "continue"
                        ? methods.rebase_continue.execute({ ...args, key }, c)
                        : mode === "integrate"
                        ? methods.integrate.execute({ ...mergeArgs, key }, c)
                        : methods.guarded_cleanup.execute({
                            ...args,
                            key,
                            approved: true,
                            commit: mergeArgs.evidence.source.commit,
                        }, c);
                await rejects(execute, /completion is uncertain/);
                const beforeRetry: number = dispatches;
                await rejects(execute, /is unknown/);
                assert(dispatches === beforeRetry);
                // An uncertain dispatch must leave room for later worker writes.
                assert((await Deno.stat(outputDirectory)).isDirectory);
            }
        }
        forcedOutcome = "";
        await Deno.writeTextFile(source + "/late", "stale merge approval");
        await methods.integrate.execute({
            ...mergeArgs,
            key: "foreground-stale",
        }, c);
        assert(saved.get("integrationDecision-current")?.integrated === false);
        await Deno.remove(source + "/late");
        await methods.integrate.execute(mergeArgs, c);
        assert(saved.get("integrationDecision-current")?.integrated === true);
        const mergeDispatches = dispatches;
        await methods.integrate.execute(mergeArgs, c);
        assert(dispatches === mergeDispatches);
        assert(
            await git(main, "rev-parse", "HEAD") ===
                mergeArgs.evidence.source.commit,
        );
        const cleanupArgs = {
            ...args,
            key: "foreground-cleanup",
            approved: true as const,
            commit: mergeArgs.evidence.source.commit,
        };
        await Deno.writeTextFile(source + "/keep", "preserve me");
        await methods.guarded_cleanup.execute({
            ...cleanupArgs,
            key: "foreground-unsafe",
        }, c);
        assert(
            String(saved.get("cleanupDecision-current")?.reason).includes(
                "changed after integration",
            ),
        );
        assert(saved.get("cleanupDecision-current")?.removed === false);
        assert(await Deno.readTextFile(source + "/keep") === "preserve me");
        await Deno.remove(source + "/keep");
        await methods.guarded_cleanup.execute(cleanupArgs, c);
        assert(saved.get("remove-worktree")?.removed === true);
        const cleanupDispatches = dispatches;
        await methods.guarded_cleanup.execute(cleanupArgs, c);
        assert(dispatches === cleanupDispatches);
        await rejects(() => Deno.stat(source), /NotFound/);
        assert(
            !methods.commit_and_rebase.arguments.safeParse({
                ...args,
                message: "bad\0message",
            }).success,
        );
    } finally {
        Deno.Command = originalCommand;
        Deno.env.delete("YINZ_COMMIT_SESSION_POLL_SECONDS");
        dismissal.resolve();
        if (pane!) {
            try {
                pane.kill("SIGKILL");
            } catch { /* Already exited. */ }
        }
        await completion!;
        for (const directory of outputDirectories) {
            await Deno.remove(directory, { recursive: true }).catch((error) => {
                if (!(error instanceof Deno.errors.NotFound)) throw error;
            });
        }
        await Deno.remove(dir, { recursive: true });
    }
});
Deno.test("checkout status only reduces the menu for clean local branches with no commits beyond the target", async () => {
    const dir = await Deno.makeTempDir();
    try {
        const main = dir + "/main", source = dir + "/topic", sub = dir + "/sub";
        const { c, saved } = context(dir, source);
        const git = (cwd: string, args: string[]) =>
            checked(c, cwd, [
                "git",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "protocol.file.allow=always",
                ...args,
            ]);
        for (const repo of [main, sub]) {
            await Deno.mkdir(repo);
            await git(repo, ["init", "-b", "main"]);
            await Deno.writeTextFile(repo + "/file", "base\n");
            await Deno.writeTextFile(repo + "/.gitignore", "ignored\n");
            await git(repo, ["add", "."]);
            await git(repo, ["commit", "-m", "base"]);
        }
        await git(main, ["submodule", "add", sub, "module"]);
        await git(main, ["commit", "-am", "submodule"]);
        await git(main, ["worktree", "add", "-b", "topic", source]);
        await git(source, ["submodule", "update", "--init"]);
        await git(source, ["config", "submodule.module.ignore", "all"]);
        const index = await git(source, [
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "index",
        ]);
        const inspect = async (
            label: string,
            unchanged: boolean,
            overrides: Record<string, string> = {},
            failed = false,
        ) => {
            const before = await Deno.readFile(index);
            await methods.checkout_status.execute(
                methods.checkout_status.arguments.parse({
                    source,
                    ...overrides,
                }),
                c,
            );
            const result = saved.get("checkoutStatus-current")!;
            assert(
                result.unchanged === unchanged,
                label + ": " + JSON.stringify(result),
            );
            assert(
                (result.error !== null) === failed,
                label + ": " + JSON.stringify(result),
            );
            assert(
                before.toString() === (await Deno.readFile(index)).toString(),
                "inspection rewrote the index",
            );
        };
        await inspect("equal to main", true);
        await Deno.writeTextFile(source + "/ignored", "ignored change");
        await inspect("ignored files do not count", true);
        await git(main, ["commit", "--allow-empty", "-m", "advance target"]);
        await inspect("behind main", true);
        await Deno.writeTextFile(source + "/file", "changed\n");
        await inspect("unstaged", false);
        await git(source, ["add", "file"]);
        await inspect("staged", false);
        await git(source, ["reset", "--hard", "HEAD"]);
        await Deno.writeTextFile(source + "/untracked", "new file");
        await inspect("untracked", false);
        await Deno.remove(source + "/untracked");
        const module = source + "/module";
        await Deno.writeTextFile(module + "/file", "changed submodule\n");
        await inspect("submodule unstaged despite ignore config", false);
        await git(module, ["add", "file"]);
        await inspect("submodule staged", false);
        await git(module, ["reset", "--hard", "HEAD"]);
        await Deno.writeTextFile(module + "/untracked", "new submodule file");
        await inspect("submodule untracked", false);
        await Deno.remove(module + "/untracked");
        await git(module, ["commit", "--allow-empty", "-m", "submodule ahead"]);
        await inspect("submodule commit", false);
        await git(module, ["reset", "--hard", "HEAD~"]);
        await inspect("submodule restored", true);
        for (const name of ["rebase-merge", "rebase-apply"]) {
            const path = await git(source, [
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                name,
            ]);
            await Deno.mkdir(path);
            await inspect("clean paused " + name, false);
            await Deno.remove(path);
        }
        await git(source, [
            "commit",
            "--allow-empty",
            "-m",
            "ahead without a diff",
        ]);
        await inspect("commits beyond main", false);
        await git(source, ["branch", "release"]);
        await inspect("configured local target", true, {
            targetBranch: "release",
        });
        await git(source, ["tag", "missing"]);
        await inspect("missing branch even with matching tag", false, {
            targetBranch: "missing",
        }, true);
        await inspect(
            "missing checkout",
            false,
            { source: dir + "/missing" },
            true,
        );
        await inspect("not a repository", false, { source: dir }, true);
        await Deno.mkdir(source + "/nested");
        await inspect("not a checkout root", false, {
            source: source + "/nested",
        }, true);
        await Deno.mkdir(dir + "/unborn");
        await git(dir + "/unborn", ["init", "-b", "main"]);
        await inspect("missing HEAD", false, { source: dir + "/unborn" }, true);
        const previousIndex = Deno.env.get("GIT_INDEX_FILE");
        try {
            Deno.env.set("GIT_INDEX_FILE", index);
            await inspect("inherited Git context", false, {}, true);
        } finally {
            if (previousIndex === undefined) Deno.env.delete("GIT_INDEX_FILE");
            else Deno.env.set("GIT_INDEX_FILE", previousIndex);
        }
        Deno.Command = class extends originalCommand {
            override output(): Promise<Deno.CommandOutput> {
                throw new Error("Injected Git inspection failure");
            }
        };
        await inspect("Git command error", false, {}, true);
    } finally {
        Deno.Command = originalCommand;
        await Deno.remove(dir, { recursive: true });
    }
});
Deno.test("guarded Git extension rejects dirty/stale evidence, rebases, integrates once, and refuses unsafe cleanup", async () => {
    const dir = await Deno.makeTempDir();
    try {
        const main = dir + "/main", source = dir + "/topic";
        await Deno.mkdir(main);
        const { c, saved } = context(dir, source);
        const git = (cwd: string, args: string[]) =>
            checked(c, cwd, [
                "git",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                ...args,
            ]);
        await git(main, ["init", "-b", "main"]);
        await Deno.writeTextFile(main + "/file", "base\n");
        await git(main, ["add", "."]);
        await git(main, ["commit", "-m", "base"]);
        await git(main, ["worktree", "add", "-b", "topic", source]);
        const args = {
            source,
            target: main,
            targetBranch: "main",
            gate: ["cargo", "x", "ci", "check"],
            binary: "yinz",
        };
        await Deno.writeTextFile(source + "/file", "topic\n");
        await methods.verify_checkout.execute(args, c);
        assert(!Evidence.parse(saved.get("evidence-current")).source.clean);
        await rejects(
            () => methods.rebase.execute({ ...args, key: "dirty" }, c),
            /dirty/,
        );
        const beforeDiff = await git(source, ["status", "--porcelain=v1"]);
        const untracked = 'untracked contents 😀é\t"\\\n'.repeat(40000);
        assert(new TextEncoder().encode(untracked).length > 1_100_000);
        await Deno.writeTextFile(source + "/untracked.txt", untracked);
        await methods.show_diff.execute(args, c);
        const output = String(saved.get("diff-current")?.output);
        assert(
            output.includes("Committed branch changes:") &&
                output.includes("Staged changes:") &&
                output.includes("Unstaged changes:"),
        );
        assert(
            output.includes("+topic") && output.includes("untracked contents"),
        );
        assert(
            output.endsWith(untracked),
            "the saved diff retains every character",
        );
        await Deno.remove(source + "/untracked.txt");
        assert(await git(source, ["status", "--porcelain=v1"]) === beforeDiff);
        await methods.guarded_commit.execute({
            ...args,
            key: "commit",
            message: "topic",
        }, c);
        const committed = await git(source, ["rev-parse", "HEAD"]);
        await methods.guarded_commit.execute({
            ...args,
            key: "empty-commit",
            message: "skip",
        }, c);
        assert(await git(source, ["rev-parse", "HEAD"]) === committed);
        await methods.rebase.execute({ ...args, key: "rebase" }, c);
        await methods.verify_checkout.execute(args, c);
        const proof = Evidence.parse(saved.get("evidence-current"));
        const input = {
            ...args,
            key: "integrate",
            approved: true as const,
            evidence: proof,
            review: {
                evidence: proof,
                outcome: "completed" as const,
                findings: [],
                reviewer: "separate-agent",
                valid: true as const,
                resolved: true as const,
            },
            test: {
                evidence: proof,
                outcome: "completed" as const,
                exitCode: 0 as const,
                log: "terminal_9",
                passed: true as const,
            },
        };
        await Deno.writeTextFile(source + "/untracked", "dirty");
        await methods.integrate.execute({ ...input, key: "dirty-evidence" }, c);
        assert(saved.get("integrationDecision-current")?.integrated === false);
        await Deno.remove(source + "/untracked");
        await git(main, ["commit", "--allow-empty", "-m", "target moved"]);
        await methods.integrate.execute({ ...input, key: "moved-target" }, c);
        assert(saved.get("integrationDecision-current")?.integrated === false);
        await methods.rebase.execute({ ...args, key: "fresh-rebase" }, c);
        await methods.verify_checkout.execute(args, c);
        const fresh = Evidence.parse(saved.get("evidence-current"));
        const ready = {
            ...input,
            evidence: fresh,
            review: { ...input.review, evidence: fresh },
            test: { ...input.test, evidence: fresh },
        };
        await methods.integrate.execute(ready, c);
        assert(await git(main, ["rev-parse", "HEAD"]) === fresh.source.commit);
        await methods.integrate.execute(ready, c); // identical dispatch retrieves its recorded result
        await rejects(
            () => methods.integrate.execute({ ...ready, gate: ["false"] }, c),
            /Conflicting request/,
        );
        // Local checkout evidence requires no worker operation. Failed and skipped
        // checks can be accepted explicitly, but never replace revision freshness.
        Deno.Command = originalCommand;
        const local = args;
        await methods.verify_checkout.execute(local, c);
        const localProof = Evidence.parse(saved.get("evidence-current"));
        for (
            const checks of [
                { review: null, test: null },
                {
                    review: {
                        ...input.review,
                        evidence: localProof,
                        resolved: false,
                        findings: [{
                            severity: "error" as const,
                            file: "file",
                            message: "Unresolved finding",
                        }],
                    },
                    test: {
                        ...input.test,
                        evidence: localProof,
                        outcome: "failed",
                        exitCode: 1,
                        passed: false,
                    },
                },
            ]
        ) {
            const informed = {
                ...local,
                key: checks.test ? "failed-checks" : "skipped-checks",
                evidence: localProof,
                ...checks,
                approved: true as const,
            };
            await rejects(
                () =>
                    methods.integrate.execute(
                        { ...informed, approved: false } as never,
                        c,
                    ),
                /true/,
            );
            await methods.integrate.execute(informed, c);
            assert(
                saved.get("integrationDecision-current")?.integrated === false,
            );
        }
        for (
            const outcome of [
                "failed",
                "cancelled",
                "stale",
                "pending",
                "unknown",
            ]
        ) {
            for (const side of ["review", "test"] as const) {
                const checks = {
                    review: {
                        ...input.review,
                        evidence: localProof,
                        outcome: side === "review" ? outcome : "completed",
                        valid: true,
                        resolved: true,
                        findings: [],
                    },
                    test: {
                        ...input.test,
                        evidence: localProof,
                        outcome: side === "test" ? outcome : "completed",
                        passed: true,
                        exitCode: 0,
                    },
                };
                await methods.integrate.execute({
                    ...local,
                    key: `reject-${side}-${outcome}`,
                    evidence: localProof,
                    approved: true,
                    ...checks,
                }, c);
                assert(
                    saved.get("integrationDecision-current")?.integrated ===
                        false,
                    `${side}: ${outcome}`,
                );
            }
        }
        await Deno.writeTextFile(
            source + "/late-untracked",
            "changed after approval",
        );
        await methods.integrate.execute({
            ...local,
            key: "stale-skipped",
            evidence: localProof,
            review: null,
            test: null,
            approved: true,
        }, c);
        assert(saved.get("integrationDecision-current")?.integrated === false);
        await Deno.remove(source + "/late-untracked");
            await Deno.writeTextFile(source + "/keep", "important");
        await rejects(
            () =>
                methods.guarded_cleanup.execute({
                    ...args,
                    key: "refused-cleanup",
                    approved: true,
                    commit: fresh.source.commit,
                }, c),
            /changed/,
        );
        assert(await Deno.readTextFile(source + "/keep") === "important");
        await Deno.remove(source + "/keep");
        await methods.guarded_cleanup.execute({
            ...args,
            key: "cleanup",
            approved: true,
            commit: fresh.source.commit,
        }, c);
        await rejects(() => Deno.stat(source), /NotFound/);
    } finally {
        Deno.Command = originalCommand;
        await Deno.remove(dir, { recursive: true });
    }
});
Deno.test("rebase conflicts are inspected and abort is explicit; unknown dispatches are never repeated", async () => {
    const dir = await Deno.makeTempDir();
    try {
        const main = dir + "/main", source = dir + "/topic";
        await Deno.mkdir(main);
        const { c, saved } = context(dir, source);
        const git = (cwd: string, args: string[]) =>
            checked(c, cwd, [
                "git",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                ...args,
            ]);
        await git(main, ["init", "-b", "main"]);
        await Deno.writeTextFile(main + "/file", "base\n");
        await git(main, ["add", "."]);
        await git(main, ["commit", "-m", "base"]);
        await git(main, ["worktree", "add", "-b", "topic", source]);
        const args = {
            source,
            target: main,
            targetBranch: "main",
            gate: ["true"],
            binary: "yinz",
        };
        await Deno.writeTextFile(source + "/file", "topic\n");
        await methods.guarded_commit.execute({
            ...args,
            key: "commit",
            message: "topic",
        }, c);
        await Deno.writeTextFile(main + "/file", "target\n");
        await git(main, ["commit", "-am", "target"]);
        await methods.rebase.execute({ ...args, key: "conflict" }, c);
        const result = saved.get("rebaseResult-current")!;
        assert(result.exitCode !== 0);
        assert(
            Evidence.parse(result.evidence).source.conflicts.includes("file"),
        );
        await methods.rebase_abort.execute({ ...args, key: "abort" }, c);
        assert(await Deno.readTextFile(source + "/file") === "topic\n");
        let dispatched = 0;
        await rejects(() =>
            claimed(c, "lost", {}, () => {
                dispatched++;
                throw new Error("killed");
            }), /killed/);
        assert(Claim.parse(saved.get("claim-lost")).outcome === "unknown");
        await rejects(() =>
            claimed(c, "lost", {}, () => {
                dispatched++;
                return Promise.resolve({});
            }), /unknown/);
        assert(dispatched === 1);
    } finally {
        Deno.Command = originalCommand;
        await Deno.remove(dir, { recursive: true });
    }
});
