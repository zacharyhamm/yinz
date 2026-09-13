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

import { observe, Operation, operation } from "./_lib/bridge.ts";
import {
    Args,
    checkoutStatus,
    CommitAndRebase,
    Evidence,
    evidence,
    git,
    locked,
    Mutation,
    Sync,
} from "./_lib/git_checkout.ts";
import { outputPreview, OutputRef } from "./_lib/evidence.ts";
export { Evidence, Finding, Sync };

const Finding = z.object({
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
const CheckoutStatusArgs = Args.pick({ source: true, targetBranch: true });
export const Integration = Mutation.extend({
    evidence: Evidence,
    review: Review.nullable(),
    test: Test.nullable(),
    approved: z.literal(true),
});
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

// Saved method calls without pane ownership retain their guarded internal execution.
const Placement = z.object({
    work: Key.optional(),
    session: Literal.min(1).optional(),
});
export const Cleanup = Mutation.extend({
    approved: z.literal(true),
    commit: z.string().regex(/^[a-f0-9]{40,64}$/),
});
/** Execute under the caller's checkout locks. */
export async function cleanupCheckout(
    args: z.infer<typeof Cleanup>,
    c: Context & GitContext,
) {
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
}
/** Execute under the caller's checkout locks. */
export async function integrateCheckout(
    args: z.infer<typeof Integration>,
    c: Context,
    run = (cwd: string, argv: string[]) => checked(c, cwd, argv),
) {
    const vmClean = await liveVm(c, args);
    const current = await evidence(c, args);
    if (!vmClean) {
        return {
            integrated: false,
            reason: "Guest changes require sync and fresh evidence",
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
                reason: "Stale evidence: repeat preparation and approval",
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
    await run(current.target.checkout, [
        "git",
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
    operation: Operation.optional(),
    outputRef: OutputRef.optional(),
});
async function foreground(
    args: z.infer<typeof Mutation> & { work: string; session: string },
    c: Context,
    mode: "commit" | "continue" | "integrate" | "cleanup",
) {
    return await claimed(c, args.key, { mode, ...args }, async () => {
        if (!c.modelType || !c.modelId) {
            throw new Error(
                "Saved output model identity unavailable",
            );
        }
        const author: string[] = [];
        for (
            const [key, config] of [["authorName", "user.name"], [
                "authorEmail",
                "user.email",
            ]]
        ) {
            const value = c.globalArgs[key];
            if (value) {
                author.push(
                    "-c",
                    `${config}=${Literal.parse(value)}`,
                );
            }
        }
        const { denoPath } = z.object({ denoPath: Literal.min(1) })
            .parse(
                JSON.parse(
                    await checked(c, c.repoDir, [
                        "swamp",
                        "doctor",
                        "extensions",
                        "--json",
                    ]),
                ),
            );
        const worker = `${c.repoDir}/extensions/models/_lib/commit_worker.ts`;
        await Deno.stat(worker);
        // Retain files on uncertain observation: the independent worker may still be writing.
        const directory = await Deno.makeTempDir({
            prefix: "zellij-commit-",
        });
        const request = {
            work: args.work,
            key: args.key,
            session: args.session,
            binary: args.binary,
        };
        await operation(c, {
            ...request,
            method: "command",
            foreground: true,
            cwd: mode === "cleanup" ? args.target : args.source,
            argv: [
                denoPath,
                "run",
                "--no-config",
                "--allow-all",
                worker,
                JSON.stringify({
                    ...args,
                    mode,
                    author,
                    directory,
                    repoDir: c.repoDir,
                }),
            ],
        });
        const command = await observe(c, request, 3600, false);
        if (
            !["completed", "failed"].includes(command.outcome) ||
            command.exitCode == null || command.exitCode === 125 ||
            command.exitCode > 128
        ) {
            throw new Error(
                "Foreground command was interrupted or its completion is uncertain; inspect the saved operation before proceeding",
            );
        }
        const saved = JSON.parse(
            await Deno.readTextFile(`${directory}/result.json`),
        );
        const completed = z.object({
            exitCode: z.number().int(),
        }).passthrough().parse(saved);
        if (completed.exitCode !== command.exitCode) {
            throw new Error(
                "Worker exit status does not match its saved result",
            );
        }
        const output = await Deno.readTextFile(
            `${directory}/output.log`,
        );
        const handle = await c.writeResource(
            "commandOutput",
            `command-output-${args.key}`,
            { output },
        );
        const outputRef = OutputRef.parse({
            modelType: c.modelType.normalized,
            modelId: c.modelId,
            ...handle,
        });
        delete command.output;
        const result = {
            ...completed,
            stdout: outputPreview(output),
            stderr: "",
            outputRef,
            operation: command,
        };
        c.signal.throwIfAborted();
        if (saved.error) throw new Error(String(saved.error));
        await Deno.remove(directory, { recursive: true });
        return result;
    });
}
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
        commandOutput: resource(
            z.object({ output: z.string() }),
            "Full foreground command output",
        ),
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
                exitCode: z.number().int().optional(),
                stdout: z.string().optional(),
                stderr: z.string().optional(),
                operation: Operation.optional(),
                outputRef: OutputRef.optional(),
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
        commit_and_rebase: {
            description:
                "Commit and rebase in a foreground pane, waiting for dismissal",
            arguments: CommitAndRebase,
            execute: async (
                input: z.infer<typeof CommitAndRebase>,
                c: Context,
            ) => {
                const args = CommitAndRebase.parse(input);
                const result = await foreground(args, c, "commit");
                return save(c, "rebaseResult", RebaseResult.parse(result));
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
            arguments: Cleanup.extend(Placement.shape),
            execute: async (
                input: z.infer<typeof Cleanup> & z.infer<typeof Placement>,
                c: Context & GitContext,
            ) => {
                const args = Cleanup.extend(Placement.shape).parse(input);
                if (args.work || args.session) {
                    const result = await foreground(
                        { ...args, ...Placement.required().parse(args) },
                        c,
                        "cleanup",
                    );
                    const removal = official.resources.removeWorktreeResult
                        .schema.parse(result.removal);
                    return {
                        dataHandles: [
                            await c.writeResource(
                                "removeWorktreeResult",
                                "remove-worktree",
                                removal,
                            ),
                        ],
                    };
                }
                return claimed(
                    c,
                    args.key,
                    args,
                    () =>
                        locked(
                            c,
                            [args.source, args.target],
                            () => cleanupCheckout(args, c),
                        ),
                );
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
            arguments: Mutation.extend(Placement.shape).extend({
                evidence: Evidence.optional(),
            }),
            execute: async (
                input: z.infer<typeof Mutation> & z.infer<typeof Placement> & {
                    evidence?: z.infer<typeof Evidence>;
                },
                c: Context,
            ) => {
                const args = Mutation.extend(Placement.shape).extend({
                    evidence: Evidence.optional(),
                }).parse(input);
                return args.work || args.session
                    ? save(
                        c,
                        "rebaseResult",
                        RebaseResult.parse(
                            await foreground(
                                {
                                    ...args,
                                    ...Placement.required().parse(args),
                                },
                                c,
                                "continue",
                            ),
                        ),
                    )
                    : mutate(args, c, "continue");
            },
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
            arguments: Integration.extend(Placement.shape),
            execute: async (
                input: z.infer<typeof Integration> & z.infer<typeof Placement>,
                c: Context,
            ) => {
                const args = Integration.extend(Placement.shape).parse(input);
                c.logger.info(
                    "Verifying approval and evidence for local integration",
                );
                const result = args.work || args.session
                    ? await foreground(
                        { ...args, ...Placement.required().parse(args) },
                        c,
                        "integrate",
                    )
                    : await locked(
                        c,
                        [
                            args.source,
                            args.target,
                        ],
                        () =>
                            claimed(
                                c,
                                args.key,
                                args,
                                () => integrateCheckout(args, c),
                            ),
                    );
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
