import {
    observe,
    operation,
    preparePrompt,
    PromptText,
} from "./_lib/bridge.ts";
import { checked, Context } from "./_lib/workflow.ts";
import { methods, resources } from "./_lib/zellij_methods.ts";

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
    for (const character of ["x", "é", "界", "🙂"]) {
        const width = new TextEncoder().encode(character).length;
        const exact = character.repeat(Math.floor(262144 / width)) +
            "x".repeat(262144 % width);
        for (const prompt of ["short prompt", exact]) {
            assert(preparePrompt(prompt) === prompt);
            await methods.prepare_prompt.execute({ prompt }, c);
            assert(saved.get("prompt-current")?.prompt === prompt);
        }
        for (const padding of ["", "a", "ab", "abc"]) {
            const beginning =
                "Fix the failed tests. Do not merge.\nLOG START\n" +
                padding;
            const end = "\nLOG END\n";
            const prompt = beginning + exact + character + end;
            const prepared = preparePrompt(prompt);
            assert(PromptText.safeParse(prepared).success);
            assert(new TextEncoder().encode(prepared).length <= 262144);
            assert(!prepared.includes("\ufffd"));
            const [head, tail] = prepared.split(
                /\n\n\[Prompt truncated:.*\]\n\n/,
            );
            assert(head.startsWith(beginning) && prompt.startsWith(head));
            assert(tail.endsWith(end) && prompt.endsWith(tail));
            assert(prepared.includes("middle omitted"));
            assert(prepared.includes("full evidence remains saved in Swamp"));
            await methods.prepare_prompt.execute({ prompt }, c);
            assert(saved.get("prompt-current")?.prompt === prepared);
        }
    }
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
assert sys.argv[-1] == 'zellij:swamp-bridge'
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
