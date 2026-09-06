/** Control an existing Zellij session through its CLI. @module */
import { z } from "npm:zod@4.3.6";
import { methods, resources } from "./_lib/zellij_methods.ts";

const GlobalArgs = z.object({
    session: z.string().min(1),
    binary: z.string().min(1).default("zellij"),
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
    context.logger.info("Calling Zellij");
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
        throw new Error(`Zellij ${args[0]} failed (${output.code}): ${stderr}`);
    }
    context.logger.info("Zellij command completed");
    return { session, stdout, stderr, exitCode: output.code };
}

/** Reusable CLI methods; action and pipe calls are not automatically retried. */
export const model = {
    type: "@hammz/zellij",
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
    },
    methods: {
        ...methods,
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
                "Run a Zellij action with literal argv; prefer explicit pane/tab IDs",
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
