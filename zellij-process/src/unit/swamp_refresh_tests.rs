//! Real managed-service regression. Requires ZELLIJ_TEST_BINARY and a compatible Swamp on PATH.
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
#[ignore = "requires a built Zellij and compatible Swamp; see docs/SWAMP_WORKFLOWS.md"]
async fn refreshes_local_extensions_once_after_active_work_and_preserves_suspension() {
    assert!(std::env::var_os("ZELLIJ_TEST_BINARY").is_some());
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
    let probe = cli(&root, &["workflow", "create", "refresh-probe"]).unwrap();
    let hold = cli(&root, &["workflow", "create", "refresh-hold"]).unwrap();
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
        let original = endpoint(&root).unwrap().unwrap().child_pid;
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
