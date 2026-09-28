/** Control an existing Yinz session through its CLI. @module */
import { z } from "npm:zod@4.3.6";
import { methods, resources } from "./_lib/yinz_methods.ts";

const GlobalArgs = z.object({
    session: z.string().min(1),
    binary: z.string().min(1).default("yinz"),
    timeoutMs: z.number().int().positive().default(30000),
});
const ActionArgs = z.object({
    action: z.string().regex(/^[a-z][a-z0-9-]*$/),
    args: z.array(z.string()).default([]),
});
const PipeArgs = z.object({
    name: z.string().min(1),
    plugin: z.string().min(1),
    payload: z.string(),
    args: z.string().optional(),
    configuration: z.string().optional(),
});
const ToolCall = z.object({
    id: z.string(),
    name: z.string(),
    arguments: z.string().default(""),
});
const CompletionArgs = z.object({
    request_id: z.string().default(""),
    messages: z.array(
        z.object({
            role: z.enum(["system", "user", "assistant", "tool"]),
            content: z.string().default(""),
            tool_calls: z.array(ToolCall).default([]),
            tool_call_id: z.string().nullish(),
        }).refine((message) =>
            message.role !== "tool" ||
            message.tool_call_id != null, "A tool message needs tool_call_id"),
    )
        .max(200),
    system: z.string().nullish(),
    model: z.string().nullish(),
    tools: z.array(z.json()).max(64).default([]),
    tool_choice: z.json().optional(),
    temperature: z.number().nullish(),
    max_tokens: z.number().int().min(0).max(4294967295).nullish(),
}).refine(
    (request) => request.messages.length > 0 || request.system != null,
    "A completion needs at least one message",
);
const CompletionReply = z.object({
    request_id: z.string(),
    content: z.string().default(""),
    tool_calls: z.array(ToolCall).default([]),
    finish_reason: z.string().default(""),
    usage: z.object({
        prompt_tokens: z.number().int().nonnegative().default(0),
        completion_tokens: z.number().int().nonnegative().default(0),
        total_tokens: z.number().int().nonnegative().default(0),
    }).nullable().default(null),
    model: z.string().default(""),
    error: z.string().default(""),
});
const Pane = z.object({
    id: z.number().int().nonnegative(),
    is_plugin: z.boolean(),
    title: z.string(),
    tab_id: z.number().int().nonnegative(),
    tab_name: z.string(),
    is_focused: z.boolean(),
    is_floating: z.boolean(),
    is_suppressed: z.boolean(),
    exited: z.boolean(),
    plugin_url: z.string().nullable(),
    pane_command: z.string().optional(),
    pane_cwd: z.string().optional(),
});
const Tab = z.object({
    tab_id: z.number().int().nonnegative(),
    position: z.number().int().nonnegative(),
    name: z.string(),
    active: z.boolean(),
});
const State = z.object({
    session: z.string(),
    panes: z.array(Pane),
    tabs: z.array(Tab),
});
const Result = z.object({
    session: z.string(),
    stdout: z.string(),
    stderr: z.string(),
    exitCode: z.number().int(),
});
type Context = {
    globalArgs: z.infer<typeof GlobalArgs>;
    signal: AbortSignal;
    logger: { info(message: string): void };
    writeResource: (
        spec: string,
        name: string,
        data: Record<string, unknown>,
    ) => Promise<{ name: string }>;
};

async function command(
    context: Omit<Context, "writeResource">,
    args: string[],
) {
    const { session, binary, timeoutMs } = GlobalArgs.parse(context.globalArgs);
    const signal = AbortSignal.any([
        context.signal,
        AbortSignal.timeout(timeoutMs),
    ]);
    signal.throwIfAborted();
    context.logger.info("Calling Yinz");
    const output = await new Deno.Command(binary, {
        args: ["--session", session, ...args],
        stdin: "null",
        stdout: "piped",
        stderr: "piped",
        signal,
    }).output();
    signal.throwIfAborted();
    const decoder = new TextDecoder();
    const stdout = decoder.decode(output.stdout);
    const stderr = decoder.decode(output.stderr);
    if (!output.success) {
        throw new Error(`Yinz ${args[0]} failed (${output.code}): ${stderr}`);
    }
    context.logger.info("Yinz command completed");
    return { session, stdout, stderr, exitCode: output.code };
}

