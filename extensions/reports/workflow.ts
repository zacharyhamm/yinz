/** Execution evidence and outstanding work from the actual workflow outputs. @module */
import { DataRepository, resolveEvidence } from "../models/_lib/evidence.ts";
export const report = {
    name: "@hammz/workflow",
    description:
        "User choices, exact revisions, optional check results, and guarded local integration",
    scope: "workflow",
    labels: ["review", "local-integration"],
    execute: async (context: {
        repoDir: string;
        inputs?: Record<string, unknown>;
        workflowRunId: string;
        workflowName: string;
        workflowStatus: string;
        stepExecutions: Array<
            {
                jobName: string;
                stepName: string;
                status: string;
                modelType: string;
                modelId: string;
                dataHandles: Array<
                    { name: string; version: number; specName?: string }
                >;
            }
        >;
        dataRepository: DataRepository;
        definitionRepository: {
            findById(
                type: { normalized: string },
                id: string,
            ): Promise<{ name: string } | null>;
        };
    }) => {
        // Resumed invocations only contain newly executed steps. Include persisted
        // resources from earlier invocations, without requiring a live session.
        const resources = [];
        for (
            const { data, modelType, modelId } of await context.dataRepository
                .findAllGlobal()
        ) {
            if (data.tags.type === "report") continue;
            const tagged = data.tags.workflowRunId === context.workflowRunId &&
                !!data.tags.specName && data.tags.specName !== "claim";
            if (!tagged && modelType.normalized !== "@hammz/question") continue;
            const bytes = await context.dataRepository.getContent(
                modelType.normalized,
                modelId,
                data.name,
                data.version,
            );
            const attributes = bytes
                ? JSON.parse(new TextDecoder().decode(bytes))
                : null;
            if (!tagged && attributes?.run !== context.workflowRunId) continue;
            const definition = await context.definitionRepository.findById(
                modelType,
                modelId,
            );
            resources.push({
                model: definition?.name ?? modelId,
                type: modelType.normalized,
                name: data.name,
                version: data.version,
                attributes: await resolveEvidence(
                    context.dataRepository,
                    attributes,
                ),
            });
        }
        const steps = [];
        for (const step of context.stepExecutions) {
            const outputs = [];
            for (const handle of step.dataHandles) {
                if (
                    step.modelType !== "@hammz/question" && ![
                        "operation",
                        "review",
                        "test",
                        "assessment",
                        "checks",
                        "diff",
                        "checkoutSync",
                        "evidence",
                        "integration",
                        "integrationDecision",
                        "question",
                        "rebaseResult",
                    ].includes(
                        handle.specName ?? handle.name.replace(/-current$/, ""),
                    )
                ) continue;
                const raw = await context.dataRepository.getContent(
                    step.modelType,
                    step.modelId,
                    handle.name,
                    handle.version,
                );
                outputs.push({
                    name: handle.name,
                    version: handle.version,
                    attributes: await resolveEvidence(
                        context.dataRepository,
                        raw ? JSON.parse(new TextDecoder().decode(raw)) : null,
                    ),
                });
            }
            steps.push({
                job: step.jobName,
                step: step.stepName,
                status: step.status,
                outputs,
            });
        }
        const json = {
            run: context.workflowRunId,
            workflow: context.workflowName,
            status: context.workflowStatus,
            steps,
            resources,
            outstanding: steps.filter((s) =>
                s.status !== "succeeded" && s.status !== "skipped"
            ).map((s) => s.step),
        };
        const quote = (value: unknown) =>
            JSON.stringify(value).replace(
                /[<>&]/g,
                (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;" }[c]!),
            ).replace(/[`*_\[\]]/g, "\\$&");
        const markdown = "# Workflow evidence\n\nRun: " +
            context.workflowRunId + "\n\nStatus: " + context.workflowStatus +
            "\n\n" +
            steps.map((s) =>
                "- " + quote(s.job + "/" + s.step) + ": " + s.status + "\n" +
                s.outputs.map((o) =>
                    "  - " + quote(o.name) + " version " + o.version + ": " +
                    quote(o.attributes)
                ).join("\n")
            ).join("\n") + "\n\nSaved run resources:\n" +
            resources.map((r: unknown) => "- " + quote(r)).join("\n") +
            "\n\nOutstanding steps: " + quote(json.outstanding);
        return { markdown, json };
    },
};
