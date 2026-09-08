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
import { Context, digest, Key, Literal } from "./workflow.ts";
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
const Test = z.object({
    evidence: Evidence,
    outcome: z.string(),
    exitCode: z.number().int().nullable(),
    log: z.string(),
    output: z.string().default(""),
    passed: z.boolean(),
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
        arguments: z.object({ prompt: z.string() }),
        execute: (input: { prompt: string }, c: Context) =>
            save(c, "prompt", { prompt: preparePrompt(input.prompt) }),
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
            const directory = await Deno.realPath(c.repoDir) +
                "/.swamp/continuation-inputs";
            await Deno.mkdir(directory, { recursive: true });
            const inputFile = `${directory}/${await digest(a)}.json`;
            await Deno.writeTextFile(inputFile, JSON.stringify(a.inputs));
            const host = {
                ...c,
                globalArgs: {
                    session: a.inputs.session,
                    binary: a.inputs.binary,
                },
            };
            const dispatched = await operation(host, {
                method: "command",
                work: a.work,
                key: a.key,
                cwd: c.repoDir,
                argv: [
                    "swamp",
                    "workflow",
                    "run",
                    a.workflow,
                    "--no-supersede",
                    "--input-file",
                    inputFile,
                    "--json",
                ],
            });
            await save(c, "operation", dispatched);
            const result = await observe(
                host,
                { work: a.work, key: a.key },
                3600,
            );
            // Successful runner stdout contains complete child reports. Those already
            // live in Swamp; nesting them in every parent report grows without bound.
            if (result.outcome === "completed" && result.exitCode === 0) {
                delete result.output;
                delete result.outputTruncated;
            }
            const saved = await save(c, "operation", result);
            // A lost dispatch/observation reply must leave the child's inputs intact.
            if (
                result.outcome === "completed" ||
                (result.outcome === "failed" && result.exitCode != null)
            ) {
                await Deno.remove(inputFile);
            }
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
    command: method(
        "Run literal argv in an independently supervised command pane",
        Command,
        "command",
    ),
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
        execute: (input: z.infer<typeof TestArgs>, c: Context) => {
            const a = TestArgs.parse(input);
            c.logger.info("Recording gate evidence");
            if (!a.operation.log) {
                throw new Error("Command result has no log identity");
            }
            return save(
                c,
                "test",
                Test.parse({
                    evidence: a.evidence,
                    outcome: a.operation.outcome,
                    exitCode: a.operation.exitCode ?? null,
                    log: a.operation.log,
                    output: a.operation.output ?? a.operation.logError ??
                        "Output unavailable",
                    passed: a.operation.outcome === "completed" &&
                        a.operation.exitCode === 0,
                }),
            );
        },
    },
};
