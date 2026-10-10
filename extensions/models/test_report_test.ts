import { captureCommand, readCapture, testReport } from "./_lib/test_report.ts";
import { methods, resources } from "./_lib/yinz_methods.ts";
import { Context } from "./_lib/workflow.ts";

function assert(value: unknown, message = "assertion failed"): asserts value {
    if (!value) throw new Error(message);
}
const revision = {
    checkout: "/tmp/topic",
    branch: "topic",
    commit: "a".repeat(40),
    contents: "content",
    clean: true,
    conflicts: [],
    rebasing: false,
};
const evidence = {
    source: revision,
    target: revision,
    gate: ["cargo", "x", "ci", "check"],
};
const base = { evidence, outcome: "failed", exitCode: 1, outputComplete: true };
const nextest = (name: string, state = "FAIL") =>
    `        ${state} [  0.010s] (1/3) crate ${name}\n--- STDOUT:              crate ${name} ---\n${name} failure output\n--- STDERR:              crate ${name} ---\n${name} stderr\n`;

Deno.test("Cargo/nextest reports retain success, duplicates, timeout, abort, doctests and non-test diagnostics", () => {
    const fixtures = [
        {
            output: "test result: ok. 2 passed; 0 failed\n",
            outcome: "completed",
            exitCode: 0,
            names: [],
            status: "PASSED",
        },
        {
            output: nextest("first") + nextest("middle", "TIMEOUT") +
                nextest("last", "SIGABRT") +
                "test first ... FAILED\nfailures:\n    first\n\ntest result: FAILED. 0 passed; 1 failed\nSummary [ 1s] 3 tests run: 3 failed\nFAIL [ 0.010s] crate first\nTIMEOUT [ 0.010s] crate middle\nSIGABRT [ 0.010s] crate last",
            names: ["crate first", "crate last", "crate middle"],
        },
        {
            output:
                "   Doc-tests thing\ntest src/lib.rs - example (line 5) ... FAILED\nfailures:\n\n---- src/lib.rs - example (line 5) stdout ----\nerror[E0308]: mismatch\n\nfailures:\n    src/lib.rs - example (line 5)\n\ntest result: FAILED. 0 passed; 1 failed\n",
            names: ["doctest thing: src/lib.rs - example (line 5)"],
        },
        {
            output:
                "Running unittests src/lib.rs (target/debug/deps/thing-abc)\ntest unit::bad ... FAILED\nfailures:\n\n---- unit::bad stdout ----\nassertion failed\n\nfailures:\n    unit::bad\n\ntest result: FAILED. 0 passed; 1 failed\n",
            names: [
                "unittests src/lib.rs (target/debug/deps/thing-abc): unit::bad",
            ],
        },
        {
            output:
                "Diff in src/lib.rs:1\nerror: clippy::lint\nerror[E0308]: mismatch\nerror: could not compile `thing`\n>> Failed: format --check, lint, test\n",
            names: [],
            gate: true,
        },
        { output: "unknown runner diagnostic 😀𝄞\n", names: [] },
    ];
    for (const fixture of fixtures) {
        const report = testReport({ ...base, ...fixture });
        assert(
            JSON.stringify(report.failedTests) ===
                JSON.stringify(fixture.names),
            report.summary,
        );
        assert(
            report.summary.startsWith(
                "Test report: " + (fixture.status ?? "FAILED"),
            ),
        );
        assert(report.summary.endsWith(fixture.output));
        assert(report.summaryLength === report.summary.length);
        if (fixture.gate) {
            assert(
                report.summary.includes(
                    "Non-test gate errors: formatting, lint, build",
                ),
            );
        }
    }
    for (const outputComplete of [false, undefined]) {
        const report = testReport({
            ...base,
            outcome: "completed",
            exitCode: 0,
            output: "",
            outputComplete,
        });
        assert(
            !report.passed && report.summary.includes("INCOMPLETE") &&
                report.summary.includes("output unavailable"),
        );
    }
});

