use super::*;

#[test]
fn provisioning_refreshes_instructions_and_preserves_operational_memory() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("memory.json"), "my goals").unwrap();
    provision(dir.path()).unwrap();
    fs::write(dir.path().join("AGENTS.md"), "old instructions").unwrap();
    provision(dir.path()).unwrap();
    let bootstrap = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(bootstrap.starts_with(BOOTSTRAP));
    assert!(bootstrap.contains(&format!(
        "{:?}",
        crate::own_binary::server_binary().unwrap()
    )));
    assert_eq!(
        fs::read_to_string(dir.path().join("memory.json")).unwrap(),
        "my goals"
    );
    for root in [".agents/skills", ".claude/skills"] {
        for name in ["yinz-mother", "yinz-agents", "yinz-sessions"] {
            assert!(dir.path().join(root).join(name).join("SKILL.md").is_file());
        }
    }
}

#[cfg(unix)]
#[test]
fn provisioning_refuses_a_link_to_global_agent_configuration() {
    let work = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(global.path(), work.path().join(".agents")).unwrap();
    assert!(provision(work.path()).is_err());
    assert_eq!(fs::read_dir(global.path()).unwrap().count(), 0);
}
