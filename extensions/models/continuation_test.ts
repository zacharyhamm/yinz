import { methods } from "./_lib/zellij_methods.ts";
import { Context, digest } from "./_lib/workflow.ts";

function assert(value: unknown): asserts value {
    if (!value) throw new Error("assertion failed");
}
Deno.test("continuations preserve large inputs, retry argv, reports and uncertain file lifetime", async () => {
    const dir = await Deno.makeTempDir();
    const binary = dir + "/bridge";
    await Deno.writeTextFile(
        binary,
        `#!/usr/bin/env python3
import json, pathlib, subprocess, sys
root = pathlib.Path.cwd()
request = json.load(sys.stdin)
mode = (root / 'mode').read_text()
path = root / 'request.json'
if request['method'] == 'command':
    previous = json.loads(path.read_text()) if path.exists() else request
    assert previous == request, 'retry changed request identity'
    path.write_text(json.dumps(request))
    argv = request['argv']
    assert argv[:3] == ['swamp', 'workflow', 'run']
    assert '--input' not in argv and '--no-supersede' in argv
    assert sum(len(a.encode()) + 1 for a in argv) < 4096
    inputs = pathlib.Path(argv[argv.index('--input-file') + 1])
    assert inputs.is_absolute() and inputs.exists()
    if mode == 'dispatch-lost':
        sys.exit(7)
    child = subprocess.run([sys.executable, 'child.py', *argv[1:]], capture_output=True, text=True)
    (root / 'child-result.json').write_text(json.dumps(dict(exitCode=child.returncode, output=child.stdout + child.stderr)))
    outcome = 'pending'
else:
    argv = json.loads(path.read_text())['argv']
    assert pathlib.Path(argv[argv.index('--input-file') + 1]).exists()
    if mode == 'observe-lost':
        sys.exit(8)
    outcome = 'unknown' if mode == 'unknown' else 'failed' if mode == 'failure' else 'completed'
result = dict(key=request['key'], work=request['work'], incarnation='test', request='{}', outcome=outcome)
if request['method'] == 'observe':
    result.update(json.loads((root / 'child-result.json').read_text()))
print(json.dumps(dict(result=result)))
`,
    );
    await Deno.chmod(binary, 0o700);
    await Deno.writeTextFile(
        dir + "/child.py",
        `import json, pathlib, sys
root = pathlib.Path.cwd()
path = pathlib.Path(sys.argv[sys.argv.index('--input-file') + 1])
assert json.loads(path.read_text()) == json.loads((root / 'expected.json').read_text())
print('child diagnostic')
sys.exit(23 if (root / 'mode').read_text() == 'failure' else 0)
`,
    );
    const saved: Record<string, unknown>[] = [];
    const c: Context = {
        repoDir: dir,
        globalArgs: {},
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: () => Promise.resolve(null),
        writeResource: (_spec, name, value) => {
            saved.push(structuredClone(value));
            return Promise.resolve({ name });
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
                    output: '🙂\n"\\'.repeat(65536),
                    passed: false,
                },
            },
        },
    });
    const inputFile = dir + "/.swamp/continuation-inputs/" +
        await digest(input) + ".json";
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
            if (["dispatch-lost", "observe-lost", "unknown"].includes(mode)) {
                assert(
                    JSON.parse(await Deno.readTextFile(inputFile)).state.test
                        .output === input.inputs.state.test!.output,
                );
            } else {
                let removed = false;
                try {
                    await Deno.stat(inputFile);
                } catch (e) {
                    removed = e instanceof Deno.errors.NotFound;
                }
                assert(removed);
                assert(saved.length === 2);
                assert(saved[0].outcome === "pending");
                assert(saved[1].exitCode === (mode === "failure" ? 23 : 0));
                if (mode === "failure") {
                    assert(error.includes("Continuation failed"));
                    assert(saved[1].output === "child diagnostic\n");
                } else assert(saved[1].output === undefined);
            }
        }
    } finally {
        await Deno.remove(dir, { recursive: true });
    }
});
