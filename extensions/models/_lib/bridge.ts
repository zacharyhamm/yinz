/** Typed bridge transport. Replies describe host outcomes, not merely CLI completion. @module */
import { z } from "npm:zod@4.3.6";
import { checked, Context, Key, Literal } from "./workflow.ts";
import { Capture } from "./test_report.ts";
export const PromptText = Literal.refine(
    (s) => s.trim().length > 0,
    "Prompt must not be blank",
).refine(
    (s) => new TextEncoder().encode(s).length <= 262144,
    "Prompt exceeds 256 KiB (262144 UTF-8 bytes)",
);
// Leave room for the terminating NUL under Linux's per-argument limit.
export const STARTUP_PROMPT_BYTES = 131071;
export const StartupPromptText = PromptText.refine(
    (s) => new TextEncoder().encode(s).length <= STARTUP_PROMPT_BYTES,
    `Startup prompt exceeds ${STARTUP_PROMPT_BYTES} UTF-8 bytes`,
);
export function preparePrompt(prompt: string, maxBytes = 262144): string {
    const encoder = new TextEncoder();
    const bytes = encoder.encode(prompt);
    if (bytes.length <= maxBytes) return PromptText.parse(prompt);
    const limit = maxBytes === 262144 ? "256 KiB" : `${maxBytes} UTF-8 bytes`;
    const notice =
        `\n\n[Prompt truncated: middle omitted to fit ${limit}. Beginning and end excerpts retained; full evidence remains saved in Swamp.]\n\n`;
    const budget = maxBytes - encoder.encode(notice).length;
    let head = Math.ceil(budget / 2);
    let tail = bytes.length - Math.floor(budget / 2);
    // Move cuts past UTF-8 continuation bytes without splitting characters.
    while ((bytes[head] & 0xc0) === 0x80) head--;
    while ((bytes[tail] & 0xc0) === 0x80) tail++;
    const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
    return PromptText.parse(
        decoder.decode(bytes.subarray(0, head)) + notice +
            decoder.decode(bytes.subarray(tail)),
    );
}
export const Owner = z.object({
    host_id: z.string().min(1),
    session: z.string().min(1),
    incarnation: z.string().min(1),
    work: Key,
    agent: Key,
});
export const Operation = z.object({
    key: Key,
    work: Key,
    outcome: z.enum([
        "unknown",
        "pending",
        "completed",
        "failed",
        "interrupted",
        "cancelled",
        "process-exit",
    ]),
    incarnation: z.string(),
    dispatcher: z.string().optional(),
    transport: z.enum(["serve"]).optional(),
    requestId: z.string().optional(),
    correlation: z.string().optional(),
    run: z.string().optional(),
    error: z.string().optional(),
    request: z.string(),
    owner: Owner.optional(),
    pane: z.number().int().nonnegative().optional(),
    supervisor: z.string().optional(),
    remoteHost: z.string().optional(),
    remotePaneId: z.string().optional(),
    process: z.number().int().optional(),
    branch: z.string().nullable().optional(),
    checkout: z.string().nullable().optional(),
    target: z.union([
        z.literal("Local"),
        z.object({ Ssh: z.object({ host: z.string() }) }),
    ]).optional(),
    processExited: z.boolean().optional(),
    cancelRequested: z.boolean().optional(),
    terminationVerified: z.boolean().optional(),
    outstanding: z.string().optional(),
    accepted: z.boolean().optional(),
    choice: z.enum(["started", "cancelled"]).optional(),
    turn: z.string().optional(),
    ready: z.boolean().optional(),
    dispatched: z.boolean().optional(),
    message: z.string().nullable().optional(),
    hook: z.string().optional(),
    agentSession: z.string().nullable().optional(),
    exitCode: z.number().int().nullable().optional(),
    log: z.string().optional(),
    clean: z.boolean().optional(),
    output: z.string().optional(),
    outputTruncated: z.boolean().optional(),
    logError: z.string().optional(),
    capture: Capture.optional(),
});
export const Session = z.object({
    session: z.string().min(1),
    binary: Literal.min(1).default("yinz"),
    timeoutMs: z.number().int().positive().default(30000),
});
export const Request = z.object({
    work: Key,
    key: Key,
    session: Literal.min(1).optional(),
    binary: Literal.min(1).optional(),
});
export const AgentSpec = z.object({
    prompt: StartupPromptText.nullish().transform((s) => s ?? undefined),
    kind: z.enum(["", "codex", "claude", "cursor", "opencode", "muse"]).transform((s) =>
        s || undefined
    ).optional(),
    command: Literal.transform((s) => s || undefined).optional(),
    args: z.array(Literal).optional(),
    model: Literal.optional(),
    effort: Literal.optional(),
    branch: Literal.optional(),
    base: Literal.optional(),
    worktree: z.boolean().optional(),
    worktree_dir: Literal.optional(),
    source_directory: Literal.optional(),
    cwd: Literal.optional(),
    name: Literal.optional(),
    reuse: z.boolean().optional(),
    allow_outside_worktree: z.boolean().optional(),
    hooks: z.boolean().optional(),
});
// Select correlation fields explicitly: errors and payloads can contain secrets.
function timing(c: Pick<Context, "logger" | "modelId">, request: unknown) {
    let enabled = false;
    try {
        enabled = Deno.env.get("YINZ_SWAMP_TIMING") === "1";
    } catch { /* Restricted callers remain usable. */ }
    const start = performance.now();
    const attempt = enabled ? crypto.randomUUID() : undefined;
    const fields: Record<string, unknown> = {};
    if (request && typeof request === "object") {
        for (const key of ["method", "work", "key"]) {
            const value = (request as Record<string, unknown>)[key];
            if (typeof value === "string") fields[key] = value;
        }
    }
    return (phase: string, outcome: string, extra = {}) => {
        if (!enabled) return;
        try {
            const line = "swamp_timing " + JSON.stringify({
                layer: "typescript",
                modelId: c.modelId,
                ...fields,
                attempt,
                phase,
                outcome,
                elapsedMs: performance.now() - start,
                ...extra,
            });
            // A literal template segment avoids LogTape parsing JSON as placeholders
            // or quoting/escaping it as an interpolated property.
            c.logger.info(Object.assign([line], { raw: [line] }));
        } catch { /* Diagnostics must not alter delivery. */ }
    };
}
export async function bridge(
    c: Pick<
        Context,
        "signal" | "globalArgs" | "repoDir" | "logger" | "modelId"
    >,
    request: unknown,
) {
    const record = timing(c, request);
    let phase = "process";
    try {
        const global = Session.parse(c.globalArgs);
        const timeoutMs =
            (request as { method?: string }).method === "agent_spawn"
                ? Math.max(global.timeoutMs, 180000)
                : global.timeoutMs;
        const signal = AbortSignal.any([
            c.signal,
            AbortSignal.timeout(timeoutMs),
        ]);
        const stdout = await checked({ signal }, c.repoDir, [
            global.binary,
            "--session",
            global.session,
            "pipe",
            "--name",
            "swamp",
            "--plugin",
            "yinz:swamp-bridge",
        ], JSON.stringify(request) + "\n");
        record("process", "ok");
        phase = "parse";
        const parseStart = performance.now();
        const response = z.object({
            result: z.unknown().optional(),
            error: z.string().optional(),
        }).parse(JSON.parse(stdout));
        if (response.error) throw new Error(response.error);
        const correlation: Record<string, string> = {};
        if (response.result && typeof response.result === "object") {
            for (const field of ["requestId", "run"]) {
                const value =
                    (response.result as Record<string, unknown>)[field];
                if (typeof value === "string") correlation[field] = value;
            }
        }
        record("parse", "ok", {
            parseMs: performance.now() - parseStart,
            ...correlation,
        });
        return response.result;
    } catch (error) {
        record(phase, "error");
        throw error;
    }
}
export async function operation(c: Context, request: Record<string, unknown>) {
    // Fresh ad-hoc models reached after resume can lack global arguments.
    // Carry the declared session with the operation instead of using ambient focus.
    if (request.session) {
        c = {
            ...c,
            globalArgs: {
                ...c.globalArgs,
                session: request.session,
                binary: request.binary ?? c.globalArgs.binary,
            },
        };
    }
    const startupPrompt = request.method === "agent_spawn"
        ? (request.spec as z.infer<typeof AgentSpec> | undefined)?.prompt
        : undefined;
    if (request.method === "agent_prompt" || startupPrompt != null) {
        const prompt = request.method === "agent_spawn"
            ? StartupPromptText.parse(startupPrompt)
            : PromptText.parse(request.prompt);
        const hash = await crypto.subtle.digest(
            "SHA-256",
            new TextEncoder().encode(prompt),
        );
        request = {
            ...request,
            prompt_sha256: Array.from(
                new Uint8Array(hash),
                (b) => b.toString(16).padStart(2, "0"),
            ).join(""),
        };
    }
    return Operation.parse(await bridge(c, request));
}
export async function observe(
    c: Context,
    args: z.infer<typeof Request>,
    timeoutSeconds: number,
    includeOutput = true,
) {
    const record = timing(c, { method: "observe", ...args });
    let polls = 0, requestMs = 0, waitMs = 0;
    let outcome = "error";
    const poll = async (includeOutput: boolean) => {
        const start = performance.now();
        polls++;
        try {
            return await operation(c, {
                method: "observe",
                ...args,
                ...(includeOutput ? {} : { includeOutput: false }),
            });
        } finally {
            requestMs += performance.now() - start;
        }
    };
    try {
        const deadline = Date.now() + timeoutSeconds * 1000;
        while (true) {
            const result = await poll(false);
            const request = result.outcome === "unknown"
                ? JSON.parse(result.request)
                : null;
            const awaitingPicker = request?.method === "pane_action" &&
                ["fork", "duplicate"].includes(request.action);
            if (result.outcome !== "pending" && !awaitingPicker) {
                const final = includeOutput ? await poll(true) : result;
                outcome = final.outcome;
                return final;
            }
            if (Date.now() >= deadline) {
                throw new Error(
                    "Observation deadline reached; the operation remains attached and must not be dispatched again",
                );
            }
            const waitStart = performance.now();
            try {
                await new Promise<void>((resolve, reject) => {
                    const stop = () => {
                        clearTimeout(timer);
                        reject(c.signal.reason);
                    };
                    const timer = setTimeout(() => {
                        c.signal.removeEventListener("abort", stop);
                        resolve();
                    }, 500);
                    c.signal.addEventListener("abort", stop, { once: true });
                    if (c.signal.aborted) stop();
                });
            } finally {
                waitMs += performance.now() - waitStart;
            }
        }
    } finally {
        record("observation", outcome, { polls, requestMs, waitMs });
    }
}
