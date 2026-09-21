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
            const model = data.tags.modelName ||
                (await context.definitionRepository.findById(
                    modelType,
                    modelId,
                ))?.name ||
                modelId;
            resources.push({
                model,
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
        const testReports = [
            ...new Set(
                [...resources, ...steps.flatMap((s) => s.outputs)]
                    .flatMap((r) => {
                        const test = r.attributes?.test ?? r.attributes;
                        return typeof test?.summary === "string"
                            ? [test.summary as string]
                            : [];
                    }),
            ),
        ];
        // Test evidence stores the raw log. Expose its readable report once, not
        // again in each step, resource and resumed assessment (or bridge replies
        // grow past their transport limit).
        for (
            const record of [
                ...resources,
                ...steps.flatMap((s) => s.outputs),
            ]
        ) {
            const test = record.attributes?.test ?? record.attributes;
            if (typeof test?.summary !== "string") continue;
            test.reportIndex = testReports.indexOf(test.summary);
            delete test.summary;
            delete test.output;
        }
        const json = {
            run: context.workflowRunId,
            workflow: context.workflowName,
            status: context.workflowStatus,
            steps,
            resources,
            testReports,
            outstanding: steps.filter((s) =>
                s.status !== "succeeded" && s.status !== "skipped"
            ).map((s) => s.step),
        };
        const quote = (value: unknown) =>
            JSON.stringify(value).replace(
                /[<>&]/g,
                (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;" }[c]!),
            ).replace(/[`*_\[\]]/g, "\\$&");
        const latest = new Map(
            [...resources]
                .sort((a, b) => a.version - b.version)
                .map((r) => [r.name, r.attributes]),
        );
        const checks = latest.get("checks-current");
        const integration = latest.get("integrationDecision-current");
        const cleanup = latest.get("cleanupDecision-current");
        const integrated = integration?.integrated ||
            context.inputs?.phase === "cleanup";
        const decisionPhase = !context.inputs?.phase ||
            ["prepare", "confirm"].includes(String(context.inputs.phase));
        const state = context.inputs?.state;
        const resultEvidence = integration?.evidence ??
            latest.get("rebaseResult-current")?.evidence;
        const evidence = resultEvidence ?? checks?.evidence ??
            latest.get("evidence-current") ?? await resolveEvidence(
                context.dataRepository,
                state && typeof state === "object" && "evidence" in state
                    ? state.evidence
                    : null,
            );
        const summary = [
            ...(evidence
                ? [
                    "Branch: " + quote(evidence.source.branch) + " → " +
                    quote(evidence.target.branch),
                    (decisionPhase || resultEvidence || integrated
                        ? "Source revision: "
                        : "Source revision before action: ") +
                    quote(evidence.source.commit),
                    "Target revision: " + quote(evidence.target.commit),
                ]
                : []),
            ...(context.inputs?.checkout
                ? ["Worktree: " + quote(context.inputs.checkout)]
                : []),
            ...(checks && decisionPhase
                ? [
                    "Review: " + checks.codeReview + ". Tests: " +
                    checks.tests + ".",
                ]
                : []),
            ...(integrated
                ? ["Integration completed locally. No commits were pushed."]
                : []),
            ...(cleanup
                ? [
                    cleanup.removed
                        ? "Clean worktree removed. Branch and VM resources were not deleted."
                        : "Worktree removal was refused: " +
                            quote(cleanup.reason),
                ]
                : context.inputs?.phase === "cleanup" &&
                        context.inputs?.answer === "keep"
                ? ["Worktree, branch and VM resources kept."]
                : []),
            ...(!integrated && decisionPhase && checks?.recommendation
                ? ["Next: " + quote(checks.recommendation)]
                : []),
        ];
        const markdown = "# Workflow result\n\n" + summary.join("\n\n") +
            "\n\nRun: " +
            context.workflowRunId + "\n\nStatus: " + context.workflowStatus +
            "\n\n" + testReports.map((summary) => {
                let length = 3;
                for (const match of summary.matchAll(/`+/g)) {
                    length = Math.max(length, match[0].length + 1);
                }
                const fence = "`".repeat(length);
                return fence + "text\n" + summary + "\n" + fence + "\n\n";
            }).join("") +
            steps.map((s) =>
                "- " + quote(s.job + "/" + s.step) + ": " + s.status + "\n" +
                s.outputs.map((o) =>
                    "  - " + quote(o.name) + " version " + o.version + ": " +
                    quote(o.attributes)
                ).join("\n")
            ).join("\n") + "\n\nSaved run resources:\n" +
            resources.map((r: unknown) => "- " + quote(r)).join("\n") +
            "\n\nOutstanding steps: " + quote(json.outstanding);
        return { markdown, json: { ...json, summary } };
    },
};
