export const launchSupport = {
  outcome: "ok",
  output: "agent-launch-transaction-v2",
  server_instance: "server-test",
};
export function receipt(
  launchId: string,
  overrides: Record<string, unknown> = {},
) {
  return {
    version: 2,
    launch_id: launchId,
    request_digest: "a".repeat(64),
    server_instance: "server-test",
    outcome: "started",
    phase: "finished",
    reason: null,
    process: "running",
    resources: { cleanup_pending: false },
    disposition: "available",
    ...overrides,
  };
}
export function launchReply(
  args: string[],
  input: Record<string, unknown>,
): Record<string, unknown> {
  if (input.launch || args[3] !== "spawn") return input;
  const id = args[args.indexOf("--launch-id") + 1];
  if (input.outcome === "ok" && input.spawned) {
    return { ...input, launch: receipt(id) };
  }
  if (
    input.outcome === "rejected" &&
    Object.entries(input).every(([key, value]) =>
      ["outcome", "message"].includes(key) || value == null ||
      (Array.isArray(value) && value.length === 0)
    )
  ) {
    return {
      ...input,
      launch: receipt(id, {
        outcome: "rejected",
        process: "not_started",
        disposition: "closed",
        reason: "invalid_request",
      }),
    };
  }
  return input;
}
