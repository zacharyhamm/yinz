/** Host operations and evidence adapters for the existing Zellij model. @module */
import { z } from "npm:zod@4.3.6";
import {
    AgentSpec,
    observe,
    Operation,
    operation,
    preparePrompt,
    PromptText,
    Request,
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
import { Evidence, Finding, Sync } from "../git_workflow.ts";
const Agent = Request.extend({ agent: Key });
const Spawn = Request.extend({ spec: AgentSpec });
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
const Checks = z.object({
    evidence: Evidence,
    review: Review.nullable(),
    test: Test.nullable(),
    tests: z.enum(["skipped", "passed", "failed"]),
    codeReview: z.enum(["skipped", "passed", "failed"]),
});
const CheckArgs = Checks.omit({ tests: true, codeReview: true });
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
        "Exact revision and optional check results presented for informed merge approval",
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
    c.logger.info("Zellij workflow operation recorded");
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
            c.logger.info("Dispatching owned Zellij operation");
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
) {
    const ref = test.outputRef;
    if (ref) {
        if (!c.dataRepository) {
            throw new Error("Saved evidence repository unavailable");
        }
        test = await resolveEvidence(c.dataRepository, test);
        if (test.output === null) throw new Error("Missing saved test output");
    }
    const summary = test.summary ?? testReport(test).summary;
    const full = introduction + "\n\n" + summary;
    if (PromptText.safeParse(full).success) return full;
    if (!ref) {
        throw new Error(
            "Oversized test report has no saved evidence reference",
        );
    }
    const predicate = `modelType == ${
        JSON.stringify(ref.modelType)
    } && ownerType == "model-method" && ownerRef == ${
        JSON.stringify(ref.modelId)
    } && name == ${JSON.stringify(ref.name)} && version == ${ref.version}`;
    const quote = (s: string) => "'" + s.replaceAll("'", "'\"'\"'") + "'";
    return PromptText.parse(
        introduction + "\n\nTested revision: " + test.evidence.source.commit +
            "\nCompletion: " + test.outcome + "; exit status: " +
            test.exitCode +
            "\nThe complete report exceeds the prompt limit. Read this exact saved version before fixing failures (summary and raw output are included):\n" +
            "swamp data query " + quote(predicate) +
            " --select attributes --json --repo-dir " + quote(c.repoDir),
    );
}
const FlowInputs = z.object({
    flowVersion: z.literal(2),
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
        "fix",
        "review-agent",
        "confirm",
        "cleanup",
    ]),
    state: z.object({
        vmSync: Sync.nullable(),
        evidence: Evidence.nullable(),
        review: Review.nullable(),
        test: Test.nullable(),
    }),
});
const ContinueFlow = Request.extend({
    workflow: z.enum([
        "@hammz/zellij-agent-exit",
        "@hammz/zellij-review-integration",
    ]),
    inputs: FlowInputs,
});
export const methods = {
    prepare_prompt: {
        description:
            "Prepare and validate a helper prompt before spawning its agent",
        arguments: z.object({
            prompt: z.string(),
            test: Test.nullable().optional(),
        }),
        execute: async (
            input: { prompt: string; test?: z.infer<typeof Test> | null },
            c: Context,
        ) => save(c, "prompt", {
            prompt: input.test
                ? await reportPrompt(c, input.prompt, Test.parse(input.test))
                : preparePrompt(input.prompt),
        }),
    },
    check_status: {
        description:
            "Save the exact optional check results shown at merge confirmation",
        arguments: CheckArgs,
        execute: (input: z.infer<typeof CheckArgs>, c: Context) => {
            const a = CheckArgs.parse(input);
            return save(c, "checks", {
                ...a,
                tests: a.test === null
                    ? "skipped"
                    : a.test.passed
                    ? "passed"
                    : "failed",
                codeReview: a.review === null
                    ? "skipped"
                    : a.review.resolved
                    ? "passed"
                    : "failed",
            });
        },
    },
    continue_flow: {
        description:
            "Start a fresh workflow decision with a new run identity and no inherited answer",
        arguments: ContinueFlow,
        execute: async (input: z.infer<typeof ContinueFlow>, c: Context) => {
            const a = ContinueFlow.parse(input);
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
    checkout_sync: {
        description:
            "Sync and verify the original VM, or record explicit local checkout evidence",
        arguments: Request,
        execute: async (input: z.infer<typeof Request>, c: Context) => {
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
            return save(
                c,
                "checkoutSync",
                result.vm ? Sync.parse(result) : { mode: "local" },
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
                    ready: a.current.source.clean && a.current.target.clean &&
                        !a.current.source.rebasing &&
                        !a.current.target.rebasing &&
                        a.current.source.conflicts.length === 0 &&
                        a.current.target.conflicts.length === 0 &&
                        a.review.resolved && a.test !== null &&
                        a.test.passed &&
                        JSON.stringify(a.review.evidence) ===
                            JSON.stringify(a.test.evidence) &&
                        JSON.stringify(a.current) ===
                            JSON.stringify(a.review.evidence),
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
                        prefix: "zellij-test-",
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
                            argv: captureCommand(capture.directory, args.argv),
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
            c.logger.info("Observing existing Zellij operation");
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
                        a.operation.hook === "Stop")) ||
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
            // A retry after cleanup must use the already-saved evidence.
            if (previous?.operationFingerprint === operationFingerprint) {
                return save(c, "test", previous);
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
            if (a.operation.capture && captured.outputComplete) {
                try {
                    await Deno.remove(a.operation.capture.directory, {
                        recursive: true,
                    });
                } catch (error) {
                    c.logger.info(
                        "Evidence saved; capture cleanup failed: " + error,
                    );
                }
            }
            return result;
        },
    },
};
