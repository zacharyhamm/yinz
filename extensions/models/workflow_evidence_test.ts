import { methods, resources } from "./_lib/zellij_methods.ts";
import { Context } from "./_lib/workflow.ts";
function assert(v: unknown): asserts v {
    if (!v) throw new Error("assertion failed");
}
Deno.test("review and gate evidence cannot be inferred from missing findings, process exit, dirty files or changed targets", async () => {
    const saved = new Map<string, Record<string, unknown>>();
    const c: Context = {
        repoDir: "/tmp",
        globalArgs: {},
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: (n) => Promise.resolve(saved.get(n) ?? null),
        writeResource: (spec, name, value) => {
            resources[spec as keyof typeof resources].schema.parse(value);
            saved.set(name, value);
            return Promise.resolve({ name });
        },
    };
    const owner = {
        host_id: "host",
        session: "session",
        incarnation: "incarnation",
        work: "work",
        agent: "reviewer",
    };
    const op = {
        owner,
        key: "review",
        work: "work",
        incarnation: "incarnation",
        request: "{}",
        outcome: "completed" as const,
        message: '{"findings":[]}',
    };
    const revision = {
        checkout: "/tmp/source",
        branch: "topic",
        commit: "a".repeat(40),
        contents: "content",
        clean: true,
        conflicts: [],
        rebasing: false,
    };
    const evidence = {
        source: revision,
        target: { ...revision, checkout: "/tmp/main", branch: "main" },
        gate: ["true"],
        vmSync: { ...op, clean: true as const, vm: null },
    };
    for (const message of ["", "not JSON", '{"findings":null}']) {
        await methods.review_result.execute({
            operation: { ...op, message },
            evidence,
            worker: "worker",
        }, c);
        assert(saved.get("review-current")?.resolved === false);
    }
    await methods.review_result.execute({
        operation: op,
        evidence,
        worker: "reviewer",
    }, c);
    assert(saved.get("review-current")?.resolved === false);
    await methods.review_result.execute({
        operation: op,
        evidence,
        worker: "worker",
    }, c);
    const review = resources.review.schema.parse(saved.get("review-current"));
    for (const outcome of ["process-exit", "failed", "unknown"] as const) {
        await methods.test_result.execute({
            operation: { ...op, outcome, exitCode: 0, log: "terminal_7" },
            evidence,
        }, c);
        assert(saved.get("test-current")?.passed === false);
    }
    let rejected = false;
    try {
        await methods.test_result.execute({
            operation: { ...op, exitCode: 0 },
            evidence,
        }, c);
    } catch {
        rejected = true;
    }
    assert(rejected);
    await methods.test_result.execute({
        operation: { ...op, exitCode: 0, log: "terminal_7" },
        evidence,
    }, c);
    const test = resources.test.schema.parse(saved.get("test-current"));
    for (
        const current of [{ ...evidence, gate: ["false"] }, {
            ...evidence,
            target: { ...evidence.target, commit: "b".repeat(40) },
        }, { ...evidence, source: { ...revision, clean: false } }]
    ) {
        await methods.assess.execute({ review, test, current }, c);
        assert(saved.get("assessment-current")?.ready === false);
    }
    await methods.assess.execute({ review, test: null, current: evidence }, c);
    assert(saved.get("assessment-current")?.ready === false);
    await methods.assess.execute({ review, test, current: evidence }, c);
    assert(saved.get("assessment-current")?.ready === true);
    for (const method of ["repair_prompt", "review_prompt"] as const) {
        try {
            const args = method === "repair_prompt"
                ? {
                    work: "work",
                    key: "large",
                    agent: "agent",
                    assessment: {
                        review,
                        test: { ...test, log: "x".repeat(262144) },
                        ready: false,
                    },
                }
                : {
                    work: "work",
                    key: "large",
                    agent: "agent",
                    evidence: {
                        ...evidence,
                        gate: ["x".repeat(262144)],
                    },
                };
            // Invalid session config would fail if validation reached transport.
            await methods[method].execute(args as never, c);
            throw new Error("oversized prompt accepted");
        } catch (error) {
            assert(String(error).includes("262144 UTF-8 bytes"));
        }
    }
});