Deno.test("private concurrent capture preserves literal argv, full large stdout/stderr, exit codes and tee errors", async () => {
    await Promise.all([0, 7].map(async (exitCode) => {
        const directory = await Deno.makeTempDir({
            dir: "/tmp",
            prefix: "yinz-test-",
        });
        try {
            assert(
                (await Deno.stat(directory)).mode! % 512 === 448,
                "capture must be private",
            );
            const literal = "$(touch SHOULD-NOT-EXIST) `false` 'quoted'";
            const argv = captureCommand(directory, [
                "/usr/bin/python3",
                "-c",
                "import sys; print(sys.argv[1]); print('BEGIN'); print('x'*1200000); print('MIDDLE'); print('x'*1200000); print('END',file=sys.stderr); sys.exit(int(sys.argv[2]))",
                literal,
                String(exitCode),
            ]);
            const result = await new Deno.Command(argv[0], {
                args: argv.slice(1),
                stdout: "piped",
                stderr: "piped",
            }).output();
            assert(result.code === exitCode);
            const captured = await readCapture(directory, exitCode);
            assert(
                captured.outputComplete &&
                    captured.output === new TextDecoder().decode(result.stdout),
            );
            assert(
                captured.output.includes(literal) &&
                    captured.output.includes("BEGIN") &&
                    captured.output.includes("MIDDLE") &&
                    captured.output.includes("END"),
            );
            assert(captured.output.length > 2400000);
            await Deno.remove(directory + "/output");
            await Deno.mkdir(directory + "/output");
            const broken = captureCommand(directory, [
                "/bin/sh",
                "-c",
                "printf diagnostic; exit 0",
            ]);
            const error = await new Deno.Command(broken[0], {
                args: broken.slice(1),
                stdout: "null",
                stderr: "piped",
            }).output();
            assert(
                error.code === 0,
                "tee must not replace the gate's exit status",
            );
            const failed = await readCapture(directory, 0);
            assert(
                !failed.outputComplete &&
                    failed.captureError?.includes("tee failure"),
            );
            assert(
                !testReport({
                    ...base,
                    ...failed,
                    outcome: "completed",
                    exitCode: 0,
                }).passed,
            );
        } finally {
            await Deno.remove(directory, { recursive: true });
        }
    }));
});

Deno.test("capture survives a failed evidence save; saved reports retain beginning, middle and end and resolve for fix prompts", async () => {
    const directory = await Deno.makeTempDir({
        dir: "/tmp",
        prefix: "yinz-test-",
    });
    const saved = new Map<string, Record<string, unknown>>();
    let failSave = true;
    const binary = directory + "-bridge";
    await Deno.writeTextFile(
        binary,
        `#!/usr/bin/env python3
import json, pathlib, sys
assert json.load(sys.stdin)['method'] == 'command_close'
assert pathlib.Path('${directory}/saved').exists(), 'cleanup before save'
assert not pathlib.Path('${directory}/fail-cleanup').exists(), 'cleanup failed'
print('{"result": {}}')
`,
    );
    await Deno.chmod(binary, 0o700);
    const c: Context = {
        repoDir: "/tmp",
        modelType: { normalized: "@hammz/yinz" },
        modelId: "saved",
        globalArgs: { session: "test", binary },
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: (name) => Promise.resolve(saved.get(name) ?? null),
        writeResource: async (spec, name, value) => {
            if (failSave) throw new Error("storage unavailable");
            resources[spec as keyof typeof resources].schema.parse(value);
            saved.set(name, value);
            await Deno.writeTextFile(directory + "/saved", "yes").catch(
                () => {},
            );
            return Promise.resolve({ name, version: 1 });
        },
    };
    const log = nextest("first") + "padding\n".repeat(160000) +
        nextest("middle") + "padding\n".repeat(160000) + nextest("last");
    await Deno.writeTextFile(directory + "/output", log);
    await Deno.writeTextFile(directory + "/status", "1 0\n");
    await Deno.writeTextFile(directory + "/error", "");
    const input = {
        evidence,
        operation: {
            key: "test",
            work: "work",
            incarnation: "test",
            request: "{}",
            outcome: "failed" as const,
            exitCode: 1,
            log: "terminal_7",
            capture: { directory },
        },
    };
    try {
        let rejected = false;
        try {
            await methods.test_result.execute(input, c);
        } catch {
            rejected = true;
        }
        assert(rejected && (await Deno.stat(directory)).isDirectory);
        failSave = false;
        await Deno.writeTextFile(directory + "/fail-cleanup", "yes");
        rejected = false;
        try {
            await methods.test_result.execute(input, c);
        } catch (error) {
            rejected = String(error).includes(
                "Evidence saved; capture cleanup failed",
            );
        }
        assert(rejected && (await Deno.stat(directory)).isDirectory);
        const original = JSON.stringify(saved.get("test-current"));
        await Deno.writeTextFile(
            directory + "/output",
            "a rerun must not replace evidence",
        );
        await Deno.remove(directory + "/fail-cleanup");
        await methods.test_result.execute(input, c);
        assert(JSON.stringify(saved.get("test-current")) === original);
        // A closed pane cleanup is idempotent even after the capture is gone.
        await Deno.writeTextFile(
            binary,
            "#!/bin/sh\nprintf '%s' '{\"result\": {}}'\n",
        );
        let removed = false;
        try {
            await Deno.stat(directory);
        } catch (error) {
            removed = error instanceof Deno.errors.NotFound;
        }
        assert(removed);
        const test = resources.test.schema.parse(saved.get("test-current"));
        assert(
            test.output === log && test.outputComplete &&
                test.failedTests?.length === 3,
        );
        for (const name of ["first", "middle", "last"]) {
            assert(test.summary?.includes(name + " failure output"));
        }
        await methods.test_result.execute(input, c);
        assert(
            saved.get("test-current")?.summary === test.summary,
            "retry after cleanup lost evidence",
        );
        const short = {
            ...test,
            output: "failure-evidence",
            summary: "Test report: FAILED\nfailure-evidence",
        };
        c.dataRepository = {
            findAllGlobal: () => Promise.resolve([]),
            getContent: () =>
                Promise.resolve(
                    new TextEncoder().encode(JSON.stringify(short)),
                ),
        };
        await methods.prepare_prompt.execute({
            prompt: "Fix the failed tests.",
            test: {
                ...short,
                output: "preview",
                summary: undefined,
                outputRef: {
                    modelType: "@hammz/yinz",
                    modelId: "saved",
                    name: "test-current",
                    version: 1,
                },
            },
        }, c);
        assert(
            saved.get("prompt-current")?.prompt ===
                "Fix the failed tests.\n\n" + short.summary,
        );
        const summary = "REPORT START\n" + "🙂".repeat(40000) + "\nREPORT END";
        for (const startup of [false, true]) {
            await methods.prepare_prompt.execute({
                prompt: "Fix the failed tests.",
                test: { ...short, summary },
                startup,
            }, c);
            const prompt = String(saved.get("prompt-current")?.prompt);
            assert(prompt.startsWith("Fix the failed tests.\n\n"));
            assert(!prompt.includes("\ufffd"));
            if (startup) {
                assert(new TextEncoder().encode(prompt).length <= 131071);
                assert(
                    prompt.includes("swamp data query") &&
                        String(saved.get("repair-test")?.summary) === summary,
                );
            } else {
                assert(prompt === "Fix the failed tests.\n\n" + summary);
            }
        }
        c.dataRepository.getContent = () =>
            Promise.resolve(new TextEncoder().encode(JSON.stringify(test)));
        await methods.prepare_prompt.execute({
            prompt: "Fix the failed tests.",
            test: {
                ...test,
                output: "preview",
                summary: undefined,
                outputRef: {
                    modelType: "@hammz/yinz",
                    modelId: "saved",
                    name: "test-current",
                    version: 1,
                },
            },
        }, c);
        const prompt = String(saved.get("prompt-current")?.prompt);
        assert(
            prompt.includes('ownerRef == "saved"') &&
                prompt.includes("version == 1"),
        );
        assert(
            prompt.includes("swamp data query ") &&
                !prompt.includes("Prompt truncated"),
        );
        assert(new TextEncoder().encode(prompt).length <= 262144);
    } finally {
        await Deno.remove(binary);
        try {
            await Deno.remove(directory, { recursive: true });
        } catch (error) {
            if (!(error instanceof Deno.errors.NotFound)) throw error;
        }
    }
});

