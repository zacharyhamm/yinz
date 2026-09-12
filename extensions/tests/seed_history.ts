/** Populate a disposable timing fixture with Swamp's real persistence adapters.
 * Run with --config <swamp-source>/deno.json; args: source, fixture, run count.
 */
import { join, toFileUrl } from "@std/path";

const [source, fixture, rawCount] = Deno.args;
const count = Number(rawCount);
if (
    !fixture.includes("zellij-workflow-acceptance-") ||
    !Number.isInteger(count) || count < 0
) {
    throw new Error(
        "Expected a disposable acceptance fixture and a nonnegative count",
    );
}
const load = (path: string) =>
    import(toFileUrl(join(source, "src", path)).href);
const { Definition } = await load("domain/definitions/definition.ts");
const { ModelType } = await load("domain/models/model_type.ts");
const { ModelOutput } = await load("domain/models/model_output.ts");
const { WorkflowRun } = await load("domain/workflows/workflow_run.ts");
const { YamlDefinitionRepository } = await load(
    "infrastructure/persistence/yaml_definition_repository.ts",
);
const { YamlOutputRepository } = await load(
    "infrastructure/persistence/yaml_output_repository.ts",
);
const { YamlWorkflowRepository } = await load(
    "infrastructure/persistence/yaml_workflow_repository.ts",
);
const { YamlWorkflowRunRepository } = await load(
    "infrastructure/persistence/yaml_workflow_run_repository.ts",
);
const { swampPath, SWAMP_SUBDIRS } = await load(
    "infrastructure/persistence/paths.ts",
);
const definitions = new YamlDefinitionRepository(
    fixture,
    undefined,
    swampPath(fixture, SWAMP_SUBDIRS.autoDefinitions),
    false,
);
const outputs = new YamlOutputRepository(fixture);
const workflows = new YamlWorkflowRepository(fixture);
const runs = new YamlWorkflowRunRepository(fixture);
const workflow = await workflows.findByName("@hammz/zellij-agent-exit");
if (!workflow) throw new Error("Missing fixture workflow");
for (let i = 0; i < count; i++) {
    const run = WorkflowRun.create(workflow);
    run.start();
    run.cancel("Retained fixture run");
    await runs.save(workflow.id, run);
    for (
        const typeName of [
            "@hammz/question",
            "@hammz/zellij",
            "@swamp/git",
        ]
    ) {
        const type = ModelType.create(typeName);
        const definition = Definition.create({
            type: typeName,
            name: `retained-${i}-${typeName.replace(/[^a-z0-9]+/g, "-")}`,
            globalArguments: {
                session: "retained-fixture",
                directory: fixture,
            },
        });
        await definitions.save(type, definition);
        const output = ModelOutput.create({
            definitionId: definition.id,
            methodName: "fixture",
            provenance: {
                definitionHash: await definition.computeHash(),
                modelVersion: "fixture",
                triggeredBy: "workflow",
                workflowId: workflow.id,
                workflowRunId: run.id,
            },
        });
        output.markRunning(Deno.pid);
        output.markSucceeded();
        await outputs.save(type, "fixture", output);
    }
}
console.log(
    JSON.stringify({ runs: count, definitions: count * 3, outputs: count * 3 }),
);
