/** Host operations and evidence adapters for the existing Yinz model. @module */
import { z } from "npm:zod@4.3.6";
import {
    AgentSpec,
    bridge,
    observe,
    Operation,
    operation,
    Owner,
    preparePrompt,
    PromptText,
    Request,
    STARTUP_PROMPT_BYTES,
} from "./bridge.ts";
import { OutputRef, readEvidence, resolveEvidence } from "./evidence.ts";
import { Context, digest, Key, Literal } from "./workflow.ts";
import {
    Capture,
    captureCommand,
    readCapture,
    ReportFields,
    testReport,
} from "./test_report.ts";
import { Evidence, Finding, mergeCheckProblem, Sync } from "../git_workflow.ts";
const Agent = Request.extend({ agent: Key });
const Spawn = Request.extend({
    spec: AgentSpec,
    waitForExit: z.boolean().default(false),
});
const Prompt = Agent.extend({
    prompt: PromptText,
    waitForExit: z.boolean().default(false),
});
const Observe = Request.extend({
    timeoutSeconds: z.number().int().positive().default(3600),
});
const Command = Request.extend({
    argv: z.array(Literal).min(1),
    cwd: Literal.min(1),
    foreground: z.boolean().default(false),
    captureOutput: z.boolean().default(false),
});
const Review = z.object({
    evidence: Evidence,
    outcome: z.string(),
    findings: z.array(Finding),
    reviewer: z.string(),
    valid: z.boolean(),
    resolved: z.boolean(),
    problem: z.string().nullable(),
});
const Test = ReportFields.extend({
    evidence: Evidence,
    outcome: z.string(),
    exitCode: z.number().int().nullable(),
    log: z.string(),
    output: z.string().default(""),
    outputRef: OutputRef.optional(),
    passed: z.boolean(),
    operationFingerprint: z.string().optional(),
});
const ReviewArgs = z.object({
    operation: Operation,
    evidence: Evidence,
    worker: Key,
});
const CommitMessage = z.object({
    message: Literal.nullable(),
    valid: z.boolean(),
    problem: z.string().nullable(),
    evidence: Evidence,
    owner: Owner.nullable(),
});
const CommitMessageArgs = z.object({
    operation: Operation,
    evidence: Evidence,
    agent: Key,
});
const Checks = z.object({
    evidence: Evidence,
    review: Review.nullable(),
    test: Test.nullable(),
    tests: z.enum([
        "missing",
        "current",
        "stale",
        "failed",
        "cancelled",
        "incomplete",
    ]),
    codeReview: z.enum([
        "missing",
        "current",
        "stale",
        "failed",
        "cancelled",
        "incomplete",
    ]),
    ready: z.boolean(),
    reason: z.string().nullable(),
    recommendation: z.string(),
    repairReview: z.boolean(),
    repairTests: z.boolean(),
});
const CheckArgs = Checks.omit({
    tests: true,
    codeReview: true,
    ready: true,
    reason: true,
    recommendation: true,
    repairReview: true,
    repairTests: true,
});
const Assessment = z.object({
    review: Review,
    test: Test.nullable(),
    ready: z.boolean(),
});
const AssessmentArgs = z.object({
    review: Review,
    test: Test.nullable(),
    current: Evidence,
});
const TestArgs = z.object({ operation: Operation, evidence: Evidence });
const resource = <T extends z.ZodType>(schema: T, description: string) => ({
    schema,
    description,
    lifetime: "infinite",
    garbageCollection: 50,
});
export const resources = {
    commitMessage: resource(
        CommitMessage,
        "Generated commit subject and body bound to the inspected checkout and authoring agent",
    ),
    capture: resource(
        Capture,
        "Private command capture pending durable test evidence",
    ),
    continuation: resource(
        z.object({ fingerprint: z.string(), outputRef: OutputRef }),
        "Exact test evidence chosen for a continuation retry",
    ),
    prompt: resource(
        z.object({ prompt: PromptText }),
        "Validated helper prompt with beginning and end excerpts when oversized",
    ),
    checks: resource(
        Checks,
        "Current revision and required check results",
    ),
    checkoutSync: resource(
        Sync,
        "Local checkout or verified original VM synchronization",
    ),
    assessment: resource(Assessment, "Current review and gate disposition"),
    operation: resource(
        Operation,
        "Durable host operation, ownership and actual resource identities",
    ),
    review: resource(
        Review,
        "Structured review bound to the exact inspected evidence",
    ),
    test: resource(Test, "Gate exit result bound to the exact tested evidence"),
};
async function save(c: Context, name: string, value: Record<string, unknown>) {
    c.logger.info("Yinz workflow operation recorded");
    return {
        dataHandles: [await c.writeResource(name, name + "-current", value)],
    };
}
function method<T extends z.ZodType>(
    description: string,
    schema: T,
    name: string,
) {
    return {
        description,
        arguments: schema,
        execute: async (input: z.input<T>, c: Context) => {
            c.logger.info("Dispatching owned Yinz operation");
            const request = {
                ...schema.parse(input) as Record<string, unknown>,
                method: name,
            };
            if (name === "agent_spawn") {
                const spec = AgentSpec.parse(
                    (request as Record<string, unknown>).spec,
                );
                // A workflow's explicit command overrides its default agent kind.
                if (spec.command) delete spec.kind;
                (request as Record<string, unknown>).spec = spec;
            }
            return save(c, "operation", await operation(c, request));
        },
    };
}
async function reportPrompt(
    c: Context,
    introduction: string,
    test: z.infer<typeof Test>,
    maxBytes: number,
) {
    let ref = test.outputRef;
    if (ref) {
        if (!c.dataRepository) {
            throw new Error("Saved evidence repository unavailable");
        }
        test = await resolveEvidence(c.dataRepository, test);
        if (test.output === null) throw new Error("Missing saved test output");
    }
    const summary = test.summary ?? testReport(test).summary;
    const full = introduction + "\n\n" + summary;
    if (
        new TextEncoder().encode(full).length <= maxBytes &&
        PromptText.safeParse(full).success
    ) return full;
    if (!ref) {
        ref = await saveEvidence(c, "test", test);
    }
    return savedPrompt(c, introduction, ref, "failure report");
}
async function saveEvidence(
    c: Context,
    spec: "test" | "review",
    value: Record<string, unknown>,
) {
    if (!c.modelType || !c.modelId) {
        throw new Error("Saved evidence model identity unavailable");
    }
    return OutputRef.parse({
        modelType: c.modelType.normalized,
        modelId: c.modelId,
        ...await c.writeResource(spec, "repair-" + spec, value),
    });
}
function savedPrompt(
    c: Context,
    introduction: string,
    ref: z.infer<typeof OutputRef>,
    label: string,
) {
    const predicate = `modelType == ${
        JSON.stringify(ref.modelType)
    } && ownerType == "model-method" && ownerRef == ${
        JSON.stringify(ref.modelId)
    } && name == ${JSON.stringify(ref.name)} && version == ${ref.version}`;
    const quote = (s: string) => "'" + s.replaceAll("'", "'\"'\"'") + "'";
    return PromptText.parse(
        introduction + "\n\nThe complete " + label +
            " exceeds the prompt limit. Read this exact saved version before fixing failures:\n" +
            "swamp data query " + quote(predicate) +
            " --select attributes --json --repo-dir " + quote(c.repoDir),
    );
}