async function completionDaemon(context: Omit<Context, "writeResource">) {
    const { session, binary, timeoutMs } = GlobalArgs.parse(context.globalArgs);
    const signal = AbortSignal.any([
        context.signal,
        AbortSignal.timeout(timeoutMs),
    ]);
    signal.throwIfAborted();
    const child = new Deno.Command(binary, {
        args: ["--session", session, "events", "--type", "session_update"],
        stdin: "null",
        stdout: "piped",
        stderr: "piped",
    }).spawn();
    let stopped = false;
    const stop = () => {
        if (stopped) return;
        stopped = true;
        try {
            child.kill("SIGKILL");
        } catch (error) {
            if (!(error instanceof Deno.errors.NotFound)) throw error;
        }
    };
    signal.addEventListener("abort", stop, { once: true });
    const stderr = new Response(child.stderr).text();
    const reader = child.stdout.getReader();
    try {
        signal.throwIfAborted();
        const decoder = new TextDecoder("utf-8", { fatal: true });
        let snapshot = "";
        while (!snapshot.includes("\n")) {
            const { value, done } = await reader.read();
            signal.throwIfAborted();
            if (done) {
                const status = await child.status;
                throw new Error(
                    `Yinz discovery ended (${status.code}): ${await stderr}`,
                );
            }
            snapshot += decoder.decode(value, { stream: true });
        }
        const event = z.object({
            event: z.literal("SessionUpdate"),
            payload: z.object({
                SessionUpdate: z.tuple([
                    z.array(z.object({
                        name: z.string(),
                        is_current_session: z.boolean(),
                        plugins: z.record(
                            z.string(),
                            z.object({ location: z.string() }),
                        ),
                    })),
                    z.array(z.unknown()),
                ]),
            }),
        }).parse(JSON.parse(snapshot.slice(0, snapshot.indexOf("\n"))));
        const sessions = event.payload.SessionUpdate[0].filter((candidate) =>
            candidate.name === session && candidate.is_current_session
        );
        if (sessions.length !== 1) {
            throw new Error("Expected exactly one current session");
        }
        const daemons = Object.entries(sessions[0].plugins).filter((
            [, plugin],
        ) => plugin.location === "yinz:hocus-agentd");
        if (daemons.length !== 1) {
            throw new Error("Expected exactly one hocus-agentd plugin");
        }
        const id = daemons[0][0];
        if (!/^(0|[1-9][0-9]*)$/.test(id) || Number(id) > 4294967295) {
            throw new Error("Invalid hocus-agentd plugin ID");
        }
        return `plugin_${id}`;
    } finally {
        signal.removeEventListener("abort", stop);
        stop();
        await reader.cancel();
        reader.releaseLock();
        const status = await child.status;
        const detail = await stderr;
        signal.throwIfAborted();
        if (!status.success && status.signal !== "SIGKILL") {
            throw new Error(
                `Yinz discovery failed (${status.code}): ${detail}`,
            );
        }
    }
}

