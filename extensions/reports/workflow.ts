/** Execution evidence and outstanding work from the actual workflow outputs. @module */
import { checked } from "../models/_lib/workflow.ts";
export const report = {
    name: "@hammz/workflow",
    description:
        "User choices, exact revisions, optional check results, and guarded local integration",
    scope: "workflow",
    labels: ["review", "local-integration"],
    execute: async (context: {
        repoDir: string;
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
        dataRepository: {
            getContent(
                type: string,
                id: string,
                name: string,
                version: number,
            ): Promise<Uint8Array | null>;
        };
    }) => {
        // A resumed invocation only receives its newly executed steps. Query the
        // saved run resources as well, so earlier review and gate evidence survives.
        const persisted = JSON.parse(
            await checked(
                { signal: AbortSignal.timeout(30000) },
                context.repoDir,
                [
                    "swamp",
                    "data",
                    "query",
                    "(has(tags.workflowRunId) && tags.workflowRunId == " +
                    JSON.stringify(context.workflowRunId) +
                    ' && has(tags.specName) && tags.specName != "claim") || (modelType == "@hammz/question" && has(attributes.run) && attributes.run == ' +
                    JSON.stringify(context.workflowRunId) + ")",
                    "--select",
                    '{"model":modelName,"type":modelType,"name":name,"version":version,"attributes":attributes}',
                    "--json",
                ],
            ),
        );
        if (!Array.isArray(persisted.results) || persisted.limited) {
            throw new Error("Incomplete saved workflow evidence");
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
                    attributes: raw
                        ? JSON.parse(new TextDecoder().decode(raw))
                        : null,
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
            resources: persisted.results,
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
            persisted.results.map((r: unknown) => "- " + quote(r)).join("\n") +
            "\n\nOutstanding steps: " + quote(json.outstanding);
        return { markdown, json };
    },
};
