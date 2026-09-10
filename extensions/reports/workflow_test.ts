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
        ["@hammz/zellij", "earlier", "review-current", 2, {
            workflowRunId: "run",
            specName: "review",
        }, { findings: [] }],
        ["@hammz/zellij", "earlier", "test-current", 1, {
            workflowRunId: "previous",
            specName: "test",
        }, { output: large }],
        ["@hammz/zellij", "earlier", "test-current", 2, {
            workflowRunId: "other",
            specName: "test",
        }, { output: "newer" }],
        ["@hammz/zellij", "current", "checks-current", 1, {
            workflowRunId: "run",
            specName: "checks",
        }, {
            test: {
                output: "preview",
                outputRef: {
                    modelType: "@hammz/zellij",
                    modelId: "earlier",
                    name: "test-current",
                    version: 1,
                },
            },
        }],
        ["@hammz/zellij", "current", "missing", 1, {
            workflowRunId: "run",
            specName: "test",
        }, null],
        ["@hammz/zellij", "current", "claim", 1, {
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
    ] as const;
    try {
        const result = await report.execute({
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
                findById: (_type, id) =>
                    Promise.resolve({ name: id + "-name" }),
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
        });
        if (
            result.json.resources.length !== 4 ||
            result.json.outstanding[0] !== "approval" ||
            result.markdown.includes("<script>") ||
            result.json.resources[0].model !== "earlier-name" ||
            result.json.resources[1].attributes.test.output !== large ||
            result.json.resources[2].attributes !== null ||
            result.json.steps[0].outputs[0].attributes.state !== "pending"
        ) {
            throw new Error(
                "Report lost saved evidence, exact versions, or missing content",
            );
        }
    } finally {
        Deno.Command = original;
    }
});
