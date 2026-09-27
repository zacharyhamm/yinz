//! Definition inspection and optimistic editing at the managed-server boundary.
use super::*;

/// Swamp's getPath is the primary path even for extension-only definitions.
/// Resolve its name symlink before deciding whether this checkout owns the source.
fn source(root: &Path, definition: &Value) -> Result<Value> {
    let root = root
        .canonicalize()
        .context("cannot resolve control checkout")?;
    let path = PathBuf::from(
        definition["path"]
            .as_str()
            .context("workflow has no path")?,
    );
    let primary = path.canonicalize().ok();
    let resolved = primary.clone().or_else(|| {
        let name = Path::new(definition["name"].as_str()?);
        if name.is_absolute()
            || name
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return None;
        }
        root.join("workflows")
            .join(name)
            .join("workflow.yaml")
            .canonicalize()
            .ok()
    });
    let Some(path) = resolved else {
        return Ok(
            json!({"editable":false,"reason":"Workflow source cannot be resolved in the control checkout"}),
        );
    };
    let editable = path.strip_prefix(&root).is_ok_and(|relative| {
        !relative
            .components()
            .any(|part| part.as_os_str() == ".swamp")
    });
    let bytes = std::fs::read(&path).context("cannot inspect workflow source")?;
    let content = std::str::from_utf8(&bytes).context("workflow source is not UTF-8")?;
    Ok(
        json!({"path":path,"editable":editable,"content":content,"revision":Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        "extension":primary.is_none(),
        "editBehavior":if primary.is_none() {"Swamp saves a project override with the same ID"} else {"Swamp replaces the project definition"}}),
    )
}

pub async fn get(
    root: PathBuf,
    workflow: &str,
    progress: &(impl Fn(Value) + Sync),
) -> Result<Value> {
    let mut definition = call_with_progress(
        root.clone(),
        "workflow.get",
        json!({"workflowIdOrName":workflow}),
        progress,
    )
    .await?;
    definition["source"] = source(&root, &definition)?;
    Ok(definition)
}

pub async fn change(
    root: PathBuf,
    operation: &str,
    mut payload: Value,
    progress: &(impl Fn(Value) + Sync),
) -> Result<Value> {
    let workflow = payload["workflowIdOrName"]
        .as_str()
        .context("workflow is required")?
        .to_owned();
    let expected = payload
        .as_object_mut()
        .and_then(|p| p.remove("expected"))
        .context("Inspect with workflow_get and supply its complete result as expected")?;
    let current = get(root.clone(), &workflow, progress).await?;
    if current != expected {
        return Err(anyhow!("stale_definition: workflow changed since inspection; get it again before editing or deleting"));
    }
    if current["source"]["editable"] != true {
        return Err(anyhow!(
            "read_only_definition: downloaded or external workflow sources are inspectable only"
        ));
    }
    if operation == "workflow.edit" {
        call_with_progress(root.clone(), "workflow.schema", json!({}), progress).await?;
        let definition: Value = serde_json::from_str(
            payload["content"]
                .as_str()
                .context("definition content is required")?,
        )
        .context("definition must be a JSON object matching the workflow schema")?;
        if !definition.is_object() || definition["id"] != current["id"] {
            return Err(anyhow!("workflow ID must match the inspected definition"));
        }
        // Fetch again after schema retrieval: even that read can wait for a service refresh.
        if get(root.clone(), &workflow, progress).await? != expected {
            return Err(anyhow!("stale_definition: workflow changed before edit"));
        }
    }
    // No retry: call/request preserve an uncertain mutation outcome after disconnect.
    call_with_progress(root, operation, payload, progress).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_ownership_follows_symlinks_and_revision_tracks_content() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("extensions/workflows")).unwrap();
        std::fs::create_dir_all(root.join("workflows/@local/task")).unwrap();
        let file = root.join("extensions/workflows/task.yaml");
        let content = r#"# Retain fields omitted by workflow.get.
jobs:
  - name: main
    concurrency: 2
    steps:
      - name: probe
        guard: ${{ true }}
        forEach:
          item: target
          in: ${{ inputs.targets }}
        allowFailure: true
"#;
        std::fs::write(&file, content).unwrap();
        std::os::unix::fs::symlink(&file, root.join("workflows/@local/task/workflow.yaml"))
            .unwrap();
        let definition = json!({"path":root.join("workflows/missing.yaml"),"name":"@local/task"});
        let before = source(root, &definition).unwrap();
        assert_eq!(before["editable"], true);
        assert_eq!(before["extension"], true);
        assert_eq!(before["content"], content);
        std::fs::write(&file, "edited").unwrap();
        assert_ne!(
            source(root, &definition).unwrap()["revision"],
            before["revision"]
        );
        std::fs::create_dir_all(root.join(".swamp/downloaded")).unwrap();
        let downloaded = root.join(".swamp/downloaded/task.yaml");
        std::fs::write(&downloaded, "third-party").unwrap();
        std::fs::remove_file(root.join("workflows/@local/task/workflow.yaml")).unwrap();
        std::os::unix::fs::symlink(
            &downloaded,
            root.join("workflows/@local/task/workflow.yaml"),
        )
        .unwrap();
        assert_eq!(source(root, &definition).unwrap()["editable"], false);
        assert_eq!(
            source(root, &json!({"path":file,"name":"regular"})).unwrap()["editable"],
            true
        );
    }
}