const RoleSpec = AgentSpec.pick({
    kind: true,
    model: true,
    effort: true,
    command: true,
    args: true,
});
const FlowInputs = z.object({
    flowVersion: z.literal(3),
    launch: Key,
    session: Literal.min(1),
    binary: Literal.min(1),
    checkout: Literal.min(1),
    target: Literal.min(1),
    targetBranch: Literal.min(1),
    gate: z.array(Literal).min(1),
    kind: Literal,
    agentCommand: Literal,
    agentArgs: z.array(Literal),
    commitAgent: RoleSpec.optional(),
    reviewAgent: RoleSpec.optional(),
    reviewFixAgent: RoleSpec.optional(),
    rebaseAgent: RoleSpec.optional(),
    testFixAgent: RoleSpec.optional(),
    rebasePrompt: Literal.refine((s) => s.trim().length > 0).default(
        "Resolve only the current rebase conflicts in this checkout and stage each resolution. Do not run rebase --continue or --abort: the workflow owns continuation. Do not merge, remove resources, or contact remotes. Report the result and exit when done.",
    ),
    firecracker: Literal,
    firecrackerImage: Literal,
    sourcePane: z.number().int().min(-1),
    phase: z.enum([
        "prepare",
        "diff",
        "commit",
        "conflict",
        "resolve",
        "resolved",
        "review",
        "tests",
        "failed-tests",
        "test-return",
        "fix",
        "review-agent",
        "review-fix",
        "continue",
        "confirm",
        "cleanup",
    ]),
    testReturn: z.object({
        workflow: z.enum([
            "@hammz/yinz-agent-exit",
            "@hammz/yinz-review-integration",
        ]),
        phase: z.enum(["prepare", "conflict", "resolved", "review", "confirm"]),
    }).default({
        workflow: "@hammz/yinz-review-integration",
        phase: "review",
    }),
    state: z.object({
        vmSync: Sync.nullable(),
        evidence: Evidence.nullable(),
        review: Review.nullable(),
        test: Test.nullable(),
    }),
});
const ContinueFlow = Request.extend({
    workflow: z.enum([
        "@hammz/yinz-agent-exit",
        "@hammz/yinz-review-integration",
    ]),
    inputs: FlowInputs,
    invalidateChecks: z.boolean().default(false),
});
export const methods = {
    commit_message_result: {
        description:
            "Validate and retain the commit agent's complete message before Git mutation",
        arguments: CommitMessageArgs,
        execute: (input: z.infer<typeof CommitMessageArgs>, c: Context) => {
            const a = CommitMessageArgs.parse(input);
            let message: string | null = null, problem: string | null = null;
            try {
                if (
                    a.operation.outcome !== "process-exit" ||
                    a.operation.exitCode !== 0 ||
                    a.operation.hook !== "Stop" ||
                    a.operation.owner?.agent !== a.agent
                ) {
                    throw new Error(
                        "Commit message agent did not complete successfully; inspect its saved operation",
                    );
                }
                message = z.object({
                    message: Literal.refine((s) => s.trim().length > 0),
                })
                    .parse(JSON.parse(a.operation.message ?? "")).message;
            } catch (error) {
                problem = String(error);
            }
            return save(
                c,
                "commitMessage",
                CommitMessage.parse({
                    message,
                    valid: problem === null,
                    problem,
                    evidence: a.evidence,
                    owner: a.operation.owner ?? null,
                }),
            );
        },
    },
    prepare_prompt: {
        description:
            "Prepare and validate a helper prompt before spawning its agent",
        arguments: z.object({
            prompt: z.string(),
            test: Test.nullable().optional(),
            review: Review.nullable().optional(),
            startup: z.boolean().default(false),
        }),
        execute: async (
            input: {
                prompt: string;
                test?: z.infer<typeof Test> | null;
                review?: z.infer<typeof Review> | null;
                startup?: boolean;
            },
            c: Context,
        ) => {
            const maxBytes = input.startup ? STARTUP_PROMPT_BYTES : 262144;
            let prompt = input.test
                ? await reportPrompt(
                    c,
                    input.prompt,
                    Test.parse(input.test),
                    maxBytes,
                )
                : input.prompt;
            if (input.review) {
                const full = prompt + "\n\nSaved review findings:\n" +
                    JSON.stringify(Review.parse(input.review));
                prompt = new TextEncoder().encode(full).length <= maxBytes
                    ? full
                    : savedPrompt(
                        c,
                        prompt,
                        await saveEvidence(c, "review", input.review),
                        "review findings",
                    );
            }
            return save(c, "prompt", {
                prompt: preparePrompt(prompt, maxBytes),
            });
        },
    },
    check_status: {
        description:
            "Classify saved checks against the current checkout evidence",
        arguments: CheckArgs,
        execute: (input: z.infer<typeof CheckArgs>, c: Context) => {
            const a = CheckArgs.parse(input);
            const status = (check: typeof a.review | typeof a.test) => {
                if (!check) return "missing";
                if (
                    check.outcome === "stale" ||
                    JSON.stringify(check.evidence) !==
                        JSON.stringify(a.evidence)
                ) return "stale";
                if (["cancelled", "interrupted"].includes(check.outcome)) {
                    return "cancelled";
                }
                if (
                    "findings" in check
                        ? !check.valid ||
                            !["completed", "process-exit"].includes(
                                check.outcome,
                            )
                        : check.outputComplete === false ||
                            !!check.captureError ||
                            check.exitCode === null ||
                            !["completed", "failed"].includes(check.outcome)
                ) return "incomplete";
                return ("findings" in check
                        ? check.valid && check.resolved &&
                            check.findings.every((f) =>
                                f.severity === "info"
                            ) &&
                            ["completed", "process-exit"].includes(
                                check.outcome,
                            )
                        : check.passed && check.outcome === "completed" &&
                            check.exitCode === 0)
                    ? "current"
                    : "failed";
            };
            const reason = mergeCheckProblem(a.evidence, a.review, a.test);
            const codeReview = status(a.review);
            const tests = status(a.test);
            const source = a.evidence.source;
            const target = a.evidence.target;
            const repairReview = codeReview === "failed" && !!a.review?.valid &&
                a.review.findings.some((f) => f.severity !== "info");
            const repairTests = tests === "failed" &&
                a.test?.exitCode != null &&
                a.test.exitCode !== 0;
            const recommendation = target.rebasing || !target.clean ||
                    target.conflicts.length
                ? "Finish changes in the target checkout, then return to this menu."
                : source.rebasing
                ? (source.conflicts.length
                    ? "Resolve and stage conflicts, then choose Continue rebase."
                    : "Choose Continue rebase to finish the paused rebase.")
                : repairReview
                ? "Fix review findings, then commit and rerun checks."
                : repairTests
                ? "Fix test failures, then commit and rerun checks."
                : !source.clean
                ? "Commit & rebase before final checks. Review is also available now."
                : codeReview === "incomplete"
                ? "Reviewer did not complete a valid review. Inspect its output and retry Review."
                : codeReview !== "current"
                ? "Run Review for the current source and target."
                : tests === "incomplete"
                ? "Test execution or output is incomplete. Inspect its output and retry tests."
                : tests !== "current"
                ? "Run tests for the current source and target."
                : "Merge after confirming the exact source and target revisions.";
            return save(c, "checks", {
                ...a,
                tests,
                codeReview,
                ready: reason === null,
                reason,
                recommendation,
                repairReview,
                repairTests,
            });
        },
    },
    continue_flow: {
        description:
            "Start a fresh workflow decision with a new run identity and no inherited answer",
        arguments: ContinueFlow,
        execute: async (input: z.infer<typeof ContinueFlow>, c: Context) => {
            const a = ContinueFlow.parse(input);
            if (a.invalidateChecks) {
                for (
                    const check of [a.inputs.state.review, a.inputs.state.test]
                ) {
                    if (check) check.outcome = "stale";
                }
            }
            const test = a.inputs.state.test;
            const continuationName = "continuation-" +
                await digest({ work: a.work, key: a.key });
            const fingerprint = await digest(a);
            const previous = await c.readResource(continuationName);
            if (previous && previous.fingerprint !== fingerprint) {
                throw new Error("Conflicting continuation request");
            }
            if (test && previous) {
                test.outputRef = OutputRef.parse(previous.outputRef);
            }
            if (
                test &&
                (test.outputRef || test.summary !== undefined ||
                    new TextEncoder().encode(test.output).length > 65536)
            ) {
                if (!c.dataRepository) {
                    throw new Error("Saved evidence repository unavailable");
                }
                if (test.outputRef) {
                    const saved = await readEvidence(
                        c.dataRepository,
                        test.outputRef,
                    );
                    if (!saved || typeof saved.output !== "string") {
                        throw new Error("Missing saved test output");
                    }
                    test.output = saved.output;
                } else {
                    const fingerprint = await digest(Test.parse(test));
                    for (const item of await c.dataRepository.findAllGlobal()) {
                        if (item.data.tags.specName !== "test") continue;
                        const ref = {
                            modelType: item.modelType.normalized,
                            modelId: item.modelId,
                            name: item.data.name,
                            version: item.data.version,
                        };
                        const saved = Test.safeParse(
                            await readEvidence(c.dataRepository, ref),
                        );
                        if (
                            saved.success &&
                            await digest(saved.data) === fingerprint
                        ) {
                            test.outputRef = ref;
                            break;
                        }
                    }
                    if (!test.outputRef) {
                        if (!c.modelType || !c.modelId) {
                            throw new Error(
                                "Saved evidence model identity unavailable",
                            );
                        }
                        const handle = await c.writeResource(
                            "test",
                            "test-" + fingerprint,
                            test,
                        );
                        test.outputRef = OutputRef.parse({
                            modelType: c.modelType.normalized,
                            modelId: c.modelId,
                            ...handle,
                        });
                    }
                }
                if (!previous) {
                    await c.writeResource("continuation", continuationName, {
                        fingerprint,
                        outputRef: test.outputRef,
                    });
                }
                if (test.output.length > 16384) {
                    test.output = test.output.slice(0, 8192).replace(
                        /[\uD800-\uDBFF]$/,
                        "",
                    ) +
                        "\n[Full output saved in Swamp; see outputRef]\n" +
                        test.output.slice(-8192).replace(
                            /^[\uDC00-\uDFFF]/,
                            "",
                        );
                }
                // The authoritative summary is resolved from outputRef by readers.
                delete test.summary;
                delete test.failedTests;
            }
            const host = {
                ...c,
                globalArgs: {
                    session: a.inputs.session,
                    binary: a.inputs.binary,
                },
            };
            const dispatched = await operation(host, {
                method: "swamp_dispatch",
                operation: "workflow.run",
                work: a.work,
                key: a.key,
                directory: c.repoDir,
                payload: {
                    workflowIdOrName: a.workflow,
                    inputs: a.inputs,
                    noSupersede: true,
                },
            });
            await save(c, "operation", dispatched);
            const result = await observe(
                host,
                { work: a.work, key: a.key },
                3600,
            );
            // Successful child output contains complete child reports. Those already
            // live in Swamp; nesting them in every parent report grows without bound.
            if (result.outcome === "completed" && result.exitCode === 0) {
                delete result.output;
                delete result.outputTruncated;
            }
            const saved = await save(c, "operation", result);
            if (result.outcome !== "completed" || result.exitCode !== 0) {
                throw new Error(
                    "Continuation failed; inspect operation-current (" +
                        result.outcome + ", exit " + result.exitCode + ")",
                );
            }
            return saved;
        },
    },
    pane_action: method(
        "Act on the original exited pane; fork cancellation returns a saved result",
        Request.extend({
            action: z.enum(["restart", "duplicate", "fork", "leave"]),
        }),
        "pane_action",
    ),
    step_ready: method(
        "Refuse another step until earlier owned commands have finished or been reconciled",
        Request,
        "step_ready",
    ),
    checkout_sync: {
        description:
            "Sync and verify the original VM, or record explicit local checkout evidence",
        arguments: Request.extend({ previous: Sync.nullable().optional() }),
        execute: async (
            input: z.infer<typeof Request> & {
                previous?: z.infer<typeof Sync> | null;
            },
            c: Context,
        ) => {
            const result = await operation(c, {
                ...Request.parse(input),
                method: "source_sync",
            });
            await save(c, "operation", result);
            if (result.outcome !== "completed" || !result.clean) {
                throw new Error(
                    "Original VM has unresolved changes or sync conflicts",
                );
            }
            const previous = input.previous;
            // A fresh sync must not stale identical VM contents merely by changing its operation key.
            const sameVm = previous && !("mode" in previous) &&
                result.vm === previous.vm &&
                JSON.stringify(result.owner) === JSON.stringify(previous.owner);
            return save(
                c,
                "checkoutSync",
                result.vm
                    ? (sameVm ? previous : Sync.parse(result))
                    : { mode: "local" },
            );
        },
    },
    workflow_attach: {
        description:
            "Record the actual Swamp run identity against its durable launch token",
        arguments: Request.extend({
            run: z.string().uuid(),
            workflow: Literal.min(1),
        }),
        execute: async (
            input: z.infer<typeof Request> & { run: string; workflow: string },
            c: Context,
        ) => {
            const args = Request.extend({
                run: z.string().uuid(),
                workflow: Literal.min(1),
            }).parse(input);
            return save(
                c,
                "operation",
                await operation(c, {
                    ...args,
                    method: "workflow_attach",
                    directory: c.repoDir,
                }),
            );
        },
    },
    assess: {
        description:
            "Combine structured findings and gate evidence without inventing missing results",
        arguments: AssessmentArgs,
        execute: (input: z.infer<typeof AssessmentArgs>, c: Context) => {
            const a = AssessmentArgs.parse(input);
            return save(
                c,
                "assessment",
                Assessment.parse({
                    ...a,
                    ready:
                        mergeCheckProblem(a.current, a.review, a.test) === null,
                }),
            );
        },
    },
    repair_prompt: {
        description:
            "Send the workflow-defined repair instructions with concrete findings and gate logs",
        arguments: Agent.extend({ assessment: Assessment }),
        execute: async (
            input: z.infer<typeof Agent> & {
                assessment: z.infer<typeof Assessment>;
            },
            c: Context,
        ) => {
            const a = Agent.extend({ assessment: Assessment }).parse(input);
            if (a.assessment.test?.outputRef) {
                if (!c.dataRepository) {
                    throw new Error("Saved evidence repository unavailable");
                }
                a.assessment = await resolveEvidence(
                    c.dataRepository,
                    a.assessment,
                );
                if (a.assessment.test?.output === null) {
                    throw new Error("Missing saved test output");
                }
            }
            return save(
                c,
                "operation",
                await operation(c, {
                    method: "agent_prompt",
                    work: a.work,
                    key: a.key,
                    agent: a.agent,
                    prompt: preparePrompt(
                        "Resolve the following review findings and test failures in your checkout. Resolve rebase conflicts if present, but do not merge, remove resources, or contact a remote. Leave changes for the workflow to commit. Report completion explicitly.\n" +
                            JSON.stringify(a.assessment),
                    ),
                }),
            );
        },
    },
    review_prompt: {
        description:
            "Ask a separate reviewer to inspect the exact source and target commits",
        arguments: Agent.extend({ evidence: Evidence }),
        execute: async (
            input: z.infer<typeof Agent> & {
                evidence: z.infer<typeof Evidence>;
            },
            c: Context,
        ) => {
            const a = Agent.extend({ evidence: Evidence }).parse(input);
            return save(
                c,
                "operation",
                await operation(c, {
                    method: "agent_prompt",
                    work: a.work,
                    key: a.key,
                    agent: a.agent,
                    prompt: preparePrompt(
                        'Review the checkout and source commit against the target commit described below. Do not edit files, commit, merge, remove resources or contact remotes. Include unresolved rebase conflicts. Your final response must contain only JSON: {"findings":[{"severity":"error|warning|info","file":"path","message":"finding"}]}. An empty findings array means you completed a clean review.\n' +
                            JSON.stringify(a.evidence),
                    ),
                }),
            );
        },
    },
    agent_spawn: method(
        "Spawn an owned agent without sending prompt bytes",
        Spawn,
        "agent_spawn",
    ),
    agent_inspect: method(
        "Inspect an owned agent after verifying its supervisor and process",
        Agent,
        "agent_inspect",
    ),
    agent_prompt: method(
        "Register completion before dispatching one prompt",
        Prompt,
        "agent_prompt",
    ),
    command: {
        description:
            "Run literal argv in a supervised pane, optionally capturing complete output",
        arguments: Command,
        execute: async (input: z.input<typeof Command>, c: Context) => {
            const args = Command.parse(input);
            let capture;
            if (args.captureOutput) {
                const name = "capture-" + await digest(args);
                const previous = await c.readResource(name);
                capture = previous ? Capture.parse(previous) : {
                    directory: await Deno.makeTempDir({
                        dir: "/tmp",
                        prefix: "yinz-test-",
                    }),
                };
                if (!previous) await c.writeResource("capture", name, capture);
            }
            return save(
                c,
                "operation",
                await operation(c, {
                    ...args,
                    method: "command",
                    ...(capture
                        ? {
                            capture,
                            argv: captureCommand(
                                capture.directory,
                                args.argv,
                                args.foreground,
                                {
                                    binary: args.binary ??
                                        String(c.globalArgs.binary ?? "yinz"),
                                    session: args.session ??
                                        String(c.globalArgs.session),
                                    work: args.work,
                                    key: args.key,
                                },
                            ),
                        }
                        : {}),
                }),
            );
        },
    },
    vm_inspect: method(
        "Inspect guest changes through the existing provider",
        Agent,
        "vm_inspect",
    ),
    vm_sync: method(
        "Sync an owned VM and report unresolved changes",
        Agent,
        "vm_sync",
    ),
    cancel_step: method(
        "Interrupt one owned operation and verify termination before another step",
        Request.extend({ operation: Key }),
        "cancel_step",
    ),
    reconcile: method(
        "Reconcile a saved supervisor without repeating uncertain mutations",
        Request.extend({ operation: Key }),
        "reconcile",
    ),
    observe: {
        description:
            "Observe explicit hook or process completion; idle is never success",
        arguments: Observe,
        execute: async (input: z.infer<typeof Observe>, c: Context) => {
            const a = Observe.parse(input);
            c.logger.info("Observing existing Yinz operation");
            return save(c, "operation", await observe(c, a, a.timeoutSeconds));
        },
    },
    review_result: {
        description:
            "Require separate, structured review findings and bind them to inspected contents",
        arguments: ReviewArgs,
        execute: (input: z.infer<typeof ReviewArgs>, c: Context) => {
            const a = ReviewArgs.parse(input);
            c.logger.info("Validating structured review findings");
            let findings: z.infer<typeof Finding>[] = [];
            let problem: string | null = null;
            if (
                (a.operation.outcome !== "completed" &&
                    !(a.operation.outcome === "process-exit" &&
                        a.operation.hook === "Stop" &&
                        a.operation.exitCode === 0)) ||
                !a.operation.owner ||
                a.operation.owner.agent === a.worker
            ) problem = "A separate review agent must explicitly complete";
            else {
                try {
                    findings = z.object({ findings: z.array(Finding) }).parse(
                        JSON.parse(a.operation.message ?? ""),
                    ).findings;
                } catch {
                    problem = "Missing or malformed review findings";
                }
            }
            return save(
                c,
                "review",
                Review.parse({
                    evidence: a.evidence,
                    outcome: a.operation.outcome,
                    findings,
                    reviewer: a.operation.owner?.agent ?? "",
                    valid: problem === null,
                    resolved: problem === null &&
                        findings.every((f) => f.severity === "info"),
                    problem,
                }),
            );
        },
    },
    test_result: {
        description:
            "Bind actual command completion and exit status to the tested revision",
        arguments: TestArgs,
        execute: async (input: z.infer<typeof TestArgs>, c: Context) => {
            const a = TestArgs.parse(input);
            c.logger.info("Recording gate evidence");
            if (!a.operation.log) {
                throw new Error("Command result has no log identity");
            }
            const operationFingerprint = await digest(a);
            const previous = await c.readResource("test-current");
            const cleanup = async (outputComplete: unknown) => {
                if (!a.operation.capture) return;
                try {
                    await bridge({
                        ...c,
                        globalArgs: {
                            ...c.globalArgs,
                            session: a.operation.owner?.session ??
                                c.globalArgs.session,
                        },
                    }, {
                        method: "command_close",
                        work: a.operation.work,
                        key: a.operation.key,
                    });
                    if (outputComplete) {
                        try {
                            await Deno.remove(a.operation.capture.directory, {
                                recursive: true,
                            });
                        } catch (error) {
                            if (!(error instanceof Deno.errors.NotFound)) {
                                throw error;
                            }
                        }
                    }
                } catch (error) {
                    throw new Error(
                        "Evidence saved; capture cleanup failed: " + error,
                    );
                }
            };
            // A retry after cleanup must use the already-saved evidence.
            if (previous?.operationFingerprint === operationFingerprint) {
                const result = await save(c, "test", previous);
                await cleanup(previous.outputComplete);
                return result;
            }
            const captured = a.operation.capture
                ? await readCapture(
                    a.operation.capture.directory,
                    a.operation.exitCode ?? null,
                )
                : {
                    output: a.operation.output ?? "",
                    outputComplete: a.operation.output !== undefined &&
                        !a.operation.outputTruncated && !a.operation.logError,
                    captureError: a.operation.logError ??
                        (a.operation.output === undefined
                            ? "Output unavailable"
                            : a.operation.outputTruncated
                            ? "Only bounded terminal scrollback was captured"
                            : null),
                };
            const test = {
                evidence: a.evidence,
                outcome: a.operation.outcome,
                exitCode: a.operation.exitCode ?? null,
                log: a.operation.log,
                operationFingerprint,
                ...captured,
            };
            const result = await save(
                c,
                "test",
                Test.parse({ ...test, ...testReport(test) }),
            );
            await cleanup(captured.outputComplete);
            return result;
        },
    },
};
