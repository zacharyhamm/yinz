/** The supervised pane owns Git and its locks; cancellation never implies rollback. */
import { z } from "npm:zod@4.3.6";
import {
    CommitAndRebase,
    Evidence,
    evidence,
    git,
    locked,
    LockTimeout,
    type LockWait,
    Mutation,
} from "./git_checkout.ts";
import { operation } from "./bridge.ts";
import { Context, digest, Literal } from "./workflow.ts";
import type { GitContext } from "../../../.swamp/pulled-extensions/@swamp/git/models/_lib/types.ts";
import {
    Cleanup,
    cleanupCheckout,
    integrateCheckout,
    Integration,
} from "../git_workflow.ts";
import { setCommandExecutor } from "../../../.swamp/pulled-extensions/@swamp/git/models/_lib/runner.ts";

const args = z.discriminatedUnion("mode", [
    CommitAndRebase.extend({ mode: z.literal("commit") }),
    Mutation.extend({
        mode: z.literal("continue"),
        evidence: Evidence.optional(),
    }),
    Integration.extend({ mode: z.literal("integrate") }),
    Cleanup.extend({ mode: z.literal("cleanup") }),
]).and(z.object({
    work: z.string().min(1),
    session: Literal.min(1),
    author: z.array(Literal),
    directory: Literal.min(1),
    repoDir: Literal.min(1),
})).parse(JSON.parse(Deno.args[0]));
const label = {
    commit: "Commit and rebase",
    continue: "Continue rebase",
    integrate: "Merge",
    cleanup: "Cleanup",
}[args.mode];
// Test fixtures shorten these; production keeps a stranded holder visible, then fails.
const seconds = (name: string, fallback: number) => {
    const raw = Deno.env.get(name);
    if (!raw) return fallback;
    const value = Number(raw);
    return Number.isFinite(value) && value >= 0 ? value : fallback;
};
const LOCK_WAIT_SECONDS = seconds("ZELLIJ_COMMIT_LOCK_WAIT_SECONDS", 600);
const LOCK_NOTICE_SECONDS = seconds("ZELLIJ_COMMIT_LOCK_NOTICE_SECONDS", 2);
const SESSION_POLL_SECONDS = seconds("ZELLIJ_COMMIT_SESSION_POLL_SECONDS", 30);
let cancelled = false, dismissing = false;
const records = new Map<string, Record<string, unknown>>();
const c: Context & GitContext = {
    signal: new AbortController().signal,
    repoDir: args.repoDir,
    globalArgs: {},
    logger: { info() {}, debug() {}, warn() {}, error() {} },
    readResource: (name) => Promise.resolve(records.get(name) ?? null),
    writeResource: (_spec, name, value) => {
        records.set(name, value);
        return Promise.resolve({ name });
    },
};
// The server can carry the Git environment of a different checkout.
for (
    const name of (await git(c, args.source, ["rev-parse", "--local-env-vars"]))
        .split("\n")
) {
    Deno.env.delete(name);
}
// Direct writes: a WritableStream over the file buffers until close, which
// leaves output.log empty for every reader while the pane is still alive.
const file = await Deno.open(`${args.directory}/output.log`, {
    create: true,
    write: true,
});
const writeAll = async (
    target: { write(bytes: Uint8Array): Promise<number> },
    bytes: Uint8Array,
) => {
    let written = 0;
    while (written < bytes.length) {
        written += await target.write(bytes.subarray(written));
    }
};
const emit = async (bytes: Uint8Array) => {
    await writeAll(file, bytes);
    await writeAll(Deno.stdout, bytes);
};
const print = (text: string) => emit(new TextEncoder().encode(text));
Deno.addSignalListener("SIGINT", () => {
    if (dismissing) Deno.exit(exitCode);
    if (cancelled) return;
    cancelled = true;
    void operation(c, {
        method: "cancel_step",
        work: args.work,
        operation: args.key,
        key: args.key,
        session: args.session,
        binary: args.binary,
    }).catch((error) => print(String(error) + "\n"));
});
const run = async (cwd: string, argv: string[]) => {
    await print("$ " + argv.map((a) => JSON.stringify(a)).join(" ") + "\n");
    if (cancelled) {
        throw new Error("Step cancelled; inspect the checkout before retrying");
    }
    const child = new Deno.Command("bash", {
        args: ["-c", 'exec "$@" 2>&1', "foreground-command", ...argv],
        cwd,
        env: { GIT_TERMINAL_PROMPT: "0", GIT_EDITOR: "true", GIT_PAGER: "cat" },
        stdin: "inherit",
        stdout: "piped",
        stderr: "inherit",
    }).spawn();
    let stdout = "";
    const decoder = new TextDecoder();
    for await (const bytes of child.stdout) {
        await emit(bytes);
        stdout += decoder.decode(bytes, { stream: true });
    }
    stdout += decoder.decode();
    return { exitCode: (await child.status).code, stdout, stderr: "" };
};
// Reuse the official cleanup checks and non-force removal, with live command output.
setCommandExecutor((argv, opts) => run(opts?.cwd ?? args.target, argv));
/** Whether `session` is listed; null when the listing itself failed. */
const sessionListed = async (session: string) => {
    try {
        const listing = await new Deno.Command(args.binary, {
            args: ["list-sessions", "--short", "--no-formatting"],
            cwd: args.repoDir,
            stdin: "null",
            stdout: "piped",
            stderr: "piped",
        }).output();
        const text = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
        if (listing.code === 0) {
            return text(listing.stdout).split("\n").includes(session);
        }
        return text(listing.stderr).includes("No active zellij sessions")
            ? false
            : null;
    } catch {
        return null;
    }
};
const describeWait = async ({ path, holder }: LockWait) => {
    let text = `Waiting for checkout lock ${path}`;
    if (holder) {
        text += ` held by pid ${holder.pid} since ${holder.since}` +
            ` (session ${holder.session}, step ${holder.key}).` +
            " Dismiss that pane, or press Ctrl-C to cancel this step.";
        const stranded = await sessionListed(holder.session);
        if (stranded === false) {
            text += `\nSession ${holder.session} no longer exists;` +
                ` that worker is stranded. Terminate pid ${holder.pid} to continue.`;
        }
    } else {
        text += " held by another process (no holder record)." +
            " Dismiss that pane, or press Ctrl-C to cancel this step.";
    }
    await print(text + "\n");
};
let exitCode = 125;
/** Save, announce, notify, then hold the pane until Enter or session loss. */
const finish = async (result: Record<string, unknown>) => {
    if (cancelled) {
        exitCode = 130;
        result = { ...result, exitCode, cancelled: true };
    }
    await Deno.writeTextFile(
        `${args.directory}/result.json`,
        JSON.stringify(result),
    );
    await print(`\n${label} finished (exit ${exitCode}). Press Enter to return.\n`);
    await operation(c, {
        method: "step_finished",
        work: args.work,
        key: args.key,
        operation: args.key,
        process: Deno.pid,
        session: args.session,
        binary: args.binary,
    })
        .catch((error) =>
            print("Completion notification failed: " + String(error) + "\n")
        );
    dismissing = true;
    const byte = new Uint8Array(1);
    let read = Deno.stdin.read(byte);
    while (true) {
        let timer: ReturnType<typeof setTimeout> | undefined;
        const tick = new Promise<"tick">((resolve) => {
            timer = setTimeout(() => resolve("tick"), SESSION_POLL_SECONDS * 1000);
        });
        const outcome = await Promise.race([read, tick]);
        clearTimeout(timer);
        if (outcome === "tick") {
            // A pane nobody can dismiss must not keep the checkout locked.
            if (await sessionListed(args.session) === false) {
                await print(
                    `Session ${args.session} is gone; releasing checkout locks.\n`,
                );
                break;
            }
            continue;
        }
        if (outcome === null) {
            exitCode = 125;
            break;
        }
        if (byte[0] === 10) break;
        read = Deno.stdin.read(byte);
    }
};
try {
    await print(
        `${label}: ${args.source} onto ${args.targetBranch} in ${args.target}\n`,
    );
    try {
        // Locks cover dismissal so a live developer can inspect the outcome
        // before another mutation; they are released on lock-wait timeout,
        // cancellation, or when the owning session disappears.
        await locked(c, [args.source, args.target], async () => {
            let result;
            try {
                if (cancelled) {
                    throw new Error(
                        "Step cancelled; inspect the checkout before retrying",
                    );
                }
                if (args.mode === "integrate") {
                    const decision = await integrateCheckout(
                        args,
                        c,
                        async (cwd, argv) => {
                            const result = await run(cwd, argv);
                            if (result.exitCode !== 0) {
                                throw new Error(result.stdout);
                            }
                            return result.stdout;
                        },
                    );
                    exitCode = 0;
                    result = { exitCode, ...decision };
                    if (decision.reason) await print(decision.reason + "\n");
                } else if (args.mode === "cleanup") {
                    await cleanupCheckout(args, c);
                    exitCode = 0;
                    result = { exitCode, removal: records.get("remove-worktree") };
                } else if (args.mode === "continue") {
                    const before = await evidence(c, args);
                    if (
                        args.evidence &&
                        await digest(before) !== await digest(args.evidence)
                    ) {
                        throw new Error(
                            "Stale rebase choice: inspect the checkout again",
                        );
                    }
                    if (!before.target.clean || before.target.rebasing) {
                        throw new Error("Target checkout is dirty or rebasing");
                    }
                    if (!before.source.rebasing || before.source.conflicts.length) {
                        throw new Error("Rebase is not ready to continue");
                    }
                    exitCode =
                        (await run(args.source, ["git", "rebase", "--continue"]))
                            .exitCode;
                    result = { exitCode, evidence: await evidence(c, args) };
                } else {
                    const before = await evidence(c, args);
                    if (
                        args.expectedEvidence &&
                        await digest(before) !== await digest(args.expectedEvidence)
                    ) {
                        throw new Error(
                            "Checkout changed since commit message preparation; generate a new message before committing",
                        );
                    }
                    if (!before.target.clean || before.target.rebasing) {
                        throw new Error("Target checkout is dirty or rebasing");
                    }
                    if (before.source.rebasing || before.source.conflicts.length) {
                        throw new Error("Resolve the rebase before committing");
                    }
                    if (cancelled) {
                        throw new Error(
                            "Step cancelled; inspect the checkout before retrying",
                        );
                    }
                    const child = new Deno.Command("bash", {
                        args: [
                            "-c",
                            String.raw`
set -eu
trap 'exit 130' INT
exec 2>&1
target=$1 message=$2 dirty=$3
shift 3
run() {
    printf '$'
    printf ' %q' "$@"
    printf '\n'
    "$@"
}
if [ "$dirty" = true ]; then
    run git add -A
    run git "$@" commit -m "$message"
else
    printf 'No changes to commit.\n'
fi
run git rebase "$target"
`,
                            "commit-and-rebase",
                            before.target.commit,
                            args.message,
                            String(!before.source.clean),
                            ...args.author,
                        ],
                        cwd: before.source.checkout,
                        env: {
                            GIT_TERMINAL_PROMPT: "0",
                            GIT_EDITOR: "true",
                            GIT_PAGER: "cat",
                        },
                        stdin: "inherit",
                        stdout: "piped",
                        stderr: "inherit",
                    }).spawn();
                    for await (const bytes of child.stdout) await emit(bytes);
                    exitCode = (await child.status).code;
                    result = { exitCode, evidence: await evidence(c, args) };
                }
            } catch (error) {
                exitCode = 124;
                result = {
                    exitCode,
                    error: String(error),
                    evidence: await evidence(c, args),
                };
                await print(`${error}\n`);
            }
            await finish(result);
        }, {
            identity: {
                pid: Deno.pid,
                session: args.session,
                work: args.work,
                key: args.key,
            },
            onWait: describeWait,
            waitSeconds: LOCK_WAIT_SECONDS,
            noticeSeconds: LOCK_NOTICE_SECONDS,
        });
    } catch (error) {
        if (!(error instanceof LockTimeout)) throw error;
        // Never acquired: nothing to protect, so dismissal holds no lock.
        exitCode = 124;
        await print(`${error}\n`);
        await finish({
            exitCode,
            error: String(error),
            evidence: await evidence(c, args),
        });
    }
} finally {
    file.close();
}
Deno.exit(exitCode);
