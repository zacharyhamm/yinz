import { operation, PromptText } from "./_lib/bridge.ts";
import { checked, Context } from "./_lib/workflow.ts";

function assert(value: unknown): asserts value {
    if (!value) throw new Error("assertion failed");
}
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
    const c = {
        repoDir: dir,
        globalArgs: { session: "test", binary },
        signal: new AbortController().signal,
        logger: { info() {} },
        readResource: () => Promise.resolve(null),
        writeResource: () => {
            throw new Error("unexpected write");
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
