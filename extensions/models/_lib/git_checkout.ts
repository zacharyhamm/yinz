/** Checkout evidence and cooperating kernel locks, shared with the pane worker. @module */
import { z } from "npm:zod@4.3.6";
import { checked, Context, digest, exec, Key, Literal } from "./workflow.ts";
import { Operation } from "./bridge.ts";
const VmSync = Operation.extend({
    outcome: z.literal("completed"),
    clean: z.literal(true),
});
export const Sync = z.union([VmSync, z.object({ mode: z.literal("local") })]);
const Ref = Literal.min(1).refine((s) => !s.startsWith("-"));
const Revision = z.object({
    checkout: z.string(),
    branch: z.string(),
    commit: z.string(),
    contents: z.string(),
    clean: z.boolean(),
    conflicts: z.array(z.string()),
    rebasing: z.boolean(),
});
export const Evidence = z.object({
    source: Revision,
    target: Revision,
    gate: z.array(Literal).min(1),
    vmSync: Sync,
});
export const Args = z.object({
    source: Literal.min(1),
    binary: Literal.min(1).default("zellij"),
    target: Literal.min(1),
    targetBranch: Ref.default("main"),
    gate: z.array(Literal).min(1).default(["cargo", "x", "ci", "check"]),
    vmSync: Sync,
});
export const Mutation = Args.extend({ key: Key });
export const CommitAndRebase = Mutation.extend({
    work: Key,
    session: Literal.min(1),
    message: Literal.min(1),
    expectedEvidence: Evidence.optional(),
});
export const git = (c: Pick<Context, "signal">, path: string, args: string[]) =>
    checked(c, path, ["git", ...args]);

export async function checkoutStatus(
    c: Pick<Context, "signal">,
    checkout: string,
) {
    const status = await git(c, checkout, [
        "--no-optional-locks",
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ]);
    let rebasing = false;
    for (const name of ["rebase-merge", "rebase-apply"]) {
        const path = await git(c, checkout, [
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            name,
        ]);
        try {
            await Deno.stat(path);
            rebasing = true;
        } catch (e) {
            if (!(e instanceof Deno.errors.NotFound)) throw e;
        }
    }
    return { status, rebasing };
}

