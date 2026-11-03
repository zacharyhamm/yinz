/** A shared Swamp instance queues integration calls; Git also has a common OS lock. */
import { z } from "npm:zod@4.6.7";

const Arguments = z.object({
  issueName: z.string().regex(/^issue-[a-z0-9_-]+$/).max(64),
  skipCiRequest: z.string().trim().default(""),
}).strict();

/** One runtime instance, yinz-main-merge, is shared by all issue workflows. */
export const behavior = {
  type: "@yinz/merge-coordinator",
  version: "2026.11.03.1",
  globalArguments: z.object({}).strict(),
  methods: {
    integrate: {
      kind: "action",
      description:
        "Serialize issue integration while the called lifecycle records its own evidence",
      arguments: Arguments,
      execute: async (args: z.infer<typeof Arguments>, context: {
        logger: {
          info: (message: string, values: Record<string, unknown>) => void;
        };
        runModel: (request: {
          definition: string;
          method: string;
          arguments: { skipCiRequest: string };
        }) => Promise<{ ok: true } | { ok: false; error: { message: string } }>;
      }) => {
        context.logger.info("Integrating {issue} into local main", {
          issue: args.issueName,
        });
        const result = await context.runModel({
          definition: args.issueName,
          method: "integrate",
          arguments: { skipCiRequest: args.skipCiRequest },
        });
        if (!result.ok) throw new Error(result.error.message);
        context.logger.info("Integration completed for {issue}", {
          issue: args.issueName,
        });
        return { dataHandles: [] };
      },
    },
  },
};
