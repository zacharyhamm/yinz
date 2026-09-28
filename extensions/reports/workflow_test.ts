import { report } from "./workflow.ts";

Deno.test("reports need no session or subprocess and retain resumed, exact and missing evidence", async () => {
    const original = Deno.Command;
    Deno.Command = class extends original {
        constructor() {
            super("forbidden");
            throw new Error("Report launched a subprocess");
        }
    };
    const large = "begin" + "x".repeat(3 * 1024 * 1024) + "end";
    const rows = [
        ["@hammz/question", "question-model", "report-summary", 1, {
            type: "report",
        }, "# Markdown report"],
        ["@hammz/yinz", "earlier", "review-current", 2, {
            workflowRunId: "run",
            specName: "review",
            modelName: "earlier-name",
        }, { findings: [] }],
        ["@hammz/yinz", "earlier", "test-current", 1, {
            workflowRunId: "previous",
            specName: "test",
        }, { output: large }],
        ["@hammz/yinz", "earlier", "test-current", 2, {
            workflowRunId: "other",
            specName: "test",
        }, { output: "newer" }],
        ["@hammz/yinz", "current", "checks-current", 1, {
            workflowRunId: "run",
            specName: "checks",
        }, {
            evidence: {
                source: { branch: "topic", commit: "abc" },
                target: { branch: "main", commit: "def" },
            },
            codeReview: "current",
            tests: "failed",
            recommendation: "Fix test failures, then commit and rerun checks.",
            test: {
                output: "preview",
                outputRef: {
                    modelType: "@hammz/yinz",
                    modelId: "earlier",
                    name: "test-current",
                    version: 1,
                },
            },
        }],
        ["@hammz/yinz", "current", "missing", 1, {
            workflowRunId: "run",
            specName: "test",
        }, null],
        ["@hammz/yinz", "current", "claim", 1, {
            workflowRunId: "run",
            specName: "claim",
        }, {}],
        ["@hammz/question", "question-model", "decision", 3, {}, {
            run: "run",
            state: "pending",
            prompt: "<script>",
        }],
        ["@hammz/question", "question-model", "decision", 4, {}, {
            run: "other",
            state: "answered",
        }],
        ["@hammz/yinz", "current", "test-with-summary", 1, {
            workflowRunId: "run",
            specName: "test",
        }, {
            output: large,
            summary: "Test report: FAILED\n" + large,
            outputComplete: true,
        }],
    ] as const;
    try {
        const context: Parameters<typeof report.execute>[0] = {
            repoDir: ".",
            workflowRunId: "run",
            workflowName: "review",
            workflowStatus: "suspended",
            stepExecutions: [{
                jobName: "main",
                stepName: "question",
                status: "succeeded",
                modelType: "@hammz/question",
                modelId: "question-model",
                dataHandles: [{ name: "decision", version: 3 }],
            }, {
                jobName: "main",
                stepName: "approval",
                status: "waiting_approval",
                modelType: "",
                modelId: "",
                dataHandles: [],
            }],
            definitionRepository: {
                findById: (_type, id) => {
                    if (id === "earlier") {
                        throw new Error(
                            "Report looked up a name already present in provenance",
                        );
                    }
                    return Promise.resolve({ name: id + "-name" });
                },
            },
            dataRepository: {
                findAllGlobal: () =>
                    Promise.resolve(
                        rows.map(([type, id, name, version, tags]) => ({
                            modelType: { normalized: type },
                            modelId: id,
                            data: { name, version, tags },
                        })),
                    ),
                getContent: (type, id, name, version) => {
                    const value = rows.find((r) =>
                        r[0] === type && r[1] === id && r[2] === name &&
                        r[3] === version
                    )?.[5];
                    return Promise.resolve(
                        value
                            ? new TextEncoder().encode(
                                typeof value === "string"
                                    ? value
                                    : JSON.stringify(value),
                            )
                            : null,
                    );
                },
            },
        };
        const result = await report.execute(context);
        if (
            result.json.resources.length !== 5 ||
            result.json.outstanding[0] !== "approval" ||
            result.markdown.includes("<script>") ||
            result.json.resources[0].model !== "earlier-name" ||
            result.json.resources[1].attributes.test.output !== large ||
            result.json.resources[2].attributes !== null ||
            result.json.resources[4].attributes.output !== undefined ||
            result.json.resources[4].attributes.reportIndex !== 0 ||
            result.json.testReports[0] !== "Test report: FAILED\n" + large ||
            !result.markdown.includes(
                "```text\nTest report: FAILED\n" + large,
            ) ||
            result.json.steps[0].outputs[0].attributes.state !== "pending"
        ) {
            throw new Error(
                "Report lost saved evidence, exact versions, or missing content",
            );
        }
        if (
            !result.json.summary.some((line) =>
                line.includes('"topic" → "main"')
            ) ||
            !result.json.summary.includes("Review: current. Tests: failed.") ||
            result.markdown.indexOf("Next:") > result.markdown.indexOf("Run:")
        ) {
            throw new Error(
                "Report did not lead with the outcome and next action",
            );
        }
        const kept = await report.execute({
            ...context,
            inputs: { phase: "cleanup", answer: "keep", checkout: "/topic" },
        });
        if (
            !kept.json.summary.includes(
                "Integration completed locally. No commits were pushed.",
            ) ||
            !kept.json.summary.includes(
                "Worktree, branch and VM resources kept.",
            ) || kept.json.summary.some((line) => line.startsWith("Next:"))
        ) throw new Error("Cleanup report recommended already completed work");
        for (const phase of ["commit", "review-agent", "tests"]) {
            const action = await report.execute({
                ...context,
                inputs: { phase },
            });
            if (
                action.json.summary.some((line) =>
                    line.startsWith("Next:") || line.startsWith("Review:")
                ) || !action.json.summary.some((line) =>
                    line.startsWith("Source revision before action:")
                )
            ) {
                throw new Error(
                    "Action report presented pre-action checks as current",
                );
            }
        }
    } finally {
        Deno.Command = original;
    }
});
