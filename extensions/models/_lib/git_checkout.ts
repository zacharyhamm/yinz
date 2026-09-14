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
/** Kernel locks release on process death; never unlink their inodes. */
export async function locked<T>(
    c: Pick<Context, "signal">,
    paths: string[],
    action: () => Promise<T>,
) {
    const locks: Deno.FsFile[] = [];
    try {
        const dirs = await Promise.all(
            paths.map(async (p) =>
                await git(c, p, ["rev-parse", "--absolute-git-dir"])
            ),
        );
        for (const dir of [...new Set(dirs)].sort()) {
            const file = await Deno.open(`${dir}/zellij-workflow.lock`, {
                create: true,
                write: true,
            });
            locks.push(file);
            await file.lock(true);
            c.signal.throwIfAborted();
        }
        return await action();
    } finally {
        for (const file of locks.reverse()) file.close();
    }
}