async function revision(c: Pick<Context, "signal">, directory: string) {
    const checkout = await Deno.realPath(directory);
    if (
        await Deno.realPath(
            await git(c, checkout, ["rev-parse", "--show-toplevel"]),
        ) !== checkout
    ) throw new Error("Expected a checkout root");
    const symbolic = await exec(c, checkout, [
        "git",
        "symbolic-ref",
        "--short",
        "HEAD",
    ]);
    let branch = symbolic.stdout.trimEnd();
    if (symbolic.exitCode !== 0) {
        for (
            const name of ["rebase-merge/head-name", "rebase-apply/head-name"]
        ) {
            try {
                branch = (await Deno.readTextFile(
                    await git(c, checkout, [
                        "rev-parse",
                        "--path-format=absolute",
                        "--git-path",
                        name,
                    ]),
                )).trim().replace(/^refs\/heads\//, "");
                break;
            } catch (e) {
                if (!(e instanceof Deno.errors.NotFound)) throw e;
            }
        }
        if (!branch) throw new Error("Detached source has no rebase ownership");
    }
    const commit = await git(c, checkout, ["rev-parse", "HEAD"]);
    const { status, rebasing } = await checkoutStatus(c, checkout);
    const diff = await git(c, checkout, [
        "diff",
        "HEAD",
        "--binary",
        "--no-ext-diff",
        "--no-textconv",
    ]);
    const untracked = (await git(c, checkout, [
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ])).split("\0").filter(Boolean);
    const files = [];
    for (const path of untracked) {
        const info = await Deno.lstat(`${checkout}/${path}`);
        files.push([
            path,
            info.isSymlink
                ? await Deno.readLink(`${checkout}/${path}`)
                : await digest(
                    Array.from(await Deno.readFile(`${checkout}/${path}`)),
                ),
        ]);
    }
    const conflicts = (await git(c, checkout, [
        "diff",
        "--name-only",
        "--diff-filter=U",
        "-z",
    ])).split("\0").filter(Boolean);
    return Revision.parse({
        checkout,
        branch,
        commit,
        contents: await digest([
            status,
            diff,
            await git(c, checkout, [
                "diff",
                "--cached",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
                "--",
            ]),
            files,
        ]),
        clean: status === "",
        conflicts,
        rebasing,
    });
}
export async function evidence(
    c: Pick<Context, "signal">,
    args: z.infer<typeof Args>,
) {
    const source = await revision(c, args.source);
    const target = await revision(c, args.target);
    if (source.checkout === target.checkout) {
        throw new Error("Source and target checkouts must differ");
    }
    if (target.branch !== args.targetBranch) {
        throw new Error(`Target must have ${args.targetBranch} checked out`);
    }
    const common = (path: string) =>
        git(c, path, [
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ]);
    if (
        await Deno.realPath(await common(source.checkout)) !==
            await Deno.realPath(await common(target.checkout))
    ) throw new Error("Checkouts belong to different repositories");
    return Evidence.parse({
        source,
        target,
        gate: args.gate,
        vmSync: args.vmSync,
    });
}
/** Identity a lock holder leaves in the lock file so waiters can name it. */
export const LockHolder = z.object({
    pid: z.number().int().positive(),
    session: z.string(),
    work: z.string(),
    key: z.string(),
    since: z.string(),
});
export type LockWait = {
    path: string;
    holder: z.infer<typeof LockHolder> | null;
};
export type LockOptions = {
    /** Recorded in the lock file while held; absent holders stay anonymous. */
    identity?: Omit<z.infer<typeof LockHolder>, "since">;
    /** Called once the lock is still contended after `noticeSeconds`, then every 30s. */
    onWait?: (wait: LockWait) => void | Promise<void>;
    /** Give up with `LockTimeout` after this long; unbounded when absent. */
    waitSeconds?: number;
    noticeSeconds?: number;
};
export class LockTimeout extends Error {
    constructor(readonly wait: LockWait, seconds: number) {
        super(
            `Checkout lock ${wait.path} still held after ${seconds}s` +
                (wait.holder
                    ? ` by pid ${wait.holder.pid} (session ${wait.holder.session}, step ${wait.holder.key})`
                    : "") + "; dismiss or terminate that worker before retrying",
        );
    }
}
/** Lock file contents are advisory; partial writes and legacy empty files read as anonymous. */
export async function readLockHolder(path: string) {
    try {
        return LockHolder.parse(JSON.parse(await Deno.readTextFile(path)));
    } catch {
        return null;
    }
}
async function writeAll(file: Deno.FsFile, bytes: Uint8Array) {
    let written = 0;
    while (written < bytes.length) {
        written += await file.write(bytes.subarray(written));
    }
}
/** A cancellable pause, so an uncontended lock leaves no timer behind. */
function pause(ms: number, signal: AbortSignal) {
    let cancel = () => {};
    const promise = new Promise<"tick">((resolve, reject) => {
        const stop = () => {
            clearTimeout(timer);
            reject(signal.reason);
        };
        const timer = setTimeout(() => {
            signal.removeEventListener("abort", stop);
            resolve("tick");
        }, ms);
        signal.addEventListener("abort", stop, { once: true });
        if (signal.aborted) stop();
        cancel = () => {
            clearTimeout(timer);
            signal.removeEventListener("abort", stop);
            resolve("tick");
        };
    });
    return { promise, cancel };
}
async function acquire(
    file: Deno.FsFile,
    path: string,
    options: LockOptions,
    signal: AbortSignal,
) {
    const lock = file.lock(true).then(() => "locked" as const);
    const deadline = options.waitSeconds == null
        ? Infinity
        : Date.now() + options.waitSeconds * 1000;
    let notice = (options.noticeSeconds ?? 2) * 1000;
    while (true) {
        const wait = Math.max(0, Math.min(notice, deadline - Date.now()));
        const timer = pause(wait, signal);
        const won = await Promise.race([lock, timer.promise]);
        timer.cancel();
        if (won === "locked") return;
        const holder = await readLockHolder(path);
        if (Date.now() >= deadline) {
            throw new LockTimeout({ path, holder }, options.waitSeconds!);
        }
        await options.onWait?.({ path, holder });
        notice = 30_000;
    }
}
/** Kernel locks release on process death; never unlink their inodes. */
export async function locked<T>(
    c: Pick<Context, "signal">,
    paths: string[],
    action: () => Promise<T>,
    options: LockOptions = {},
) {
    const locks: Deno.FsFile[] = [];
    try {
        const dirs = await Promise.all(
            paths.map(async (p) =>
                await git(c, p, ["rev-parse", "--absolute-git-dir"])
            ),
        );
        for (const dir of [...new Set(dirs)].sort()) {
            const path = `${dir}/zellij-workflow.lock`;
            const file = await Deno.open(path, { create: true, write: true });
            locks.push(file);
            await acquire(file, path, options, c.signal);
            c.signal.throwIfAborted();
            await file.truncate(0);
            if (options.identity) {
                await writeAll(
                    file,
                    new TextEncoder().encode(JSON.stringify(LockHolder.parse({
                        ...options.identity,
                        since: new Date().toISOString(),
                    }))),
                );
            }
        }
        return await action();
    } finally {
        for (const file of locks.reverse()) file.close();
    }
}
