import { report } from "./workflow.ts";

Deno.test("reports retain exact question versions and missing evidence", async () => {
    const reads: unknown[] = [];
    const original = Deno.Command;
    class SavedRun extends original {
        override output() {
            return Promise.resolve({
                code: 0,
                success: true,
                signal: null,
                stdout: new TextEncoder().encode(
                    JSON.stringify({
                        results: [{
                            name: "review-current",
                            attributes: { findings: [] },
                        }],
                        limited: false,
                    }),
                ),
                stderr: new Uint8Array(),
            });
        }
    }
    Deno.Command = SavedRun;
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
            dataRepository: {
                getContent: (...args) => {
                    reads.push(args);
                    return Promise.resolve(
                        new TextEncoder().encode(
                            '{"state":"pending","prompt":"<script>"}',
                        ),
                    );
                },
            },
        });
        if (
            JSON.stringify(reads) !==
                JSON.stringify([[
                    "@hammz/question",
                    "question-model",
                    "decision",
                    3,
                ]]) ||
            result.json.resources.length !== 1 ||
            result.json.outstanding[0] !== "approval" ||
            result.markdown.includes("<script>")
        ) {
            throw new Error(
                "Report lost the decision, version, suspension or text escaping",
            );
        }
    } finally {
        Deno.Command = original;
    }
});
