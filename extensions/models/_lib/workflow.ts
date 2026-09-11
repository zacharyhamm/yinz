/** Shared, literal command and durable dispatch helpers. @module */
import { z } from "npm:zod@4.3.6";

export const Key = z.string().regex(/^[A-Za-z0-9][A-Za-z0-9_.:-]{0,199}$/);
export const Literal = z.string().refine(
    (s) => !s.includes("\0"),
    "NUL is not allowed",
);
export type Context = {
    modelType?: { normalized: string };
    modelId?: string;
    dataRepository?: import("./evidence.ts").DataRepository;
    signal: AbortSignal;
    repoDir: string;
    globalArgs: Record<string, unknown>;
    logger: {
        info(message: string | TemplateStringsArray): void;
    };
    readResource(name: string): Promise<Record<string, unknown> | null>;
    writeResource(
        spec: string,
        name: string,
        value: Record<string, unknown>,
    ): Promise<{ name: string; version?: number }>;
};
export async function exec(
    c: Pick<Context, "signal">,
    cwd: string,
    args: string[],
    stdin?: string,
) {
    c.signal.throwIfAborted();
    args.forEach((arg) => Literal.parse(arg));
    const command = new Deno.Command(args[0], {
        args: args.slice(1),
        cwd,
        stdin: stdin === undefined ? "null" : "piped",
        stdout: "piped",
        stderr: "piped",
        env: { GIT_TERMINAL_PROMPT: "0", GIT_EDITOR: "true", GIT_PAGER: "cat" },
        signal: c.signal,
    });
    let result: Deno.CommandOutput;
    if (stdin === undefined) {
        result = await command.output();
    } else {
        const child = command.spawn();
        const writer = child.stdin.getWriter();
        const [output, input] = await Promise.allSettled([
            child.output(),
            (async () => {
                try {
                    await writer.write(new TextEncoder().encode(stdin));
                } finally {
                    await writer.close();
                }
            })(),
        ]);
        c.signal.throwIfAborted();
        if (output.status === "rejected") throw output.reason;
        result = output.value;
        // Preserve the child's diagnostic if it exits before consuming stdin.
        if (input.status === "rejected" && result.success) throw input.reason;
    }
    c.signal.throwIfAborted();
    const decoder = new TextDecoder("utf-8", { fatal: true });
    return {
        exitCode: result.code,
        stdout: decoder.decode(result.stdout),
        stderr: decoder.decode(result.stderr),
    };
}
export async function checked(
    c: Pick<Context, "signal">,
    cwd: string,
    args: string[],
    stdin?: string,
) {
    const r = await exec(c, cwd, args, stdin);
    if (r.exitCode !== 0) {
        throw new Error(
            `${args[0]} ${args[1]} failed (${r.exitCode}): ${r.stderr}`,
        );
    }
    return r.stdout.replace(/\n$/, "");
}
export async function digest(value: unknown) {
    const bytes = new TextEncoder().encode(JSON.stringify(value));
    return Array.from(
        new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)),
        (b) => b.toString(16).padStart(2, "0"),
    ).join("");
}
export const Claim = z.object({
    key: Key,
    fingerprint: z.string(),
    outcome: z.enum(["unknown", "completed", "failed"]),
    result: z.string().nullable(),
});
/** Write before dispatch. A lost reply is never permission to repeat a mutation. */
export async function claimed(
    c: Context,
    key: string,
    args: unknown,
    action: () => Promise<unknown>,
) {
    Key.parse(key);
    const name = `claim-${key}`;
    const fingerprint = await digest(args);
    const previous = await c.readResource(name);
    if (previous) {
        const claim = Claim.parse(previous);
        if (claim.fingerprint !== fingerprint) {
            throw new Error(`Conflicting request for ${key}`);
        }
        if (claim.outcome !== "completed") {
            throw new Error(
                `Operation ${key} is ${claim.outcome}; inspect and reconcile before proceeding`,
            );
        }
        return JSON.parse(claim.result!);
    }
    await c.writeResource(
        "claim",
        name,
        Claim.parse({ key, fingerprint, outcome: "unknown", result: null }),
    );
    const result = await action();
    await c.writeResource(
        "claim",
        name,
        Claim.parse({
            key,
            fingerprint,
            outcome: "completed",
            result: JSON.stringify(result),
        }),
    );
    return result;
}
export const claimResource = {
    description:
        "Durable dispatch claim; unknown mutations require reconciliation",
    schema: Claim,
    lifetime: "infinite",
    garbageCollection: 1,
};
