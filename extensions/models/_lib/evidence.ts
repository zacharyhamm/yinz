/** Exact saved evidence, resolved inside Swamp rather than through the bridge. @module */
import { z } from "npm:zod@4.3.6";
export const OutputRef = z.object({
    modelType: z.string().min(1),
    modelId: z.string().min(1),
    name: z.string().min(1),
    version: z.number().int().positive(),
});
/** At most 16 KiB of UTF-8, including the notice, without splitting characters. */
export function outputPreview(output: string) {
    const encoder = new TextEncoder();
    const bytes = encoder.encode(output);
    if (bytes.length <= 16384) return output;
    const notice =
        "\n[Output truncated; full output saved in Swamp at outputRef]\n";
    const budget = 16384 - encoder.encode(notice).length;
    let head = Math.ceil(budget / 2);
    let tail = bytes.length - Math.floor(budget / 2);
    while ((bytes[head] & 0xc0) === 0x80) head--;
    while ((bytes[tail] & 0xc0) === 0x80) tail++;
    const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
    return decoder.decode(bytes.subarray(0, head)) + notice +
        decoder.decode(bytes.subarray(tail));
}
export type DataRepository = {
    findAllGlobal(): Promise<
        Array<{
            modelType: { normalized: string };
            modelId: string;
            data: {
                name: string;
                version: number;
                tags: Record<string, string>;
            };
        }>
    >;
    getContent(
        type: string,
        id: string,
        name: string,
        version: number,
    ): Promise<Uint8Array | null>;
};
export async function readEvidence(
    repo: DataRepository,
    ref: z.infer<typeof OutputRef>,
) {
    const bytes = await repo.getContent(
        ref.modelType,
        ref.modelId,
        ref.name,
        ref.version,
    );
    return bytes ? JSON.parse(new TextDecoder().decode(bytes)) : null;
}
/** Expand saved output references; missing content stays explicitly unavailable. */
export async function resolveEvidence(
    repo: DataRepository,
    value: unknown,
): Promise<any> {
    if (Array.isArray(value)) {
        return Promise.all(value.map((v) => resolveEvidence(repo, v)));
    }
    if (!value || typeof value !== "object") return value;
    const result: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value)) {
        result[key] = await resolveEvidence(repo, item);
    }
    if (result.outputRef) {
        const ref = OutputRef.parse(result.outputRef);
        const saved = await readEvidence(repo, ref);
        result.output = saved?.output ?? null;
    }
    return result;
}
