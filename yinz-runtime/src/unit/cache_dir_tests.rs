use super::*;

#[test]
fn test_cache_override_precedes_platform_cache_initialization() {
    const PROBE: &str = "YINZ_CACHE_OVERRIDE_PROBE";
    if std::env::var_os(PROBE).is_some() {
        let assigned = PathBuf::from(std::env::var_os("YINZ_TEST_CACHE_ROOT").unwrap());
        let expected = std::env::var_os("YINZ_TEST_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| assigned.join(std::process::id().to_string()).join("cache"));
        assert_eq!(*YINZ_CACHE_DIR, expected);
        assert!(YINZ_PLUGIN_PERMISSIONS_CACHE.starts_with(&expected));
        assert!(YINZ_SESSION_INFO_CACHE_DIR.starts_with(&expected));
        std::fs::create_dir_all(&*YINZ_CACHE_DIR).unwrap();
        return;
    }
    for shared in [false, true] {
        let assigned = tempfile::tempdir().unwrap();
        for _ in 0..2 {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "consts::tests::test_cache_override_precedes_platform_cache_initialization",
                ])
                .env(PROBE, "1")
                .env("YINZ_TEST_CACHE_ROOT", assigned.path())
                .env_remove("YINZ_TEST_CACHE_DIR");
            if shared {
                command.env("YINZ_TEST_CACHE_DIR", assigned.path().join("shared"));
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let entries: Vec<_> = std::fs::read_dir(assigned.path()).unwrap().collect();
        if shared {
            assert_eq!(
                entries.len(),
                1,
                "both processes use the exact cache directory"
            );
            assert!(assigned.path().join("shared").is_dir());
        } else {
            assert_eq!(entries.len(), 2, "each test process has a separate cache");
            for process in entries {
                assert!(process.unwrap().path().join("cache").is_dir());
            }
        }
    }
}
