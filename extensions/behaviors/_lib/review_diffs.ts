/**
 * Review diff bodies, stored once per content digest outside every persisted
 * record. Baselines keep only a `DiffRef`; workers receive the bytes through
 * their private assignment files.
 */
import { Buffer } from "node:buffer";
import { sha256Hex } from "./canonical.ts";
import {
  type DiffRef,
  GitBaseline,
  isInlineBaseline,
} from "./review_continuity.ts";

/** The issue behavior's `files` spec that holds diff bodies. */
export const DIFF_FILE_SPEC = "review-diff";
export const ISSUE_BEHAVIOR_TYPE = "@yinz/issue-lifecycle";
export function diffDataName(sha256: string): string {
  return `review-diff-${sha256}`;
}

type Encoding = DiffRef["encoding"];
const encoder = new TextEncoder();

/** Decoded diff bytes from the Git helper's inline text. */
export function decodeDiff(text: string, encoding?: Encoding): Uint8Array {
  return encoding === "base64"
    ? new Uint8Array(Buffer.from(text, "base64"))
    : encoder.encode(text);
}

/** The Git helper's inline text for decoded diff bytes. */
export function encodeDiff(bytes: Uint8Array, encoding: Encoding): string {
  return encoding === "base64"
    ? Buffer.from(bytes).toString("base64")
    : new TextDecoder("utf-8", { fatal: true }).decode(bytes);
}

export interface DiffStore {
  /** Stores the bytes once under their sha256 and returns that digest. */
  put(bytes: Uint8Array): Promise<string>;
  /** The stored bytes, verified against the reference's digest and length. */
  get(ref: Pick<DiffRef, "sha256" | "bytes">): Promise<Uint8Array>;
}

export type DiffContext = {
  modelId: string;
  createFileWriter: (spec: string, name: string) => {
    writeStream: (stream: ReadableStream<Uint8Array>) => Promise<unknown>;
  };
  dataRepository: {
    getContent: (
      type: string,
      name: string,
      resource: string,
    ) => Promise<Uint8Array | null>;
  };
};

function verified(
  bytes: Uint8Array | null | undefined,
  ref: Pick<DiffRef, "sha256" | "bytes">,
): Uint8Array {
  if (!bytes) {
    throw new Error(
      `Review diff ${ref.sha256} is missing from the issue's Swamp data (${
        diffDataName(ref.sha256)
      })`,
    );
  }
  if (bytes.length !== ref.bytes || sha256Hex(bytes) !== ref.sha256) {
    throw new Error(
      `Review diff ${ref.sha256} does not match its recorded digest and length`,
    );
  }
  return bytes;
}

/** Diffs as the issue behavior's own Swamp files, read back by digest. */
export function swampDiffStore(context: DiffContext): DiffStore {
  // Within one invocation each digest is read or written at most once.
  const stored = new Set<string>();
  const read = (sha256: string) =>
    context.dataRepository.getContent(
      ISSUE_BEHAVIOR_TYPE,
      context.modelId,
      diffDataName(sha256),
    );
  return {
    async put(bytes) {
      const sha256 = sha256Hex(bytes);
      if (stored.has(sha256)) return sha256;
      const existing = await read(sha256);
      // Every reader verifies the digest and length, so a damaged copy fails
      // closed where it is used rather than here.
      if (!existing || sha256Hex(existing) !== sha256) {
        await context.createFileWriter(DIFF_FILE_SPEC, diffDataName(sha256))
          .writeStream(ReadableStream.from([bytes]));
      }
      stored.add(sha256);
      return sha256;
    },
    async get(ref) {
      return verified(await read(ref.sha256), ref);
    },
  };
}

/** An in-memory store for tests and fixtures. */
export function memoryDiffStore(
  files = new Map<string, Uint8Array>(),
): DiffStore & { files: Map<string, Uint8Array> } {
  return {
    files,
    put(bytes) {
      const sha256 = sha256Hex(bytes);
      files.set(sha256, bytes.slice());
      return Promise.resolve(sha256);
    },
    get(ref) {
      return Promise.resolve(verified(files.get(ref.sha256), ref));
    },
  };
}

/**
 * Names bodies by digest without storing them, to recompute the compact form of
 * a record (such as an assignment bound before compaction) without writing.
 */
export function digestDiffStore(): DiffStore {
  return {
    put: (bytes) => Promise.resolve(sha256Hex(bytes)),
    get: (ref) =>
      Promise.reject(
        new Error(`Review diff ${ref.sha256} is not readable here`),
      ),
  };
}

async function reference(
  store: DiffStore,
  text: string,
  encoding: Encoding | undefined,
): Promise<DiffRef> {
  const bytes = decodeDiff(text, encoding);
  const sourceEncoding = encoding ?? "utf8";
  // Compaction must be lossless: the reference reproduces the exact inline text.
  if (encodeDiff(bytes, sourceEncoding) !== text) {
    throw new Error("Review diff does not round-trip through its encoding");
  }
  const sha256 = await store.put(bytes);
  return { sha256, bytes: bytes.length, encoding: sourceEncoding };
}

/**
 * The baseline with its diff bodies stored out of line. A baseline that is
 * already by reference is returned unchanged.
 */
export async function compactBaseline(
  store: DiffStore,
  baseline: GitBaseline,
): Promise<GitBaseline> {
  if (!isInlineBaseline(baseline)) return baseline;
  const {
    featureDiff,
    featureDiffEncoding,
    delta,
    deltaEncoding,
    ...rest
  } = baseline;
  return GitBaseline.parse({
    ...rest,
    featureDiffRef: await reference(store, featureDiff!, featureDiffEncoding),
    ...(delta === undefined
      ? {}
      : { deltaRef: await reference(store, delta, deltaEncoding) }),
  });
}

/** The decoded feature diff and endpoint delta a worker reads. */
export async function baselineBytes(
  store: DiffStore | undefined,
  baseline: GitBaseline,
): Promise<{ featureDiff: Uint8Array; delta?: Uint8Array }> {
  if (isInlineBaseline(baseline)) {
    return {
      featureDiff: decodeDiff(
        baseline.featureDiff!,
        baseline.featureDiffEncoding,
      ),
      ...(baseline.delta === undefined
        ? {}
        : { delta: decodeDiff(baseline.delta, baseline.deltaEncoding) }),
    };
  }
  if (!store) throw new Error("Referenced review diffs need a diff store");
  return {
    featureDiff: await store.get(baseline.featureDiffRef!),
    ...(baseline.deltaRef ? { delta: await store.get(baseline.deltaRef) } : {}),
  };
}