/** Reusable CLI methods; action and pipe calls are not automatically retried. */
export const model = {
    type: "@hammz/yinz",
    version: "2026.09.06.1",
    globalArguments: GlobalArgs,
    checks: {
        sessionAvailable: {
            description:
                "Verify the named session responds before sending an action or pipe",
            labels: ["live"],
            appliesTo: ["action", "pipe"],
            execute: async (context: Omit<Context, "writeResource">) => {
                const result = await command(context, [
                    "action",
                    "list-tabs",
                    "--json",
                ]);
                z.array(Tab).parse(JSON.parse(result.stdout));
                return { pass: true };
            },
        },
    },
    resources: {
        ...resources,
        state: {
            description:
                "Session pane and tab snapshot (the two reads are not atomic)",
            schema: State,
            lifetime: "7d",
            garbageCollection: 10,
        },
        result: {
            description:
                "CLI output; success means CLI completion, not plugin business success",
            schema: Result,
            lifetime: "7d",
            garbageCollection: 10,
        },
        completion: {
            description:
                "One Hocus completion with its session and reported usage",
            schema: CompletionReply.extend({ session: z.string() }),
            lifetime: "7d",
            garbageCollection: 10,
        },
    },
    methods: {
        ...methods,
        hocus_complete: {
            description:
                "Request one Hocus model turn; the workflow runs any tools",
            arguments: CompletionArgs,
            execute: async (
                args: z.input<typeof CompletionArgs>,
                context: Context,
            ) => {
                const request = CompletionArgs.parse(args);
                const payload = JSON.stringify(request);
                if (new TextEncoder().encode(payload).length > 64000) {
                    throw new Error(
                        "A completion request is at most 64000 bytes",
                    );
                }
                const plugin = await completionDaemon(context);
                const result = await command(context, [
                    "pipe",
                    "--name",
                    "hocus_complete",
                    "--plugin",
                    plugin,
                    "--",
                    payload,
                ]);
                const reply = CompletionReply.parse(JSON.parse(result.stdout));
                if (reply.request_id !== request.request_id) {
                    throw new Error(
                        "Hocus completion request_id does not match",
                    );
                }
                if (reply.error !== "") throw new Error(reply.error);
                context.signal.throwIfAborted();
                return {
                    dataHandles: [
                        await context.writeResource(
                            "completion",
                            "completion",
                            { session: result.session, ...reply },
                        ),
                    ],
                };
            },
        },
        inspect: {
            description: "Read panes and tabs from the named session",
            arguments: z.object({}),
            execute: async (_args: Record<string, never>, context: Context) => {
                const panes = await command(context, [
                    "action",
                    "list-panes",
                    "--json",
                ]);
                const tabs = await command(context, [
                    "action",
                    "list-tabs",
                    "--json",
                ]);
                const state = State.parse({
                    session: context.globalArgs.session,
                    panes: JSON.parse(panes.stdout),
                    tabs: JSON.parse(tabs.stdout),
                });
                return {
                    dataHandles: [
                        await context.writeResource("state", "state", state),
                    ],
                };
            },
        },
        action: {
            description:
                "Run a Yinz action with literal argv; prefer explicit pane/tab IDs",
            arguments: ActionArgs,
            execute: async (
                args: z.infer<typeof ActionArgs>,
                context: Context,
            ) => {
                const parsed = ActionArgs.parse(args);
                const result = await command(context, [
                    "action",
                    parsed.action,
                    ...parsed.args,
                ]);
                return {
                    dataHandles: [
                        await context.writeResource("result", "action", result),
                    ],
                };
            },
        },
        pipe: {
            description:
                "Send one payload to a named plugin pipe and capture its reply",
            arguments: PipeArgs,
            execute: async (
                args: z.infer<typeof PipeArgs>,
                context: Context,
            ) => {
                const parsed = PipeArgs.parse(args);
                const argv = [
                    "pipe",
                    "--name",
                    parsed.name,
                    "--plugin",
                    parsed.plugin,
                ];
                if (parsed.args !== undefined) argv.push("--args", parsed.args);
                if (parsed.configuration !== undefined) {
                    argv.push("--plugin-configuration", parsed.configuration);
                }
                argv.push("--", parsed.payload);
                const result = await command(context, argv);
                return {
                    dataHandles: [
                        await context.writeResource("result", "pipe", result),
                    ],
                };
            },
        },
    },
};
