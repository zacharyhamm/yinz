//! Real managed-service regression. Requires YINZ_TEST_BINARY and a compatible Swamp on PATH.
use super::*;

struct ServiceCleanup(PathBuf);
impl Drop for ServiceCleanup {
    fn drop(&mut self) {
        // Even an assertion failure must not leave a service using a deleted repo.
        if let Ok(Some(state)) = endpoint(&self.0) {
            if alive(&state) && process_started(state.pid) == Some(state.started) {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(state.pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
        }
    }
}

async fn frame(events: &mut mpsc::UnboundedReceiver<Value>) -> Value {
    tokio::time::timeout(Duration::from_secs(90), events.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a built Yinz and compatible Swamp; see docs/SWAMP_WORKFLOWS.md"]
async fn refreshes_local_extensions_once_after_active_work_and_preserves_suspension() {
    assert!(std::env::var_os("YINZ_TEST_BINARY").is_some());
    let root = tempfile::tempdir()
        .unwrap()
        .into_path()
        .canonicalize()
        .unwrap();
    eprintln!("Refresh fixture: {}", root.display());
    let cleanup = ServiceCleanup(root.clone());
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap()
        .success());
    cli(
        &root,
        "repo init",
        &["repo", "init", root.to_str().unwrap(), "--tool", "none"],
    )
    .unwrap();
    let models = root.join("extensions/models");
    std::fs::create_dir_all(models.join("_lib")).unwrap();
    let helper = models.join("_lib/name.ts");
    std::fs::write(&helper, "export const name = 'before';").unwrap();
    let source = models.join("refresh.ts");
    std::fs::write(&source, r#"
import { z } from 'npm:zod@4.3.6';
import { name } from './_lib/name.ts';
export const model = {
  type: '@test/refresh', version: '2026.09.11.1', globalArguments: z.object({}),
  methods: {
    [name]: { description: 'Read-only probe', arguments: z.object({}), execute: async () => ({dataHandles: []}) },
    hold: { description: 'Hold until released by test', arguments: z.object({}), execute: async (_, c) => {
      await Deno.writeTextFile(c.repoDir + '/started', '');
      while (true) {
        c.signal.throwIfAborted();
        try { await Deno.stat(c.repoDir + '/release'); break; }
        catch (e) { if (!(e instanceof Deno.errors.NotFound)) throw e; }
        await new Promise(r => setTimeout(r, 20));
      }
      return {dataHandles: []};
    }}
  }
};
"#).unwrap();
    let probe = cli(
        &root,
        "workflow create",
        &["workflow", "create", "refresh-probe"],
    )
    .unwrap();
    let hold = cli(
        &root,
        "workflow create",
        &["workflow", "create", "refresh-hold"],
    )
    .unwrap();
    let write_workflow = |workflow: &Value, method: &str, suspend: bool| {
        let mut steps = vec![
            json!({"name":"probe","task":{"type":"model_method","modelType":"@test/refresh","modelName":"probe","methodName":method,"inputs":{}}}),
        ];
        if suspend {
            steps.push(json!({"name":"approval","dependsOn":[{"step":"probe","condition":{"type":"succeeded"}}],"task":{"type":"manual_approval","prompt":"Pause"}}));
        }
        std::fs::write(workflow["path"].as_str().unwrap(), serde_json::to_vec(&json!({"id":workflow["id"],"name":workflow["name"],"version":1,"jobs":[{"name":"main","steps":steps}]})).unwrap()).unwrap();
    };
    write_workflow(&probe, "before", false);
    write_workflow(&hold, "hold", true);
    let path = root.clone();
    tokio::task::spawn_blocking(move || prewarm(&path))
        .await
        .unwrap()
        .unwrap();
    let result = async {
        let initial = endpoint(&root).unwrap().unwrap();
        assert!(!initial.token.is_empty());
        let original = initial.child_pid;
        assert_eq!(
            call(
                root.clone(),
                "workflow.validate",
                json!({"workflowIdOrName":"refresh-probe"})
            )
            .await?["passed"],
            true
        );
        let mut active = request(
            root.clone(),
            "hold".into(),
            "workflow.run".into(),
            json!({"workflowIdOrName":"refresh-hold","noSupersede":true}),
        )
        .await?;
        let run = loop {
            let f = frame(&mut active).await;
            if let Some(run) = f["event"]["runId"].as_str() {
                break run.to_owned();
            }
            assert_ne!(f["type"], "error", "{f}");
        };
        tokio::time::timeout(Duration::from_secs(30), async {
            while !root.join("started").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        std::fs::write(&helper, "export const name = 'checkout_status';")?;
        std::fs::rename(&source, models.join("renamed.ts"))?;
        write_workflow(&probe, "checkout_status", false);
        let mut queued = request(
            root.clone(),
            "queued".into(),
            "workflow.validate".into(),
            json!({"workflowIdOrName":"refresh-probe"}),
        )
        .await?;
        assert_eq!(frame(&mut queued).await["payload"]["status"], "waiting");
        assert_eq!(endpoint(&root)?.unwrap().child_pid, original);
        assert_eq!(
            call(
                root.clone(),
                "workflow.history.get",
                json!({"workflowIdOrName":run})
            )
            .await?["status"],
            "running"
        );
        let mut contender = request(
            root.clone(),
            "contender".into(),
            "workflow.run".into(),
            json!({"workflowIdOrName":"refresh-probe","noSupersede":true}),
        )
        .await?;
        assert_eq!(frame(&mut contender).await["payload"]["status"], "waiting");
        // This test is about the clock: an unsent request must survive the entire
        // normal dispatch deadline while another execution remains active.
        let early = tokio::time::timeout(STARTUP + Duration::from_secs(1), queued.recv()).await;
        assert!(
            early.is_err(),
            "queued request ended before release: {early:?}"
        );
        std::fs::write(root.join("release"), "")?;
        loop {
            let f = frame(&mut queued).await;
            if f["type"] == "managed.progress" {
                continue;
            }
            assert_eq!(f["payload"]["data"]["passed"], true, "{f}");
            break;
        }
        loop {
            let f = frame(&mut contender).await;
            assert_ne!(f["type"], "error", "{f}");
            if f["type"] == "done" {
                break;
            }
        }
        let history = call(
            root.clone(),
            "workflow.history.search",
            json!({"workflow":"refresh-probe"}),
        )
        .await?;
        let runs = history["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["workflowName"] == "refresh-probe")
            .collect::<Vec<_>>();
        assert_eq!(runs.len(), 1, "queued execution must run exactly once");
        assert_eq!(runs[0]["status"], "succeeded");
        let refreshed = endpoint(&root)?.unwrap().child_pid;
        assert_ne!(refreshed, original);
        assert!(
            endpoint(&root)?.unwrap().token != initial.token,
            "refresh must rotate the service credential"
        );
        assert_eq!(
            call(
                root.clone(),
                "workflow.history.get",
                json!({"workflowIdOrName":run})
            )
            .await?["status"],
            "suspended"
        );
        assert_eq!(
            call(
                root.clone(),
                "workflow.validate",
                json!({"workflowIdOrName":"refresh-probe"})
            )
            .await?["passed"],
            true
        );
        assert_eq!(
            endpoint(&root)?.unwrap().child_pid,
            refreshed,
            "all contenders share one refresh"
        );
        // Broken edits must neither dispatch nor replace the healthy old process.
        std::fs::write(&helper, "this is not valid TypeScript {{{")?;
        let error = call(
            root.clone(),
            "workflow.validate",
            json!({"workflowIdOrName":"refresh-probe"}),
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Cannot load updated Swamp extensions"),
            "{error:#}"
        );
        assert_eq!(endpoint(&root)?.unwrap().child_pid, refreshed);
        // Restoring the healthy generation keeps the service and its suspension.
        std::fs::write(&helper, "export const name = 'checkout_status';")?;
        call(
            root.clone(),
            "workflow.approve",
            json!({"workflowIdOrName":"refresh-hold","runId":run,"stepName":"approval"}),
        )
        .await?;
        let mut resumed = request(
            root.clone(),
            "resume".into(),
            "workflow.resume".into(),
            json!({"workflowIdOrName":"refresh-hold","runId":run}),
        )
        .await?;
        loop {
            let f = frame(&mut resumed).await;
            assert_ne!(f["type"], "error", "{f}");
            if f["type"] == "done" {
                break;
            }
        }
        assert_eq!(
            call(
                root.clone(),
                "workflow.history.get",
                json!({"workflowIdOrName":run})
            )
            .await?["status"],
            "succeeded"
        );
        assert_eq!(endpoint(&root)?.unwrap().child_pid, refreshed);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    lifecycle(root.clone(), "stop").await.unwrap();
    drop(cleanup);
    std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    result.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires Swamp on PATH"]
fn external_source_fingerprints_track_configured_directories_and_membership() {
    let repo = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let flat = tempfile::tempdir().unwrap();
    let root = repo.path();
    cli(
        root,
        "repo init",
        &["repo", "init", root.to_str().unwrap(), "--tool", "none"],
    )
    .unwrap();
    let marker = external.path().join(".swamp.yaml");
    std::fs::write(&marker, "modelsDir: custom/models\n").unwrap();
    for path in [
        external.path().join("custom/models/_lib"),
        external.path().join("extensions/reports"),
        flat.path().to_owned(),
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    // Direct-content sources must declare an extension before Swamp accepts them.
    std::fs::write(flat.path().join("model.ts"), "export const model = {};").unwrap();
    for source in [external.path(), flat.path()] {
        cli(
            root,
            "extension source",
            &["extension", "source", "add", source.to_str().unwrap()],
        )
        .unwrap();
    }
    for directory in [
        external.path().join("custom/models/_lib"),
        external.path().join("extensions/reports"),
        flat.path().to_owned(),
    ] {
        let before = extension_fingerprint(root).unwrap();
        let file = directory.join("helper.ts");
        std::fs::write(&file, "export const value = 1;").unwrap();
        let added = extension_fingerprint(root).unwrap();
        assert_ne!(before, added);
        std::fs::write(&file, "export const value = 2;").unwrap();
        assert_ne!(added, extension_fingerprint(root).unwrap());
        std::fs::remove_file(file).unwrap();
        assert_eq!(before, extension_fingerprint(root).unwrap());
    }
    let before = extension_fingerprint(root).unwrap();
    std::fs::write(
        &marker,
        "modelsDir: custom/models\n# changed source settings\n",
    )
    .unwrap();
    assert_ne!(before, extension_fingerprint(root).unwrap());
    let custom = external.path().join("other/models");
    std::fs::create_dir_all(&custom).unwrap();
    std::fs::write(&marker, "modelsDir: other/models\n").unwrap();
    let before = extension_fingerprint(root).unwrap();
    std::fs::write(custom.join("added.ts"), "export const value = 3;").unwrap();
    assert_ne!(before, extension_fingerprint(root).unwrap());
}

#[tokio::test]
#[ignore = "requires a built Yinz and compatible Swamp; see docs/SWAMP_WORKFLOWS.md"]
async fn agent_workflow_definition_and_execution_lifecycle() {
    assert!(std::env::var_os("YINZ_TEST_BINARY").is_some());
    let root = tempfile::tempdir()
        .unwrap()
        .into_path()
        .canonicalize()
        .unwrap();
    eprintln!("Agent workflow fixture: {}", root.display());
    let cleanup = ServiceCleanup(root.clone());
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap()
        .success());
    cli(
        &root,
        "repo init",
        &["repo", "init", root.to_str().unwrap(), "--tool", "none"],
    )
    .unwrap();
    cli(
        &root,
        "model create",
        &["model", "create", "command/shell", "probe"],
    )
    .unwrap();
    // Seed a repository-owned extension through a Swamp-assigned scaffold.
    let extension = cli(
        &root,
        "workflow create",
        &["workflow", "create", "owned-probe"],
    )
    .unwrap();
    let extension_path = root.join("extensions/workflows/owned.yaml");
    std::fs::create_dir_all(extension_path.parent().unwrap()).unwrap();
    std::fs::rename(extension["path"].as_str().unwrap(), &extension_path).unwrap();
    std::fs::create_dir_all(root.join("workflows/owned-probe")).unwrap();
    std::os::unix::fs::symlink(
        &extension_path,
        root.join("workflows/owned-probe/workflow.yaml"),
    )
    .unwrap();
    let extension_source = std::fs::read(&extension_path).unwrap();
    let path = root.clone();
    tokio::task::spawn_blocking(move || prewarm(&path))
        .await
        .unwrap()
        .unwrap();
    async fn execution(root: &Path, operation: &str, payload: Value) -> Result<String> {
        let mut events = request(
            root.into(),
            uuid::Uuid::new_v4().to_string(),
            operation.into(),
            payload,
        )
        .await?;
        let mut run = None;
        loop {
            let event = frame(&mut events).await;
            assert_ne!(event["type"], "error", "{event}");
            if let Some(id) = event["event"]["runId"].as_str() {
                run = Some(id.to_owned());
            }
            if event["type"] == "done" {
                return run.context("no run ID");
            }
        }
    }
    let result = async {
        call(root.clone(), "workflow.schema", json!({})).await?;
        let created = call(root.clone(), "workflow.create", json!({"name":"agent-probe"})).await?;
        let id = created["id"].as_str().context("no workflow ID")?;
        let mut expected = definitions::get(root.clone(), id, &|_| {}).await?;
        let definition = |command: &str, approval: bool| {
            let mut steps = vec![json!({"name":"probe","task":{"type":"model_method","modelIdOrName":"probe","methodName":"execute","inputs":{"run":command}}})];
            if approval { steps.push(json!({"name":"approval","dependsOn":[{"step":"probe","condition":{"type":"succeeded"}}],"task":{"type":"manual_approval","prompt":"Probe approval"}})); }
            json!({"id":id,"name":"agent-probe","version":1,"inputs":{"type":"object","properties":{},"additionalProperties":false},"jobs":[{"name":"main","steps":steps}]})
        };
        let mut complete = definition("true", false);
        complete["jobs"][0]["concurrency"] = json!(2);
        complete["jobs"][0]["steps"][0]["guard"] = json!("${{ true }}");
        complete["jobs"][0]["steps"][0]["forEach"] = json!({"item":"target","in":"${{ [1] }}"});
        complete["jobs"][0]["steps"][0]["allowFailure"] = json!(true);
        definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":complete.to_string()}), &|_| {}).await?;
        expected = definitions::get(root.clone(), id, &|_| {}).await?;
        let mut replacement: Value = serde_saphyr::from_str(expected["source"]["content"].as_str().context("missing source content")?)?;
        assert_eq!(replacement["jobs"][0]["concurrency"], 2);
        for field in ["guard", "forEach", "allowFailure"] {
            assert_eq!(replacement["jobs"][0]["steps"][0][field], complete["jobs"][0]["steps"][0][field]);
        }
        replacement["description"] = json!("Keep all other source fields");
        definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":replacement.to_string()}), &|_| {}).await?;
        let stale = definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":complete.to_string()}), &|_| {}).await.unwrap_err();
        assert!(stale.to_string().contains("stale_definition"));
        expected = definitions::get(root.clone(), id, &|_| {}).await?;
        let retained: Value = serde_saphyr::from_str(expected["source"]["content"].as_str().unwrap())?;
        assert_eq!(retained["jobs"], replacement["jobs"]);
        definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":definition("echo retained-output; exit 7", false).to_string()}), &|_| {}).await?;
        // An already-consumed inspection must not overwrite a later definition.
        let stale = definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":definition("true", false).to_string()}), &|_| {}).await.unwrap_err();
        assert!(stale.to_string().contains("stale_definition"));
        expected = definitions::get(root.clone(), id, &|_| {}).await?;
        let mut invalid = definition("true", false); invalid["id"] = json!("changed");
        assert!(definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":invalid.to_string()}), &|_| {}).await.is_err());
        assert!(definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":json!({"id":id,"name":"agent-probe","jobs":"invalid"}).to_string()}), &|_| {}).await.is_err());
        assert_eq!(definitions::get(root.clone(), id, &|_| {}).await?, expected);
        assert_eq!(call(root.clone(), "workflow.validate", json!({"workflowIdOrName":id})).await?["passed"], true);
        let run = execution(&root, "workflow.run", json!({"workflowIdOrName":id,"noSupersede":true})).await?;
        assert_eq!(call(root.clone(), "workflow.history.get", json!({"workflowIdOrName":run})).await?["status"], "failed");
        // Inspect generated reports and output before repairing the failed definition.
        let reports = call(root.clone(), "report.get", json!({"reportName":"@swamp/workflow-summary","workflow":"agent-probe"})).await?;
        assert!(!reports.is_null());
        let outputs = call(root.clone(), "data.query", json!({"predicate":format!("has(tags.workflowRunId) && tags.workflowRunId == {}", json!(run)),"select":"{\"name\":name,\"version\":version}"})).await?;
        assert!(!outputs.is_null());
        call(root.clone(), "workflow.history.logs", json!({"runIdOrWorkflow":run,"tail":10})).await?;
        definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":definition("echo repaired", false).to_string()}), &|_| {}).await?;
        call(root.clone(), "workflow.validate", json!({"workflowIdOrName":id})).await?;
        execution(&root, "workflow.resume", json!({"workflowIdOrName":id,"runId":run,"from":"probe","inputs":{}})).await?;
        assert_eq!(call(root.clone(), "workflow.history.get", json!({"workflowIdOrName":run})).await?["status"], "succeeded");
        expected = definitions::get(root.clone(), id, &|_| {}).await?;
        definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":definition("true", true).to_string()}), &|_| {}).await?;
        for decision in ["workflow.approve", "workflow.reject"] {
            call(root.clone(), "workflow.validate", json!({"workflowIdOrName":id})).await?;
            let run = execution(&root, "workflow.run", json!({"workflowIdOrName":id,"noSupersede":true})).await?;
            assert_eq!(call(root.clone(), "workflow.history.get", json!({"workflowIdOrName":run})).await?["status"], "suspended");
            call(root.clone(), "workflow.approvals", json!({})).await?;
            call(root.clone(), decision, json!({"workflowIdOrName":id,"runId":run,"stepName":"approval"})).await?;
            if decision == "workflow.approve" {
                execution(&root, "workflow.resume", json!({"workflowIdOrName":id,"runId":run})).await?;
                assert_eq!(call(root.clone(), "workflow.history.get", json!({"workflowIdOrName":run})).await?["status"], "succeeded");
            }
        }
        expected = definitions::get(root.clone(), id, &|_| {}).await?;
        definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":id,"expected":expected,"content":definition("sleep 60", false).to_string()}), &|_| {}).await?;
        call(root.clone(), "workflow.validate", json!({"workflowIdOrName":id})).await?;
        let mut events = request(root.clone(), "cancel-probe".into(), "workflow.run".into(), json!({"workflowIdOrName":id,"noSupersede":true})).await?;
        let run = loop { let event = frame(&mut events).await; assert_ne!(event["type"], "error", "{event}"); if let Some(id) = event["event"]["runId"].as_str() {break id.to_owned();} };
        cancel(root.clone(), run.clone()).await?;
        loop { let event = frame(&mut events).await; if event["type"] == "done" {break;} }
        assert_eq!(call(root.clone(), "workflow.history.get", json!({"workflowIdOrName":run})).await?["status"], "cancelled");
        expected = definitions::get(root.clone(), id, &|_| {}).await?;
        definitions::change(root.clone(), "workflow.delete", json!({"workflowIdOrName":id,"expected":expected}), &|_| {}).await?;
        assert!(definitions::get(root.clone(), id, &|_| {}).await.is_err());
        let inspected = definitions::get(root.clone(), "owned-probe", &|_| {}).await?;
        assert_eq!(inspected["source"]["extension"], true);
        assert_eq!(inspected["source"]["editable"], true);
        let mut updated = inspected.clone();
        updated.as_object_mut().unwrap().remove("source");
        updated.as_object_mut().unwrap().remove("path");
        updated["description"] = json!("Project override");
        definitions::change(root.clone(), "workflow.edit", json!({"workflowIdOrName":"owned-probe","expected":inspected,"content":updated.to_string()}), &|_| {}).await?;
        let overridden = definitions::get(root.clone(), "owned-probe", &|_| {}).await?;
        assert_eq!(overridden["id"], extension["id"]);
        assert_eq!(overridden["source"]["extension"], false);
        assert_eq!(overridden["description"], "Project override");
        assert_eq!(std::fs::read(&extension_path).unwrap(), extension_source);
        definitions::change(root.clone(), "workflow.delete", json!({"workflowIdOrName":"owned-probe","expected":overridden}), &|_| {}).await?;
        assert_eq!(definitions::get(root.clone(), "owned-probe", &|_| {}).await?["source"]["extension"], true);
        Ok::<_, anyhow::Error>(())
    }.await;
    lifecycle(root.clone(), "stop").await.unwrap();
    drop(cleanup);
    result.unwrap();
}
