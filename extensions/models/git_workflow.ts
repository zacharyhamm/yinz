/** Local, guarded Git operations extending the installed official model. @module */
import { z } from "npm:zod@4.3.6";
import { model as official } from "../../.swamp/pulled-extensions/@swamp/git/models/git.ts";
import type { GitContext } from "../../.swamp/pulled-extensions/@swamp/git/models/_lib/types.ts";
import {
    checked,
    claimed,
    claimResource,
    Context,
    digest,
    exec,
    Key,
    Literal,
} from "./_lib/workflow.ts";

import { Operation, operation } from "./_lib/bridge.ts";
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
export const Finding = z.object({
    severity: z.enum(["error", "warning", "info"]),
    file: z.string(),
    message: z.string().min(1),
});
const Review = z.object({
    evidence: Evidence,
    outcome: z.string(),
    findings: z.array(Finding),
    reviewer: z.string().min(1),
    valid: z.boolean(),
    resolved: z.boolean(),
});
const Test = z.object({
    evidence: Evidence,
    outcome: z.string(),
    exitCode: z.number().int().nullable(),
    log: z.string().min(1),
    passed: z.boolean(),
});
const Args = z.object({
    source: Literal.min(1),
    binary: Literal.min(1).default("zellij"),
    target: Literal.min(1),
    targetBranch: Ref.default("main"),
    gate: z.array(Literal).min(1).default(["cargo", "x", "ci", "check"]),
    vmSync: Sync,
});
const Mutation = Args.extend({ key: Key });
const CheckoutStatusArgs = Args.pick({ source: true, targetBranch: true });
const Integration = Mutation.extend({
    evidence: Evidence,
    review: Review.nullable(),
    test: Test.nullable(),
    approved: z.literal(true),
});
const git = (c: Context, path: string, args: string[]) =>
    checked(c, path, ["git", ...args]);

