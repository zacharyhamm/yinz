import { z } from "npm:zod@4.6.7";

export const AgentKindSchema = z.enum(["claude", "codex", "muse", "pi", "agy"]);

/** Resolved once at start; absent model/effort select provider defaults. */
export const WorkerSelectionSchema = z.object({
  kind: AgentKindSchema,
  model: z.string().trim().min(1).optional(),
  effort: z.string().trim().min(1).optional(),
}).strict();
