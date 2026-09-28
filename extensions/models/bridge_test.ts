import {
    observe,
    operation,
    preparePrompt,
    PromptText,
    StartupPromptText,
} from "./_lib/bridge.ts";
import { checked, Context } from "./_lib/workflow.ts";
import { methods, resources } from "./_lib/yinz_methods.ts";

function assert(value: unknown): asserts value {
    if (!value) throw new Error("assertion failed");
}
Deno.test("picker observations wait through reload uncertainty for a definite result", async () => {
    const dir = await Deno.makeTempDir();
    const binary = dir + "/bridge";
    await Deno.writeTextFile(
        binary,
        `#!/usr/bin/env python3
import json, pathlib, sys
assert json.load(sys.stdin)['method'] == 'observe'
path = pathlib.Path('operations.json')
operations = json.loads(path.read_text())
result = operations.pop(0)
path.write_text(json.dumps(operations))
print(json.dumps({'result': result}))
`,
    );
    await Deno.chmod(binary, 0o700);
    const c = {
        repoDir: dir,
        globalArgs: { session: "test", binary },
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: () => Promise.resolve(null),
        writeResource: () => Promise.resolve({ name: "unused" }),
    };
    try {
        for (const action of ["fork", "duplicate", "restart", "command"]) {
            for (const choice of ["started", "cancelled"]) {
                const unknown = {
                    work: "work",
                    key: "picker",
                    incarnation: "test",
                    request: JSON.stringify({
                        method: action === "command"
                            ? "command"
                            : "pane_action",
                        action,
                    }),
                    outcome: "unknown",
                };
                const picker = ["fork", "duplicate"].includes(action);
                const completed = { ...unknown, outcome: "completed", choice };
                await Deno.writeTextFile(
                    dir + "/operations.json",
                    JSON.stringify(
                        picker
                            ? [unknown, unknown, completed, completed]
                            : [unknown, unknown],
                    ),
                );
                const result = await observe(
                    c,
                    { work: "work", key: "picker" },
                    30,
                );
                assert(result.outcome === (picker ? "completed" : "unknown"));
                if (picker) assert(result.choice === choice);
            }
        }
    } finally {
        await Deno.remove(dir, { recursive: true });
    }
});
Deno.test("prompt preparation preserves exact limits and excerpts oversized UTF-8 logs", async () => {
    const saved = new Map<string, Record<string, unknown>>();
    const c: Context = {
        repoDir: "/tmp",
        globalArgs: {},
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: () => Promise.resolve(null),
        writeResource: (spec, name, value) => {
            resources[spec as keyof typeof resources].schema.parse(value);
            saved.set(name, value);
            return Promise.resolve({ name });
        },
    };
    assert(
        methods.prepare_prompt.arguments.parse({ prompt: "short prompt" })
            .startup === false,
    );
    for (const startup of [false, true]) {
        const limit = startup ? 131071 : 262144;
        const schema = startup ? StartupPromptText : PromptText;
        for (const character of ["x", "é", "界", "🙂"]) {
            const width = new TextEncoder().encode(character).length;
            const exact = character.repeat(Math.floor(limit / width)) +
                "x".repeat(limit % width);
            assert(schema.safeParse(exact).success);
            assert(!schema.safeParse(exact + "x").success);
            for (const prompt of ["short prompt", exact]) {
                assert(
                    preparePrompt(prompt, startup ? limit : undefined) ===
                        prompt,
                );
                await methods.prepare_prompt.execute({ prompt, startup }, c);
                assert(saved.get("prompt-current")?.prompt === prompt);
            }
            for (const padding of ["", "a", "ab", "abc"]) {
                const beginning =
                    "Resolve the rebase conflicts. Stage, then exit.\nLOG START\n" +
                    padding;
                const end = "\nLOG END\n";
                const prompt = beginning + exact + character + end;
                const prepared = preparePrompt(
                    prompt,
                    startup ? limit : undefined,
                );
                assert(schema.safeParse(prepared).success);
                assert(new TextEncoder().encode(prepared).length <= limit);
                assert(!prepared.includes("\ufffd"));
                const [head, tail] = prepared.split(
                    /\n\n\[Prompt truncated:.*\]\n\n/,
                );
                assert(head.startsWith(beginning) && prompt.startsWith(head));
                assert(tail.endsWith(end) && prompt.endsWith(tail));
                assert(prepared.includes("middle omitted"));
                assert(
                    prepared.includes(
                        startup ? "131071 UTF-8 bytes" : "256 KiB",
                    ),
                );
                assert(
                    prepared.includes("full evidence remains saved in Swamp"),
                );
                await methods.prepare_prompt.execute({ prompt, startup }, c);
                assert(saved.get("prompt-current")?.prompt === prepared);
            }
        }
    }
    // Exercise the actual argv boundary, beyond the bridge's stdin dispatch.
    await methods.prepare_prompt.execute({
        prompt: "x".repeat(262144),
        startup: true,
    }, c);
    const startupPrompt = String(saved.get("prompt-current")?.prompt);
    assert(new TextEncoder().encode(startupPrompt).length === 131071);
    assert(startupPrompt.includes("Prompt truncated"));
    assert(
        (await new Deno.Command("/usr/bin/true", { args: [startupPrompt] })
            .output())
            .success,
    );
    saved.clear();
    let rejected = false;
    try {
        await methods.prepare_prompt.execute({ prompt: "" }, c);
    } catch {
        rejected = true;
    }
    assert(rejected && saved.size === 0);
});
Deno.test("256 KiB UTF-8 prompts use one stdin JSON line and a full-text digest", async () => {
    const dir = await Deno.makeTempDir();
    const binary = dir + "/bridge";
    await Deno.writeTextFile(
        binary,
        `#!/usr/bin/env python3
import sys, json, hashlib
assert sys.argv[-1] == 'yinz:swamp-bridge'
line = sys.stdin.readline()
assert not sys.stdin.read()
request = json.loads(line)
assert request['prompt_sha256'] == hashlib.sha256(request['prompt'].encode()).hexdigest()
print(json.dumps({'result': dict(request, outcome='pending', incarnation='test', request=line)}))
`,
    );
    await Deno.chmod(binary, 0o700);
    const saved = new Map<string, Record<string, unknown>>();
    const c = {
        repoDir: dir,
        globalArgs: { session: "test", binary },
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: () => Promise.resolve(null),
        writeResource: (spec, name, value) => {
            resources[spec as keyof typeof resources].schema.parse(value);
            saved.set(name, value);
            return Promise.resolve({ name });
        },
    } satisfies Context;
    try {
        for (
            const prompt of [
                "x".repeat(262144),
                "🙂".repeat(65536),
                '\n"\\'.repeat(87381) + "x",
            ]
        ) {
            assert(new TextEncoder().encode(prompt).length === 262144);
            const result = await operation(c, {
                method: "agent_prompt",
                work: "work",
                key: "key",
                agent: "agent",
                prompt,
            });
            assert(JSON.parse(result.request).prompt === prompt);
            assert(!PromptText.safeParse(prompt + "x").success);
        }
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
            target: revision,
            gate: ["true"],
            vmSync: { mode: "local" as const },
        };
        const output = "LOG START\n" + "🙂".repeat(262144) + "\nLOG END";
        await methods.test_result.execute({
            operation: {
                key: "test",
                work: "work",
                incarnation: "test",
                request: "{}",
                outcome: "completed",
                exitCode: 1,
                log: "terminal_7",
                output,
            },
            evidence,
        }, c);
        const test = resources.test.schema.parse(saved.get("test-current"));
        const agent = { work: "work", key: "large", agent: "agent" };
        await methods.repair_prompt.execute({
            ...agent,
            assessment: {
                review: {
                    evidence,
                    outcome: "completed",
                    findings: [],
                    reviewer: "reviewer",
                    valid: true,
                    resolved: true,
                    problem: null,
                },
                test,
                ready: false,
            },
        }, c);
        const repair =
            JSON.parse(String(saved.get("operation-current")?.request)).prompt;
        assert(PromptText.safeParse(repair).success);
        assert(repair.startsWith("Resolve the following review findings"));
        assert(repair.includes("LOG START") && repair.includes("LOG END"));
        assert(repair.includes("middle omitted"));
        assert(saved.get("test-current")?.output === output);
        await methods.review_prompt.execute({
            ...agent,
            evidence: { ...evidence, gate: [output] },
        }, c);
        const review =
            JSON.parse(String(saved.get("operation-current")?.request)).prompt;
        assert(PromptText.safeParse(review).success);
        assert(review.startsWith("Review the checkout and source commit"));
        assert(review.includes("LOG START") && review.includes("LOG END"));
        assert(review.includes("middle omitted"));
        const abort = new AbortController();
        abort.abort();
        try {
            await checked({ signal: abort.signal }, dir, [binary], "input\n");
            throw new Error("not cancelled");
        } catch (error) {
            assert(
                error instanceof DOMException && error.name === "AbortError",
            );
        }
        try {
            await checked(c, dir, [
                "/bin/sh",
                "-c",
                "cat >/dev/null; echo failure >&2; exit 7",
            ], "x".repeat(262144));
            throw new Error("exit failure ignored");
        } catch (error) {
            assert(String(error).includes("failed (7): failure"));
        }
        try {
            await checked(
                c,
                dir,
                ["/bin/sh", "-c", "echo refused >&2; exit 9"],
                "x".repeat(262144),
            );
            throw new Error("exit failure ignored");
        } catch (error) {
            assert(String(error).includes("failed (9): refused"));
        }
        const runningAbort = new AbortController();
        const pending = checked({ signal: runningAbort.signal }, dir, [
            "/bin/sh",
            "-c",
            "echo ready; exec sleep 60",
        ], "x".repeat(262144));
        runningAbort.abort();
        try {
            await pending;
            throw new Error("not cancelled");
        } catch (error) {
            assert(
                error instanceof DOMException && error.name === "AbortError",
            );
        }
    } finally {
        await Deno.remove(dir, { recursive: true });
    }
});

