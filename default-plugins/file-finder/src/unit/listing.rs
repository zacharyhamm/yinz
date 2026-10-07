use super::*;
use std::process::Command;

#[test]
fn git_listing_preserves_literal_filenames() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(&repo)
        .output()
        .unwrap()
        .status
        .success());
    yinz_test_support::write_warm_script(&root.path().join("fzf"), "cat");
    let output = root.path().join("selection");
    for name in [" leading and trailing ", "a'\"雪,#?%2C.pdf", "a\r"] {
        std::fs::write(repo.join(name), "").unwrap();
        let result = Command::new("sh")
            .args(["-c", &build(&BTreeMap::new(), output.to_str().unwrap())])
            .current_dir(&repo)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    root.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let selection = std::fs::read_to_string(&output).unwrap();
        assert_eq!(
            parse_selection(&selection),
            Some(repo.join(name).to_str().unwrap())
        );
        std::fs::remove_file(repo.join(name)).unwrap();
    }
}
