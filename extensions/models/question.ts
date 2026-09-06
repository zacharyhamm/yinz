/** Questions are versioned Swamp resources, never guessed by an unattended runner. @module */
import { z } from "npm:zod@4.3.6";
import { checked, Context, Key, Literal } from "./_lib/workflow.ts";
import { bridge, Operation, operation, Session } from "./_lib/bridge.ts";
const Open = z.object({
    run: z.string().uuid(),
    workflow: Literal.min(1),
    question: Key,
    step: Literal.min(1),
    kind: z.enum([
        "choice",
        "text",
        "approval",
        "instructions",
        "conflict",
        "merge",
        "cleanup",
    ]),
    prompt: z.string().min(1),
    answers: z.array(z.string().min(1)),
    resumeInput: Key,
});
const Question = Open.extend({
    state: z.enum(["pending", "recorded", "submitted", "consumed"]),
    answer: z.string().nullable(),
    submission: z.string().nullable(),
});
const Submission = z.object({
    run: z.string().uuid(),
    question: Key,
    answer: z.string().min(1),
    submission: Key,
});
const Lookup = Submission.pick({ run: true, question: true });
const History = z.object({
    id: z.string(),
    workflowName: z.string(),
    status: z.string(),
    jobs: z.array(
        z.object({
            name: z.string(),
            steps: z.array(z.object({ name: z.string(), status: z.string() })),
        }),
    ),
});
async function get(c: Context, a: z.infer<typeof Lookup>) {
    const q = Question.parse(await c.readResource(a.question));
    if (q.run !== a.run) throw new Error("Answer names the wrong workflow run");
    return q;
}
async function save(c: Context, q: z.infer<typeof Question>) {
    return {
        dataHandles: [
            await c.writeResource("question", q.question, Question.parse(q)),
        ],
    };
}
async function pending(
    c: Context,
    q: z.infer<typeof Question>,
    recovering = false,
) {
    const run = History.parse(
        JSON.parse(
            await checked(c, c.repoDir, [
                "swamp",
                "workflow",
                "history",
                "get",
                q.run,
                "--json",
            ]),
        ),
    );
    if (
        run.id !== q.run || run.workflowName !== q.workflow ||
        run.status !== "suspended"
    ) throw new Error("Stale answer: run is not suspended");
    const steps = run.jobs.flatMap((job) =>
        job.steps.filter((s) => s.name === q.step)
    );
    if (
        steps.length !== 1 ||
        !(steps[0].status === "waiting_approval" ||
            (recovering && steps[0].status === "succeeded"))
    ) throw new Error("Question no longer names the pending approval");
    return steps[0].status;
}
async function deliver(c: Context, q: z.infer<typeof Question>) {
    const existing = await bridge(c, {
        method: "lookup",
        work: q.run,
        key: `answer-${q.submission}`,
    });
    if (existing === null) {
        const status = await pending(c, q, true);
        if (status === "waiting_approval") {
            await checked(c, c.repoDir, [
                "swamp",
                "workflow",
                "approve",
                q.workflow,
                q.step,
                "--run",
                q.run,
            ]);
        }
    } else {
        const prior = Operation.parse(existing);
        if (
            prior.outcome === "unknown" || prior.outcome === "failed" ||
            prior.outcome === "interrupted" || prior.outcome === "process-exit"
        ) {
            throw new Error(
                "Recorded resume requires reconciliation; no second command was dispatched",
            );
        }
    }
    // The supervised process waits for this model's lock to release before consuming the answer.
    // Dispatch has its own durable key; recovery asks for the same operation rather than relaunching.
    const result = await operation(c, {
        method: "command",
        work: q.run,
        key: `answer-${q.submission}`,
        cwd: c.repoDir,
        argv: [
            "swamp",
            "workflow",
            "resume",
            q.workflow,
            "--run",
            q.run,
            "--input",
            JSON.stringify({
                [q.resumeInput]: q.answer,
                submission: q.submission,
            }),
            "--json",
        ],
    });
    if (result.outcome === "unknown") {
        throw new Error(
            "Answer recorded; resume dispatch unknown. Reconcile the command before recovery",
        );
    }
    q.state = "submitted";
    c.logger.info("Recorded answer delivered to the workflow");
    return save(c, q);
}
export const model = {
    type: "@hammz/question",
    version: "2026.09.06.1",
    globalArguments: Session,
    resources: {
        question: {
            description: "Run-bound question and durable submission state",
            schema: Question,
            lifetime: "infinite",
            garbageCollection: 50,
        },
    },
    methods: {
        open: {
            description: "Declare a question before manual approval",
            arguments: Open,
            execute: async (input: z.infer<typeof Open>, c: Context) => {
                const a = Open.parse(input);
                c.logger.info("Opening workflow question");
                if (
                    !["text", "instructions"].includes(a.kind) &&
                    a.answers.length < 2
                ) {
                    throw new Error(
                        "Choice questions require explicit permitted answers",
                    );
                }
                const previous = await c.readResource(a.question);
                if (previous) {
                    const q = Question.parse(previous);
                    if (JSON.stringify(Open.parse(q)) !== JSON.stringify(a)) {
                        throw new Error(
                            "Question identity already exists with different arguments",
                        );
                    }
                    return save(c, q);
                }
                return save(
                    c,
                    Question.parse({
                        ...a,
                        state: "pending",
                        answer: null,
                        submission: null,
                    }),
                );
            },
        },
        submit: {
            description:
                "Validate and record one answer before approval and resume",
            arguments: Submission,
            execute: async (input: z.infer<typeof Submission>, c: Context) => {
                const a = Submission.parse(input);
                c.logger.info("Validating workflow answer");
                const q = await get(c, a);
                if (q.state !== "pending") {
                    throw new Error(
                        "Duplicate answer; recover the recorded submission instead",
                    );
                }
                if (q.answers.length && !q.answers.includes(a.answer)) {
                    throw new Error("Answer is not permitted");
                }
                await pending(c, q);
                q.state = "recorded";
                q.answer = a.answer;
                q.submission = a.submission;
                await save(c, q);
                return deliver(c, q);
            },
        },
        recover: {
            description:
                "Finish delivery of an already recorded answer without accepting it twice",
            arguments: Lookup,
            execute: async (a: z.infer<typeof Lookup>, c: Context) => {
                c.logger.info("Recovering recorded workflow answer");
                const q = await get(c, Lookup.parse(a));
                if (q.state === "submitted" || q.state === "consumed") {
                    return save(c, q);
                }
                if (q.state !== "recorded") {
                    throw new Error("No recorded answer to recover");
                }
                return deliver(c, q);
            },
        },
        consume: {
            description:
                "Check that resume inputs match the recorded submission",
            arguments: Submission,
            execute: async (input: z.infer<typeof Submission>, c: Context) => {
                const a = Submission.parse(input);
                c.logger.info("Verifying resumed workflow answer");
                const q = await get(c, a);
                if (
                    q.state === "pending" || q.answer !== a.answer ||
                    q.submission !== a.submission
                ) {
                    throw new Error(
                        "Resume inputs lack a matching recorded answer",
                    );
                }
                q.state = "consumed";
                return save(c, q);
            },
        },
    },
};
