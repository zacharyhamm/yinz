import { methods } from "./_lib/zellij_methods.ts";
import { Context } from "./_lib/workflow.ts";

function assert(value: unknown): asserts value {
    if (!value) throw new Error("assertion failed");
}
Deno.test("continuations preserve typed inputs, operation identity, reports and uncertain outcomes", async () => {
    const dir = await Deno.makeTempDir();
    const binary = dir + "/bridge";
    await Deno.writeTextFile(
        binary,
        `#!/usr/bin/env python3
import json, pathlib, sys
root = pathlib.Path.cwd()
request = json.load(sys.stdin)
mode = (root / 'mode').read_text()
path = root / 'request.json'
if request['method'] == 'agent_prompt':
    assert len(request['prompt'].encode()) <= 262144
    assert 'Prompt truncated' in request['prompt']
    assert 'newer' not in request['prompt']
    print(json.dumps(dict(result=dict(key=request['key'], work=request['work'], incarnation='test', request='{}', outcome='pending'))))
    sys.exit(0)
if request['method'] == 'swamp_dispatch':
    previous = json.loads(path.read_text()) if path.exists() else request
    assert previous == request, 'retry changed request identity'
    path.write_text(json.dumps(request))
    assert request['operation'] == 'workflow.run'
    assert request['payload']['noSupersede'] is True
    assert len(json.dumps(request).encode()) < 262144
    assert request['payload']['inputs']['state']['test']['outputRef']['version'] == 1
    assert 'argv' not in request
    if mode == 'dispatch-lost':
        sys.exit(7)
    outcome = 'pending'
else:
    assert request['method'] == 'observe'
    if mode == 'observe-lost':
        sys.exit(8)
    outcome = 'unknown' if mode == 'unknown' else 'failed' if mode == 'failure' else 'completed'
result = dict(key=request['key'], work=request['work'], incarnation='test', request='{}', outcome=outcome, transport='serve')
if request['method'] == 'observe':
    result.update(exitCode=23 if mode == 'failure' else 0, output='child diagnostic\\n')
print(json.dumps(dict(result=result)))
`,
    );
    await Deno.chmod(binary, 0o700);
    const saved: Record<string, unknown>[] = [];
    const pointers = new Map<string, Record<string, unknown>>();
    const records: Array<
        {
            modelType: { normalized: string };
            modelId: string;
            data: {
                name: string;
                version: number;
                tags: Record<string, string>;
            };
            value: Record<string, unknown>;
        }
    > = [];
    const c: Context = {
        modelType: { normalized: "@hammz/zellij" },
        modelId: "original",
        dataRepository: {
            findAllGlobal: () =>
                Promise.resolve(
                    records.filter((r) =>
                        !records.some((other) =>
                            other.data.name === r.data.name &&
                            other.data.version > r.data.version
                        )
                    ),
                ),
            getContent: (type, id, name, version) =>
                Promise.resolve((() => {
                    const record = records.find((r) =>
                        r.modelType.normalized === type && r.modelId === id &&
                        r.data.name === name && r.data.version === version
                    );
                    return record
                        ? new TextEncoder().encode(JSON.stringify(record.value))
                        : null;
                })()),
        },
        repoDir: dir,
        globalArgs: {},
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: (name) => Promise.resolve(pointers.get(name) ?? null),
        writeResource: (spec, name, value) => {
            if (spec === "test") {
                records.push({
                    modelType: c.modelType!,
                    modelId: c.modelId!,
                    data: { name, version: 1, tags: { specName: spec } },
                    value: structuredClone(value),
                });
            }
            if (spec === "continuation") {
                pointers.set(name, structuredClone(value));
            }
            if (spec === "operation") saved.push(structuredClone(value));
            return Promise.resolve({ name, version: 1 });
        },
    };
    const revision = {
        checkout: dir,
        branch: "topic",
        commit: "a".repeat(40),
        contents: "contents",
        clean: true,
        conflicts: [],
        rebasing: false,
    };
    const evidence = {
        source: revision,
        target: revision,
        gate: ["true"],
        vmSync: { mode: "local" as const },
    };
    const input = methods.continue_flow.arguments.parse({
        work: "work",
        key: "continue",
        workflow: "@hammz/zellij-review-integration",
        inputs: {
            flowVersion: 2,
            launch: "work",
            session: "test",
            binary,
            checkout: dir,
            target: dir,
            targetBranch: "main",
            gate: ["true"],
            kind: "",
            agentCommand: "",
            agentArgs: [],
            firecracker: "",
            firecrackerImage: "",
            sourcePane: -1,
            phase: "failed-tests",
            state: {
                vmSync: { mode: "local" },
                evidence,
                review: null,
                test: {
                    evidence,
                    outcome: "completed",
                    exitCode: 1,
                    log: "terminal_1",
                    output: '🙂\n"\\'.repeat(600000),
                    passed: false,
                },
            },
        },
    });
    // Reuse the original test result instead of copying its multi-megabyte output.
    records.push({
        modelType: c.modelType!,
        modelId: c.modelId!,
        data: { name: "test-current", version: 1, tags: { specName: "test" } },
        value: structuredClone(input.inputs.state.test!),
    });
    try {
        await Deno.writeTextFile(
            dir + "/expected.json",
            JSON.stringify(input.inputs),
        );
        for (
            const mode of [
                "dispatch-lost",
                "observe-lost",
                "unknown",
                "failure",
                "success",
            ]
        ) {
            await Deno.writeTextFile(dir + "/mode", mode);
            saved.length = 0;
            let error = "";
            try {
                await methods.continue_flow.execute(input, c);
            } catch (e) {
                error = String(e);
            }
            assert(mode === "success" ? !error : !!error);
            let noInputFiles = false;
            try {
                await Deno.stat(dir + "/.swamp/continuation-inputs");
            } catch (e) {
                noInputFiles = e instanceof Deno.errors.NotFound;
            }
            assert(noInputFiles);
            if (mode === "failure" || mode === "success") {
                assert(saved.length === 2);
                assert(saved[0].outcome === "pending");
                assert(saved[1].exitCode === (mode === "failure" ? 23 : 0));
                if (mode === "failure") {
                    assert(error.includes("Continuation failed"));
                    assert(saved[1].output === "child diagnostic\n");
                } else assert(saved[1].output === undefined);
            }
        }
        assert(records.length === 1);
        const request = JSON.parse(
            await Deno.readTextFile(dir + "/request.json"),
        );
        assert(
            request.payload.inputs.state.test.outputRef.name === "test-current",
        );
        assert(records[0].value.output === input.inputs.state.test!.output);
        // A later version must not change the referenced repair evidence.
        records.push({
            ...records[0],
            data: { ...records[0].data, version: 2 },
            value: { ...records[0].value, output: "newer" },
        });
        const { resolveEvidence } = await import("./_lib/evidence.ts");
        const resolved = await resolveEvidence(
            c.dataRepository!,
            request.payload.inputs.state.test,
        );
        assert(resolved.output === input.inputs.state.test!.output);
        assert(request.payload.inputs.state.test.output.isWellFormed());
        const missing = structuredClone(request.payload.inputs.state.test);
        missing.outputRef.version = 99;
        assert(
            (await resolveEvidence(c.dataRepository!, missing)).output === null,
        );
        c.globalArgs = { session: "test", binary };
        await methods.repair_prompt.execute({
            work: "work",
            key: "repair",
            agent: "worker",
            assessment: {
                ready: false,
                review: {
                    evidence,
                    outcome: "completed",
                    findings: [],
                    reviewer: "reviewer",
                    valid: true,
                    resolved: true,
                    problem: null,
                },
                test: request.payload.inputs.state.test,
            },
        }, c);
        await methods.continue_flow.execute(input, c);
        // Changing the original payload must still be rejected by dispatch identity.
        const changed = structuredClone(input);
        changed.inputs.state.test!.output += "changed";
        let rejected = false;
        try {
            await methods.continue_flow.execute(changed, c);
        } catch {
            rejected = true;
        }
        assert(rejected);
        // Legacy input without matching saved content is persisted once on retries.
        changed.key = "legacy";
        await Deno.remove(dir + "/request.json");
        await methods.continue_flow.execute(changed, c);
        const count = records.length;
        await methods.continue_flow.execute(changed, c);
        assert(records.length === count);
    } finally {
        await Deno.remove(dir, { recursive: true });
    }
});