Deno.test("foreground tests stream output and preserve exit status only after dismissal", async () => {
    for (const code of [0, 7]) {
        for (const dismiss of [true, false, "interrupt"]) {
            const directory = await Deno.makeTempDir({
                dir: "/tmp",
                prefix: "yinz-test-",
            });
            const notify = directory + "/notify";
            await Deno.writeTextFile(
                notify,
                '#!/bin/sh\ncat > "' + directory + '/finished"\n',
            );
            await Deno.chmod(notify, 0o700);
            const argv = captureCommand(
                directory,
                [
                    "bash",
                    "-c",
                    `echo LIVE; echo DIAGNOSTIC >&2; exit ${code}`,
                ],
                true,
                {
                    binary: notify,
                    session: "session",
                    work: "PROCESS",
                    key: "PROCESS",
                },
            );
            const child = new Deno.Command(argv[0], {
                args: argv.slice(1),
                stdin: "piped",
                stdout: "piped",
                stderr: "piped",
            }).spawn();
            const errors = new Response(child.stderr).text();
            let exited = false;
            const status = child.status.then((s) => {
                exited = true;
                return s;
            });
            let output = "", sent = false;
            for await (
                const text of child.stdout.pipeThrough(new TextDecoderStream())
            ) {
                output += text;
                if (!sent && output.includes("Press Enter to return.")) {
                    assert(
                        !exited && output.includes("LIVE") &&
                            output.includes("DIAGNOSTIC"),
                    );
                    const capture = await readCapture(directory, code);
                    assert(
                        capture.outputComplete &&
                            capture.output.includes("DIAGNOSTIC"),
                    );
                    sent = true;
                    if (dismiss === "interrupt") child.kill("SIGINT");
                    else {
                        const writer = child.stdin.getWriter();
                        if (dismiss) {
                            await writer.write(new TextEncoder().encode("\n"));
                        }
                        await writer.close();
                    }
                }
            }
            const finished = JSON.parse(
                await Deno.readTextFile(directory + "/finished"),
            );
            assert(
                finished.process === child.pid && finished.work === "PROCESS" &&
                    finished.operation === "PROCESS",
            );
            assert(sent && (await status).code === (dismiss ? code : 125));
            if (dismiss === "interrupt") await child.stdin.close();
            assert(!await errors);
            await Deno.remove(directory, { recursive: true });
        }
    }
});
