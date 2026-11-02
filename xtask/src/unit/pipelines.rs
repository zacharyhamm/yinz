use super::*;
use std::path::PathBuf;

fn install_flags(profile: Option<&str>, low_memory: bool) -> flags::Install {
    flags::Install {
        data_dir: None,
        destination: PathBuf::from("/tmp/yinz"),
        args: vec![],
        profile: profile.map(str::to_owned),
        low_memory,
    }
}

#[test]
fn install_selects_release_low_memory_or_explicit_profile() {
    for (profile, low_memory, expected) in [
        (None, false, "release"),
        (None, true, "release-low-mem"),
        (Some("dev"), false, "dev"),
    ] {
        let flags = install_flags(profile, low_memory);
        assert_eq!(install_profile(&flags).unwrap(), expected);
    }
}

#[test]
fn install_rejects_low_memory_with_explicit_profile() {
    let flags = install_flags(Some("release"), true);
    let error = install_profile(&flags).unwrap_err().to_string();
    assert!(error.contains("--low-memory"), "{error}");
}
