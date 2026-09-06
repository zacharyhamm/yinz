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
    let onOutput = () => {};
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
    const session = {
        name: "example-session",
        is_current_session: true,
        panes: {}, // Headless daemon has no pane.
        plugins: {
            "42": { location: "zellij:hocus-agentd" },
            "7": { location: "zellij:hocus" },
            "8": { location: "file:unrelated.lua" },
        },
    };
    const snapshot = (sessions: unknown[] = [
        { ...session, name: "other-session", is_current_session: false },
        session,
    ]) =>
        JSON.stringify({
            event: "SessionUpdate",
            payload: { SessionUpdate: [sessions, []] },
        }) + "\n";
    let discovery = snapshot();
    let discoveryCode: number | undefined;
    let discoveries = 0;
    let killed = 0;
    let reaped = 0;
    let cancelled = 0;
    class FakeCommand {
        options: Deno.CommandOptions;

        constructor(binary: string | URL, options: Deno.CommandOptions) {
            this.options = options;
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
            if (this.options.args?.includes("events")) {
                discoveries++;
                onOutput();
                let close: () => void;
                let resolve: (status: Deno.CommandStatus) => void;
                const status = new Promise<Deno.CommandStatus>((done) => {
                    resolve = done;
                });
                return {
                    stdout: new ReadableStream<Uint8Array>({
                        start(controller) {
                            close = () => controller.close();
                            if (discovery !== "pending") {
                                const bytes = encoder.encode(discovery);
                                controller.enqueue(bytes.slice(0, 17));
                                controller.enqueue(bytes.slice(17));
                            }
                            if (discovery === "") {
                                controller.close();
                                resolve({
                                    success: false,
                                    code: 1,
                                    signal: null,
                                });
                            }
                        },
                        cancel() {
                            cancelled++;
                        },
                    }),
                    stderr: new ReadableStream<Uint8Array>({
                        start(c) {
                            c.close();
                        },
                    }),
                    kill(signal: string) {
                        equal(signal, "SIGKILL");
                        killed++;
                        if (discovery === "pending") close();
                        resolve({
                            success: false,
                            code: discoveryCode ?? 137,
                            signal: discoveryCode === undefined
                                ? "SIGKILL"
                                : null,
                        });
                    },
                    get status() {
                        reaped++;
                        return status;
                    },
                };
            }
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
            onOutput();
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

        const completion = model.methods.hocus_complete;
        const toolCall = {
            id: "call-1",
            name: "lookup",
            arguments: "{bad json",
        };
        const input = {
            request_id: "round-2",
            messages: [
                { role: "assistant" as const, tool_calls: [toolCall] },
                {
                    role: "tool" as const,
                    tool_call_id: "call-1",
                    content: "--literal $(text)",
                },
            ],
            system: "Be brief",
            model: "configured-override",
            tools: [{
                type: "function",
                function: { name: "lookup", parameters: { type: "object" } },
            }],
            tool_choice: { type: "function", function: { name: "lookup" } },
            temperature: 0.5,
            max_tokens: 123,
        };
        for (
            const reply of [
                {
                    request_id: "round-2",
                    content: "answer",
                    model: "actual-model",
                    finish_reason: "stop",
                    error: "",
                    usage: {
                        prompt_tokens: 3,
                        completion_tokens: 2,
                        total_tokens: 5,
                    },
                    tool_calls: [],
                },
                {
                    request_id: "round-2",
                    content: "",
                    model: "actual-model",
                    finish_reason: "tool_calls",
                    error: "",
                    usage: null,
                    tool_calls: [toolCall],
                },
                { request_id: "round-2", content: "no usage" },
            ]
        ) {
            responses = [{
                code: 0,
                stdout: JSON.stringify(reply) + "\n",
                stderr: "",
            }];
            equal(await completion.execute(input, context), {
                dataHandles: [{ name: "completion" }],
            });
            const argv = calls.pop()!;
            equal(argv.slice(0, -1), [
                "--session",
                "example-session",
                "pipe",
                "--name",
                "hocus_complete",
                "--plugin",
                "plugin_42",
                "--",
            ]);
            equal(calls.pop(), [
                "--session",
                "example-session",
                "events",
                "--type",
                "session_update",
            ]);
            equal(JSON.parse(argv.at(-1)!), completion.arguments.parse(input));
            const saved = writes.pop()!;
            equal({
                ...saved,
                data: model.resources.completion.schema.parse(saved.data),
            }, {
                spec: "completion",
                name: "completion",
                data: model.resources.completion.schema.parse({
                    session: "example-session",
                    ...reply,
                }),
            });
        }
        for (
            const invalid of [
                {},
                { messages: [] },
                { messages: [{ role: "invalid" }] },
                { messages: [{ role: "tool" }] },
                { messages: [{ role: "user", content: 1 }] },
                {
                    messages: [{
                        role: "user",
                        tool_calls: [{ name: "missing id" }],
                    }],
                },
                { messages: [{ role: "user" }], max_tokens: -1 },
                { messages: [{ role: "user" }], temperature: Infinity },
                { messages: Array(201).fill({ role: "user" }) },
                { messages: [{ role: "user" }], tools: Array(65).fill({}) },
                { messages: [{ role: "user", content: "é".repeat(32000) }] },
            ]
        ) {
            const count = calls.length;
            let failed = false;
            try {
                await completion.execute(
                    invalid as Parameters<typeof completion.execute>[0],
                    context,
                );
            } catch {
                failed = true;
            }
            equal(failed, true);
            equal(calls.length, count);
            equal(writes.length, 0);
        }
        for (
            const response of [
                { code: 1, stdout: "", stderr: "session not found" },
                ...[
                    "not json",
                    "{}",
                    "null",
                    JSON.stringify({ request_id: "wrong" }),
                    ...[
                        { content: 1 },
                        { tool_calls: [{}] },
                        { usage: { total_tokens: -1 } },
                        { error: "provider rate limit" },
                        { error: "cancelled" },
                    ].map((fields) =>
                        JSON.stringify({ request_id: "round-2", ...fields })
                    ),
                ].map((stdout) => ({ code: 0, stdout, stderr: "" })),
            ]
        ) {
            responses = [response];
            let failed = false;
            try {
                await completion.execute(input, context);
            } catch {
                failed = true;
            }
            equal(failed, true);
            equal(writes.length, 0);
        }
        for (const during of [false, true]) {
            const controller = new AbortController();
            const count = calls.length;
            if (during) onOutput = () => controller.abort();
            else controller.abort();
            responses = [{
                code: 0,
                stdout: JSON.stringify({
                    request_id: "round-2",
                    content: "late",
                }),
                stderr: "",
            }];
            let failed = false;
            try {
                await completion.execute(input, {
                    ...context,
                    signal: controller.signal,
                });
            } catch {
                failed = true;
            }
            onOutput = () => {};
            equal(failed, true);
            equal(calls.length, count + (during ? 1 : 0));
            equal(writes.length, 0);
        }

        for (
            const invalid of [
                "not json\n",
                "{}\n",
                "null\n",
                "",
                snapshot([]),
                snapshot([{ ...session, is_current_session: false }]),
                snapshot([{ ...session, name: "another-session" }]),
                snapshot([session, session]),
                snapshot([{ ...session, plugins: {} }]),
                snapshot([{
                    ...session,
                    plugins: { "7": { location: "zellij:hocus" } },
                }]),
                snapshot([{
                    ...session,
                    plugins: {
                        ...session.plugins,
                        "99": { location: "zellij:hocus-agentd" },
                    },
                }]),
                ...["bad", "-1", "1.5", "01", "4294967296"].map((id) =>
                    snapshot([{
                        ...session,
                        plugins: { [id]: { location: "zellij:hocus-agentd" } },
                    }])
                ),
                snapshot([{ ...session, plugins: null }]),
                "pending",
            ]
        ) {
            discovery = invalid;
            const count = calls.length;
            let failed = false;
            try {
                await completion.execute(input, {
                    ...context,
                    globalArgs: { ...context.globalArgs, timeoutMs: 10 },
                });
            } catch {
                failed = true;
            }
            equal(failed, true);
            equal(calls.length, count + 1);
            equal(calls.at(-1), [
                "--session",
                "example-session",
                "events",
                "--type",
                "session_update",
            ]);
            equal(writes.length, 0);
        }
        for (const mode of ["cancel", "failed"]) {
            discovery = mode === "cancel" ? "pending" : snapshot();
            discoveryCode = mode === "failed" ? 2 : undefined;
            const controller = new AbortController();
            const timer = mode === "cancel"
                ? setTimeout(() => controller.abort(), 10)
                : undefined;
            const count = calls.length;
            let error: unknown;
            try {
                await completion.execute(input, {
                    ...context,
                    signal: controller.signal,
                });
            } catch (caught) {
                error = caught;
            } finally {
                clearTimeout(timer);
            }
            equal(error instanceof Error, true);
            equal(calls.length, count + 1);
            equal(writes.length, 0);
        }
        discoveryCode = undefined;
        equal(killed, discoveries);
        equal(reaped >= discoveries, true);
        equal(cancelled > 0, true);
        discovery = snapshot();

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