Deno.test("commands default to background and accept foreground explicitly", () => {
    const args = {
        work: "work",
        key: "diff",
        argv: ["git", "diff"],
        cwd: "/checkout",
    };
    assert(methods.command.arguments.parse(args).foreground === false);
    assert(
        methods.command.arguments.parse({ ...args, foreground: true })
            .foreground === true,
    );
    assert(
        !methods.command.arguments.safeParse({ ...args, foreground: "true" })
            .success,
    );
});

Deno.test("bridge timings correlate attempts without logging payloads or failures", async () => {
    const { bridge } = await import("./_lib/bridge.ts");
    const dir = await Deno.makeTempDir();
    const previous = Deno.env.get("YINZ_SWAMP_TIMING");
    const logs: string[] = [];
    const binary = dir + "/bridge";
    const c = {
        repoDir: dir,
        globalArgs: { binary, session: "test" },
        signal: new AbortController().signal,
        modelId: "model-id",
        logger: {
            info: (line: string | TemplateStringsArray) => {
                assert(
                    typeof line !== "string" && line.length === 1 &&
                        line.raw[0] === line[0],
                );
                logs.push(line[0]);
            },
        },
    };
    try {
        await Deno.writeTextFile(
            binary,
            `#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{"result":"secret-output"}'\n`,
            { mode: 0o700 },
        );
        const request = {
            method: "identity",
            work: "work",
            key: "key",
            prompt: "secret-prompt",
            token: "secret-token",
        };
        Deno.env.delete("YINZ_SWAMP_TIMING");
        assert(await bridge(c, request) === "secret-output");
        assert(logs.length === 0);
        Deno.env.set("YINZ_SWAMP_TIMING", "1");
        assert(await bridge(c, request) === "secret-output");
        await Deno.writeTextFile(
            binary,
            `#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{"error":"secret-error"}'\n`,
        );
        try {
            await bridge(c, request);
            throw new Error("expected failure");
        } catch (error) {
            assert(String(error).includes("secret-error"));
        }
        await Deno.writeTextFile(
            binary,
            `#!/bin/sh\ncat >/dev/null\necho secret-stderr >&2\nexit 1\n`,
        );
        try {
            await bridge(c, request);
            throw new Error("expected failure");
        } catch (error) {
            assert(String(error).includes("secret-stderr"));
        }
        const records = logs.map((s) =>
            JSON.parse(s.slice("swamp_timing ".length))
        );
        assert(records.length === 5);
        assert(records[0].attempt === records[1].attempt);
        assert(records[0].attempt !== records[2].attempt);
        assert(records[3].phase === "parse" && records[3].outcome === "error");
        assert(
            records[4].phase === "process" && records[4].outcome === "error",
        );
        assert(
            records.every((r) =>
                r.modelId === "model-id" && r.work === "work" &&
                r.key === "key" && r.elapsedMs >= 0
            ),
        );
        assert(!logs.join("\n").includes("secret-"));
        logs.length = 0;
        await Deno.writeTextFile(
            binary,
            `#!/usr/bin/env python3
import json, pathlib, sys
request = json.load(sys.stdin)
p = pathlib.Path('poll-count')
n = int(p.read_text()) + 1 if p.exists() else 1
p.write_text(str(n))
print(json.dumps({'result': {'work':'work','key':'key','request':'{}','incarnation':'test','outcome':'pending' if n == 1 else 'completed'}}))
`,
        );
        const observed = await observe(
            {
                ...c,
                readResource: () => Promise.resolve(null),
                writeResource: () => Promise.resolve({ name: "unused" }),
            },
            { work: "work", key: "key" },
            5,
        );
        assert(observed.outcome === "completed");
        const summary = JSON.parse(logs.at(-1)!.slice("swamp_timing ".length));
        assert(summary.phase === "observation" && summary.polls === 3);
        assert(summary.waitMs >= 490 && summary.requestMs > 0);
        assert(summary.elapsedMs >= summary.waitMs + summary.requestMs);
        const abort = new AbortController();
        abort.abort();
        try {
            await bridge({ ...c, signal: abort.signal }, request);
            throw new Error("not cancelled");
        } catch (error) {
            assert(
                error instanceof DOMException && error.name === "AbortError",
            );
        }
        assert(
            JSON.parse(logs.at(-1)!.slice("swamp_timing ".length)).outcome ===
                "error",
        );
    } finally {
        if (previous === undefined) Deno.env.delete("YINZ_SWAMP_TIMING");
        else Deno.env.set("YINZ_SWAMP_TIMING", previous);
        await Deno.remove(dir, { recursive: true });
    }
});