async function checkoutStatus(c: Context, checkout: string) {
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

async function revision(c: Context, directory: string) {
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
        contents: await digest([status, diff, files]),
        clean: status === "",
        conflicts,
        rebasing,
    });
}
async function evidence(c: Context, args: z.infer<typeof Args>) {
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
async function liveVm(c: Context, args: z.infer<typeof Args>) {
    if ("mode" in args.vmSync) return true;
    const owner = args.vmSync.owner;
    if (!owner) throw new Error("VM sync evidence lacks agent ownership");
    const current = await operation({
        ...c,
        globalArgs: { session: owner.session, binary: args.binary },
    }, {
        method: "vm_status",
        work: owner.work,
        key: "verify-vm",
        agent: owner.agent,
    });
    if (
        JSON.stringify(current.owner) !== JSON.stringify(owner)
    ) throw new Error("VM sync evidence belongs to a different agent");
    return (current.vm ?? null) === (args.vmSync.vm ?? null) &&
        current.clean === true;
}
/** Kernel locks release on process death; never unlink their inodes. */
async function locked<T>(
    c: Context,
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
            await file.lock();
            c.signal.throwIfAborted();
        }
        return await action();
    } finally {
        for (const file of locks.reverse()) file.close();
    }
}
const resource = <T extends z.ZodType>(schema: T, description: string) => ({
    schema,
    description,
    lifetime: "infinite",
    garbageCollection: 50,
});
async function save(c: Context, spec: string, value: Record<string, unknown>) {
    c.logger.info("Git workflow operation completed");
    return {
        dataHandles: [await c.writeResource(spec, spec + "-current", value)],
    };
}
async function mutate(
    args: z.infer<typeof Mutation>,
    c: Context,
    mode: "rebase" | "continue" | "abort",
) {
    c.logger.info("Checking checkout before Git mutation");
    const result = await locked(
        c,
        [args.source, args.target],
        () =>
            claimed(c, args.key, { mode, ...args }, async () => {
                const before = await evidence(c, args);
                if (
                    !before.target.clean || before.target.rebasing
                ) throw new Error("Target checkout is dirty or rebasing");
                if (
                    mode === "rebase" &&
                    (!before.source.clean || before.source.rebasing)
                ) {
                    throw new Error(
                        "Source checkout is dirty or already rebasing",
                    );
                }
                if (
                    mode !== "rebase" && !before.source.rebasing
                ) throw new Error("No rebase is in progress");
                const result = await exec(c, before.source.checkout, [
                    "git",
                    "rebase",
                    ...(mode === "rebase"
                        ? [before.target.commit]
                        : [`--${mode}`]),
                ]);
                return { ...result, evidence: await evidence(c, args) };
            }),
    );
    return save(c, "rebaseResult", RebaseResult.parse(result));
}
const RebaseResult = z.object({
    exitCode: z.number().int(),
    stdout: z.string(),
    stderr: z.string(),
    evidence: Evidence,
});
/** Additional methods; the official commit and non-force cleanup remain available. */
export const extension = {
    type: "@swamp/git",
    resources: {
        checkoutStatus: resource(
            z.object({ unchanged: z.boolean(), error: z.string().nullable() }),
            "Whether the local checkout is clean, not rebasing, and has no commits beyond the target branch",
        ),
        diff: resource(
            z.object({ evidence: Evidence, output: z.string() }),
            "Saved branch, staged, unstaged and untracked changes",
        ),
        claim: claimResource,
        evidence: resource(
            Evidence,
            "Source, target, contents, gate and VM sync identity",
        ),
        rebaseResult: resource(
            RebaseResult,
            "Actual rebase result and unresolved conflicts",
        ),
        integration: resource(Evidence, "Verified fast-forward result"),
        integrationDecision: resource(
            z.object({
                integrated: z.boolean(),
                reason: z.string().nullable(),
                evidence: Evidence,
            }),
            "Integration result or the evidence invalidation that requires a new review cycle",
        ),
    },
    methods: [{
        checkout_status: {
            description:
                "Inspect local checkout changes without mutation; inspection errors preserve existing exit choices",
            arguments: CheckoutStatusArgs,
            execute: async (
                input: z.infer<typeof CheckoutStatusArgs>,
                c: Context,
            ) => {
                let unchanged = false;
                let error: string | null = null;
                try {
                    const args = CheckoutStatusArgs.parse(input);
                    const checkout = await Deno.realPath(args.source);
                    const variables = await git(c, checkout, [
                        "rev-parse",
                        "--local-env-vars",
                    ]);
                    if (variables.split("\n").some((v) => Deno.env.has(v))) {
                        throw new Error(
                            "Inherited Git environment prevents checkout inspection",
                        );
                    }
                    if (
                        await Deno.realPath(
                            await git(c, checkout, [
                                "rev-parse",
                                "--show-toplevel",
                            ]),
                        ) !== checkout
                    ) throw new Error("Expected a checkout root");
                    await git(c, checkout, [
                        "check-ref-format",
                        "refs/heads/" + args.targetBranch,
                    ]);
                    const ahead = await git(c, checkout, [
                        "rev-list",
                        "--count",
                        "refs/heads/" + args.targetBranch + "..HEAD",
                        "--",
                    ]);
                    const { status, rebasing } = await checkoutStatus(
                        c,
                        checkout,
                    );
                    unchanged = status === "" && !rebasing && ahead === "0";
                } catch (e) {
                    c.signal.throwIfAborted();
                    error = String(e);
                }
                return save(c, "checkoutStatus", { unchanged, error });
            },
        },
        guarded_commit: {
            description:
                "Run the official commit method under the cooperating checkout lock",
            arguments: Mutation.extend({ message: z.string().min(1) }),
            execute: (
                args: z.infer<typeof Mutation> & { message: string },
                c: Context & GitContext,
            ) => locked(
                c,
                [args.source],
                () =>
                    claimed(c, args.key, args, async () => {
                        const current = await evidence(c, args);
                        if (
                            current.source.rebasing ||
                            current.source.conflicts.length
                        ) {
                            throw new Error(
                                "Resolve the rebase before committing",
                            );
                        }
                        if (current.source.clean) return { dataHandles: [] };
                        return official.methods.commit.execute({
                            message: args.message,
                            addAll: true,
                        }, {
                            ...c,
                            globalArgs: {
                                ...c.globalArgs,
                                repoPath: args.source,
                            },
                        });
                    }),
            ),
        },
        guarded_cleanup: {
            description:
                "Use official non-force cleanup after approved, verified integration",
            arguments: Mutation.extend({
                approved: z.literal(true),
                commit: z.string().regex(/^[a-f0-9]{40,64}$/),
            }),
            execute: (
                input: z.infer<typeof Mutation> & {
                    approved: true;
                    commit: string;
                },
                c: Context & GitContext,
            ) => {
                const args = Mutation.extend({
                    approved: z.literal(true),
                    commit: z.string().regex(/^[a-f0-9]{40,64}$/),
                }).parse(input);
                c.logger.info("Verifying approved worktree cleanup");
                return claimed(c, args.key, args, () =>
                    locked(
                        c,
                        [args.source, args.target],
                        async () => {
                            if (!await liveVm(c, args)) {
                                throw new Error(
                                    "Guest changes or sync conflicts remain unresolved",
                                );
                            }
                            const current = await evidence(c, args);
                            if (
                                !current.source.clean ||
                                current.source.rebasing ||
                                current.source.commit !== args.commit
                            ) {
                                throw new Error(
                                    "Cleanup source changed after integration",
                                );
                            }
                            await git(c, current.target.checkout, [
                                "merge-base",
                                "--is-ancestor",
                                args.commit,
                                current.target.commit,
                            ]);
                            return official.methods.remove_worktree.execute({
                                path: current.source.checkout,
                            }, {
                                ...c,
                                globalArgs: {
                                    ...c.globalArgs,
                                    repoPath: args.source,
                                },
                            });
                        },
                    ));
            },
        },
        show_diff: {
            description: "Save all checkout changes without mutation",
            arguments: Args,
            execute: async (input: z.infer<typeof Args>, c: Context) => {
                const args = Args.parse(input);
                const current = await evidence(c, args);
                const output = [
                    "Committed branch changes:\n" +
                    await git(c, args.source, [
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        current.target.commit + "...HEAD",
                        "--",
                    ]),
                    "Staged changes:\n" +
                    await git(c, args.source, [
                        "diff",
                        "--cached",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--",
                    ]),
                    "Unstaged changes:\n" +
                    await git(c, args.source, [
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--",
                    ]),
                ];
                const paths = (await git(c, args.source, [
                    "ls-files",
                    "--others",
                    "--exclude-standard",
                    "-z",
                ])).split("\0").filter(Boolean);
                for (const path of paths) {
                    const full = `${current.source.checkout}/${path}`;
                    const info = await Deno.lstat(full);
                    const bytes = info.isSymlink
                        ? new TextEncoder().encode(await Deno.readLink(full))
                        : await Deno.readFile(full);
                    output.push(
                        `Untracked ${JSON.stringify(path)}:\n` +
                            (bytes.includes(0)
                                ? `[binary: ${bytes.length} bytes]`
                                : new TextDecoder().decode(bytes)),
                    );
                }
                return save(c, "diff", {
                    evidence: current,
                    output: output.join("\n\n"),
                });
            },
        },
        verify_checkout: {
            description: "Capture revision-bound review and gate inputs",
            arguments: Args,
            execute: async (args: z.infer<typeof Args>, c: Context) => {
                c.logger.info("Inspecting source and target checkouts");
                return save(c, "evidence", await evidence(c, Args.parse(args)));
            },
        },
        rebase: {
            description: "Rebase clean source onto the verified local target",
            arguments: Mutation,
            execute: (args: z.infer<typeof Mutation>, c: Context) =>
                mutate(Mutation.parse(args), c, "rebase"),
        },
        rebase_continue: {
            description: "Continue an explicitly resolved rebase",
            arguments: Mutation,
            execute: (args: z.infer<typeof Mutation>, c: Context) =>
                mutate(Mutation.parse(args), c, "continue"),
        },
        rebase_abort: {
            description:
                "Abort an inspected rebase without discarding unrelated work",
            arguments: Mutation,
            execute: (args: z.infer<typeof Mutation>, c: Context) =>
                mutate(Mutation.parse(args), c, "abort"),
        },
        integrate: {
            description:
                "Fast-forward under both checkout locks with revision-bound optional checks and explicit approval",
            arguments: Integration,
            execute: async (input: z.infer<typeof Integration>, c: Context) => {
                const args = Integration.parse(input);
                c.logger.info(
                    "Verifying approval and evidence for local integration",
                );
                const result = await locked(c, [
                    args.source,
                    args.target,
                ], () =>
                    claimed(c, args.key, args, async () => {
                        const vmClean = await liveVm(c, args);
                        const current = await evidence(c, args);
                        if (!vmClean) {
                            return {
                                integrated: false,
                                reason:
                                    "Guest changes require sync and fresh evidence",
                                evidence: current,
                            };
                        }
                        for (
                            const proof of [
                                args.evidence,
                                ...(args.review ? [args.review.evidence] : []),
                                ...(args.test ? [args.test.evidence] : []),
                            ]
                        ) {
                            if (await digest(current) !== await digest(proof)) {
                                return {
                                    integrated: false,
                                    reason:
                                        "Stale evidence: repeat preparation and approval",
                                    evidence: current,
                                };
                            }
                        }
                        if (
                            ![current.source, current.target].every((r) =>
                                r.clean && !r.rebasing &&
                                r.conflicts.length === 0
                            )
                        ) throw new Error("Checkout has unresolved changes");
                        await git(c, current.target.checkout, [
                            "merge-base",
                            "--is-ancestor",
                            current.target.commit,
                            current.source.commit,
                        ]);
                        await git(c, current.target.checkout, [
                            "merge",
                            "--ff-only",
                            "--no-edit",
                            current.source.commit,
                        ]);
                        return {
                            integrated: true,
                            reason: null,
                            evidence: await evidence(c, args),
                        };
                    }));
                const handles = [
                    await c.writeResource(
                        "integrationDecision",
                        "integrationDecision-current",
                        result,
                    ),
                ];
                if (result.integrated) {
                    handles.push(
                        await c.writeResource(
                            "integration",
                            "integration-current",
                            Evidence.parse(result.evidence),
                        ),
                    );
                }
                return { dataHandles: handles };
            },
        },
    }],
};
