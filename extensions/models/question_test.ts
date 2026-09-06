import { model } from "./question.ts";
import { Context } from "./_lib/workflow.ts";
function assert(v: unknown, message = "assertion failed"): asserts v {
    if (!v) throw new Error(message);
}
async function rejects(f: () => Promise<unknown>, pattern: RegExp) {
    try {
        await f();
    } catch (e) {
        assert(pattern.test(String(e)), String(e));
        return;
    }
    throw new Error("Expected rejection");
}
Deno.test("all question kinds stay pending unattended and accept one run-bound answer, including interrupted delivery", async () => {
    const original = Deno.Command;
    const run = crypto.randomUUID(), other = crypto.randomUUID();
    const saved = new Map<string, Record<string, unknown>>();
    const claims = new Map<string, Record<string, unknown>>();
    let history = "suspended",
        step = "waiting_approval",
        crash = "",
        approvals = 0,
        dispatches = 0;
    const encoder = new TextEncoder();
    const c: Context = {
        repoDir: "/tmp",
        globalArgs: { session: "test", binary: "zellij", timeoutMs: 1000 },
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: (n) =>
            Promise.resolve(structuredClone(saved.get(n) ?? null)),
        writeResource: (_spec, n, v) => {
            if (crash === "after-dispatch" && v.state === "submitted") {
                crash = "";
                throw new Error("interrupted after dispatch");
            }
            saved.set(
                n,
                structuredClone(model.resources.question.schema.parse(v)),
            );
            return Promise.resolve({ name: n });
        },
    };
    class Command {
        args: string[];
        constructor(_path: string, opts: Deno.CommandOptions) {
            this.args = opts.args ?? [];
        }
        spawn() {
            let input = "";
            let done!: () => void;
            const closed = new Promise<void>((resolve) => done = resolve);
            return {
                stdin: new WritableStream<Uint8Array>({
                    write: (bytes) => {
                        input += new TextDecoder().decode(bytes);
                    },
                    close: () => {
                        done();
                    },
                }),
                output: async () => {
                    await closed;
                    this.args.push(input);
                    return this.output();
                },
            };
        }
        output() {
            let out: unknown = {};
            if (this.args[0] === "workflow") {
                if (this.args[1] === "history") {
                    out = {
                        id: run,
                        workflowName: "example",
                        status: history,
                        jobs: [{
                            name: "main",
                            steps: [{ name: "answer", status: step }],
                        }],
                    };
                } else if (this.args[1] === "approve") {
                    approvals++;
                    step = "succeeded";
                    if (crash === "after-approve") {
                        crash = "";
                        throw new Error("interrupted after approval");
                    }
                } else throw new Error("unexpected CLI");
            } else {
                const req = JSON.parse(this.args.at(-1)!);
                if (req.method === "lookup") {
                    out = { result: claims.get(req.key) ?? null };
                } else if (req.method === "command") {
                    if (!claims.has(req.key)) {
                        dispatches++;
                        claims.set(req.key, {
                            work: req.work,
                            key: req.key,
                            request: JSON.stringify(req),
                            incarnation: "test",
                            outcome: "pending",
                        });
                    }
                    out = { result: claims.get(req.key) };
                } else throw new Error("unexpected bridge method");
            }
            return Promise.resolve({
                code: 0,
                success: true,
                signal: null,
                stdout: encoder.encode(JSON.stringify(out)),
                stderr: new Uint8Array(),
            });
        }
    }
    Deno.Command = Command as unknown as typeof Deno.Command;
    try {
        for (
            const kind of [
                "choice",
                "text",
                "approval",
                "instructions",
                "conflict",
                "merge",
                "cleanup",
            ] as const
        ) {
            const args = {
                run,
                workflow: "example",
                question: kind,
                step: "answer",
                kind,
                prompt: "Choose",
                answers: ["yes", "no"],
                resumeInput: "answer",
            };
            await model.methods.open.execute(args, c);
            await model.methods.open.execute(args, c);
            assert(saved.get(kind)?.state === "pending");
            assert(
                approvals === 0 && dispatches === 0,
                "Unattended questions must not approve themselves",
            );
        }
        const input = {
            run,
            question: "merge",
            answer: "yes",
            submission: "one",
        };
        await rejects(
            () => model.methods.submit.execute({ ...input, run: other }, c),
            /wrong/,
        );
        await rejects(
            () =>
                model.methods.submit.execute(
                    { ...input, answer: "invalid" },
                    c,
                ),
            /permitted/,
        );
        history = "succeeded";
        await rejects(() => model.methods.submit.execute(input, c), /Stale/);
        history = "suspended";
        crash = "after-approve";
        await rejects(
            () => model.methods.submit.execute(input, c),
            /interrupted/,
        );
        assert(saved.get("merge")?.state === "recorded");
        await rejects(
            () => model.methods.submit.execute(input, c),
            /Duplicate/,
        );
        crash = "after-dispatch";
        await rejects(
            () => model.methods.recover.execute({ run, question: "merge" }, c),
            /interrupted/,
        );
        history = "running";
        await model.methods.recover.execute({ run, question: "merge" }, c);
        assert(
            Number(approvals) === 1 && Number(dispatches) === 1,
            "Recovery must not approve or dispatch twice",
        );
        await model.methods.consume.execute(input, c);
        await model.methods.recover.execute({ run, question: "merge" }, c);
        assert(saved.get("merge")?.state === "consumed");
        await rejects(
            () =>
                model.methods.consume.execute(
                    { ...input, submission: "wrong" },
                    c,
                ),
            /matching/,
        );
        await rejects(
            () => model.methods.recover.execute({ run, question: "text" }, c),
            /No recorded/,
        );
        await rejects(
            () =>
                model.methods.open.execute({
                    run,
                    workflow: "example",
                    question: "bad",
                    step: "answer",
                    kind: "choice",
                    prompt: "Choose",
                    answers: [],
                    resumeInput: "answer",
                }, c),
            /explicit/,
        );
    } finally {
        Deno.Command = original;
    }
});
