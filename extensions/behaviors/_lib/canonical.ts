/** Canonical JSON and its digest, shared by fencing, evidence and compaction. */
import { createHash } from "node:crypto";

/** Sort object keys only; array ordering belongs to the submitted evidence. */
export function canonical(value: unknown): string {
  return JSON.stringify(
    value,
    (_key, item) =>
      item !== null && typeof item === "object" && !Array.isArray(item)
        ? Object.fromEntries(
          Object.entries(item).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0),
        )
        : item,
  );
}

export function evidenceDigest(value: unknown): string {
  return createHash("sha256").update(canonical(value)).digest("hex");
}

export function sha256Hex(bytes: Uint8Array | string): string {
  return createHash("sha256").update(bytes).digest("hex");
}
