use super::*;

#[test]
fn test_cache_override_precedes_platform_cache_initialization() {
    const PROBE: &str = "ZELLIJ_CACHE_OVERRIDE_PROBE";
    if std::env::var_os(PROBE).is_some() {
        let assigned = PathBuf::from(std::env::var_os("ZELLIJ_TEST_CACHE_ROOT").unwrap());
        let expected = assigned.join(std::process::id().to_string()).join("cache");
        assert_eq!(*ZELLIJ_CACHE_DIR, expected);
        assert!(ZELLIJ_PLUGIN_PERMISSIONS_CACHE.starts_with(&expected));
        assert!(ZELLIJ_SESSION_INFO_CACHE_DIR.starts_with(&expected));
        std::fs::create_dir_all(&*ZELLIJ_CACHE_DIR).unwrap();
        return;
    }
    let assigned = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "consts::tests::test_cache_override_precedes_platform_cache_initialization",
            ])
            .env(PROBE, "1")
            .env("ZELLIJ_TEST_CACHE_ROOT", assigned.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let processes: Vec<_> = std::fs::read_dir(assigned.path()).unwrap().collect();
    assert_eq!(processes.len(), 2, "each test process has a separate cache");
    for process in processes {
        assert!(process.unwrap().path().join("cache").is_dir());
    }
}
