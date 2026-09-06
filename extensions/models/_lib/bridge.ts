/** Typed bridge transport. Replies describe host outcomes, not merely CLI completion. @module */
import { z } from "npm:zod@4.3.6";
import { checked, Context, Key, Literal } from "./workflow.ts";
export const PromptText = z.string().min(1).refine(
    (s) => new TextEncoder().encode(s).length <= 262144,
    "Prompt exceeds 256 KiB (262144 UTF-8 bytes)",
);
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
        "process-exit",
    ]),
    incarnation: z.string(),
    dispatcher: z.string().optional(),
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
        z.object({
            Firecracker: z.object({
                vm: z.string(),
                image: z.string().nullable().optional(),
                worktree: z.string().nullable().optional(),
            }),
        }),
    ]).optional(),
    processExited: z.boolean().optional(),
    outstanding: z.string().optional(),
    accepted: z.boolean().optional(),
    turn: z.string().optional(),
    ready: z.boolean().optional(),
    dispatched: z.boolean().optional(),
    message: z.string().nullable().optional(),
    hook: z.string().optional(),
    agentSession: z.string().nullable().optional(),
    exitCode: z.number().int().nullable().optional(),
    log: z.string().optional(),
    vm: z.string().nullable().optional(),
    clean: z.boolean().optional(),
    output: z.string().optional(),
    outputTruncated: z.boolean().optional(),
    logError: z.string().optional(),
});
export const Session = z.object({
    session: z.string().min(1),
    binary: Literal.min(1).default("zellij"),
    timeoutMs: z.number().int().positive().default(30000),
});
export const Request = z.object({ work: Key, key: Key });
export const AgentSpec = z.object({
    kind: z.enum(["codex", "claude", "cursor"]).optional(),
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
    firecracker: Literal.transform((s) => s || undefined).optional(),
    firecracker_image: Literal.transform((s) => s || undefined).optional(),
    automatic_firecracker_vm: z.boolean().optional(),
    reuse: z.boolean().optional(),
    allow_outside_worktree: z.boolean().optional(),
    hooks: z.boolean().optional(),
});
export async function bridge(
    c: Pick<Context, "signal" | "globalArgs" | "repoDir">,
    request: unknown,
) {
    const global = Session.parse(c.globalArgs);
    const timeoutMs = (request as { method?: string }).method === "agent_spawn"
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
        "zellij:swamp-bridge",
    ], JSON.stringify(request) + "\n");
    const response = z.object({
        result: z.unknown().optional(),
        error: z.string().optional(),
    }).parse(JSON.parse(stdout));
    if (response.error) throw new Error(response.error);
    return response.result;
}
export async function operation(c: Context, request: Record<string, unknown>) {
    if (request.method === "agent_prompt") {
        const prompt = PromptText.parse(request.prompt);
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
) {
    const deadline = Date.now() + timeoutSeconds * 1000;
    while (true) {
        const result = await operation(c, { method: "observe", ...args });
        if (result.outcome !== "pending") return result;
        if (Date.now() >= deadline) {
            throw new Error(
                "Observation deadline reached; the operation remains attached and must not be dispatched again",
            );
        }
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
    }
}
