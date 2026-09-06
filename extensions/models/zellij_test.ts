import { model } from "./zellij.ts";

Deno.test("Zellij methods preserve argv, validate snapshots, and fail without writing", async () => {
    const original = Deno.Command;
    const calls: string[][] = [];
    const writes: {
        spec: string;
        name: string;
        data: Record<string, unknown>;
    }[] = [];
    let responses: { code: number; stdout: string; stderr: string }[] = [];
    const encoder = new TextEncoder();
    function equal(actual: unknown, expected: unknown) {
        if (JSON.stringify(actual) !== JSON.stringify(expected)) {
            throw new Error(
                `Expected ${JSON.stringify(expected)}, got ${
                    JSON.stringify(actual)
                }`,
            );
        }
    }
    const context = {
        globalArgs: {
            session: "example-session",
            binary: "zellij",
            timeoutMs: 1000,
        },
        signal: new AbortController().signal,
        logger: { info: (_message: string) => {} },
        writeResource: (
            spec: string,
            name: string,
            data: Record<string, unknown>,
        ) => {
            model.resources[spec as keyof typeof model.resources].schema.parse(
                data,
            );
            writes.push({ spec, name, data });
            return Promise.resolve({ name });
        },
    };
    class FakeCommand {
        constructor(binary: string | URL, options: Deno.CommandOptions) {
            equal(binary, "zellij");
            equal(
                options.stdin,
                options.args?.includes("zellij:swamp-bridge")
                    ? "piped"
                    : "null",
            );
            calls.push(options.args ?? []);
        }
        spawn() {
            return {
                stdin: new WritableStream<Uint8Array>({
                    write(bytes) {
                        calls.at(-1)!.push(new TextDecoder().decode(bytes));
                    },
                }),
                output: () => this.output(),
            };
        }
        output() {
            const response = responses.shift();
            if (!response) throw new Error("Unexpected command");
            return Promise.resolve({
                code: response.code,
                success: response.code === 0,
                stdout: encoder.encode(response.stdout),
                stderr: encoder.encode(response.stderr),
            });
        }
    }
    Deno.Command = FakeCommand as unknown as typeof Deno.Command;
    try {
        responses = [{ code: 0, stdout: "terminal_42\n", stderr: "" }];
        await model.methods.action.execute({
            action: "write-chars",
            args: ["$(touch /tmp/never); text"],
        }, context);
        equal(calls.pop(), [
            "--session",
            "example-session",
            "action",
            "write-chars",
            "$(touch /tmp/never); text",
        ]);
        equal(writes.pop()?.data.stdout, "terminal_42\n");

        responses = [{ code: 0, stdout: "reply", stderr: "" }];
        await model.methods.pipe.execute({
            name: "echo",
            plugin: "file:example.lua",
            payload: "--literal",
            args: "key=value",
            configuration: "mode=test",
        }, context);
        equal(calls.pop(), [
            "--session",
            "example-session",
            "pipe",
            "--name",
            "echo",
            "--plugin",
            "file:example.lua",
            "--args",
            "key=value",
            "--plugin-configuration",
            "mode=test",
            "--",
            "--literal",
        ]);
        equal(writes.pop()?.name, "pipe");

        const pane = {
            id: 42,
            is_plugin: false,
            title: "shell",
            tab_id: 0,
            tab_name: "work",
            is_focused: true,
            is_floating: false,
            is_suppressed: false,
            exited: false,
            plugin_url: null,
        };
        const tab = { tab_id: 0, position: 0, name: "work", active: true };
        responses = [
            { code: 0, stdout: JSON.stringify([pane]), stderr: "" },
            { code: 0, stdout: JSON.stringify([tab]), stderr: "" },
        ];
        await model.methods.inspect.execute({}, context);
        equal(writes.pop()?.data, {
            session: "example-session",
            panes: [pane],
            tabs: [tab],
        });

        const operation = {
            work: "typed",
            key: "agent",
            request: "{}",
            incarnation: "server",
            outcome: "completed",
            pane: 9,
            supervisor: "supervisor",
            process: 42,
            checkout: "/work/topic",
            branch: "topic",
        };
        responses = [{
            code: 0,
            stdout: JSON.stringify({ result: operation }),
            stderr: "",
        }];
        const ownedContext = {
            ...context,
            repoDir: ".",
            readResource: () => Promise.resolve(null),
        };
        await model.methods.agent_spawn.execute({
            work: "typed",
            key: "agent",
            spec: {
                kind: "codex",
                command: "/stub agent",
                args: ["$(literal)"],
            },
        }, ownedContext);
        const request = JSON.parse(calls.pop()!.at(-1)!);
        equal(request.spec, { command: "/stub agent", args: ["$(literal)"] });
        const savedOperation = writes.pop()!.data;
        for (const [key, value] of Object.entries(operation)) {
            equal(savedOperation[key], value);
        }
        let invalid = false;
        try {
            await model.methods.command.execute({
                work: "typed",
                key: "bad",
                cwd: ".",
                argv: ["invalid\0argument"],
            }, ownedContext);
        } catch {
            invalid = true;
        }
        equal(invalid, true);

        for (
            const response of [
                { code: 1, stdout: "", stderr: "session not found" },
                { code: 0, stdout: "{}", stderr: "" },
            ]
        ) {
            responses = [response, { code: 0, stdout: "[]", stderr: "" }];
            let failed = false;
            try {
                await model.methods.inspect.execute({}, context);
            } catch {
                failed = true;
            }
            equal(failed, true);
            equal(writes.length, 0);
        }
        const count = calls.length;
        let aborted = false;
        try {
            await model.methods.action.execute({
                action: "list-panes",
                args: [],
            }, { ...context, signal: AbortSignal.abort() });
        } catch {
            aborted = true;
        }
        equal(aborted, true);
        equal(calls.length, count);
    } finally {
        Deno.Command = original;
    }
});
