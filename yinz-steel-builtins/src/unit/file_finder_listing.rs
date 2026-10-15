use super::file_finder_tests::plugin;
use serde_json::json;
use std::{collections::BTreeMap, path::Path};

fn build_structured(
    config: &BTreeMap<String, String>,
    options: &yinz_types::pick::PickOptions,
    cwd: &Path,
    output: &str,
) -> Result<String, String> {
    let (mut runtime, _) = plugin(json!({}));
    runtime
        .invoke(
            "ff-structured-script",
            vec![json!(config), json!(options), json!(cwd), json!(output)],
        )
        .map(|value| value.as_str().unwrap().to_owned())
        .map_err(|error| format!("{error:#}"))
}
fn parse_structured_selection(source: &str) -> Result<Option<String>, String> {
    let (mut runtime, _) = plugin(json!({}));
    runtime
        .invoke("ff-selection", vec![json!(source), json!(true)])
        .map(|value| value.as_str().map(str::to_owned))
        .map_err(|error| format!("{error:#}"))
}
use std::{fs, process::Command};
use yinz_types::pick::{PickKind, PickOptions};

fn options(kind: PickKind) -> PickOptions {
    PickOptions {
        kind,
        roots: None,
        extensions: None,
        hidden: None,
        recursive: None,
        timeout_ms: None,
    }
}

#[test]
fn structured_scripts_preserve_paths_and_obey_traversal_options() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root '雪$()[*?]\\\n");
    fs::create_dir_all(root.join("Papers.pdf/nested")).unwrap();
    fs::create_dir(root.join(".hidden")).unwrap();
    for name in [
        "upper.PDF",
        "line\nbreak.pdf",
        "quote'$.pdf",
        "other.txt",
        ".secret.pdf",
        ".hidden/a.pdf",
        "Papers.pdf/nested/a.pdf",
    ] {
        fs::write(root.join(name), "").unwrap();
    }
    std::os::unix::fs::symlink(root.join("Papers.pdf"), root.join("link")).unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    yinz_test_support::write_warm_script(
        &bin.join("fzf"),
        "#!/bin/sh\nfor arg do\n  [ \"$arg\" != '--expect=' ] || exit 2\ndone\ncat\n",
    );
    let out = temp.path().join("result");
    let run = |options: &PickOptions| {
        let config = BTreeMap::from([("file_command".into(), "exit 77".into())]);
        let script = build_structured(&config, options, &root, out.to_str().unwrap()).unwrap();
        let status = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!out.with_extension("candidates").exists());
        let bytes = fs::read_to_string(&out).unwrap();
        bytes
            .split_terminator('\0')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let mut files = options(PickKind::File);
    files.extensions = Some(vec!["pdf".into()]);
    let found = run(&files);
    assert_eq!(found.len(), 4, "{found:?}");
    assert!(found.iter().any(|path| path.ends_with("line\nbreak.pdf")));
    assert!(!found.iter().any(|path| path.contains("/link/")));
    files.recursive = Some(false);
    assert_eq!(run(&files).len(), 3);
    files.hidden = Some(true);
    assert_eq!(run(&files).len(), 4);
    let dirs = run(&options(PickKind::Directory));
    assert_eq!(dirs.len(), 3);
    assert!(dirs.contains(&root.canonicalize().unwrap().to_string_lossy().into_owned()));
    let mut multiple = options(PickKind::Directory);
    multiple.roots = Some(vec!["Papers.pdf".into(), ".hidden".into()]);
    multiple.recursive = Some(false);
    assert_eq!(run(&multiple).len(), 3);
}

#[test]
fn structured_search_excludes_its_scratch_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root '雪$()[*?]\\\n");
    let bin = temp.path().join("bin");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&bin).unwrap();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let file = root.join("keep.txt");
    fs::write(&file, "keep").unwrap();
    let expected = file.canonicalize().unwrap();
    let out = alias.join("selection.sel");
    let candidates = alias.join("selection.sel.candidates");
    yinz_test_support::write_warm_script(&bin.join("fzf"), "cat");

    for search_root in [&root, &alias] {
        for recursive in [true, false] {
            let mut request = options(PickKind::File);
            request.recursive = Some(recursive);
            let script = build_structured(
                &BTreeMap::new(),
                &request,
                search_root,
                out.to_str().unwrap(),
            )
            .unwrap();
            let status = Command::new("/bin/sh")
                .args(["-c", &script])
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .status()
                .unwrap();
            assert!(status.success());
            let selection = fs::read_to_string(&out).unwrap();
            assert_eq!(
                parse_structured_selection(&selection).unwrap(),
                expected.to_str().map(str::to_owned)
            );
            assert_eq!(fs::read_to_string(&expected).unwrap(), "keep");
            assert!(!candidates.exists());
        }
    }
}

#[test]
fn structured_output_rejects_multiple_or_malformed_records() {
    assert_eq!(
        parse_structured_selection("/a\nb\0").unwrap(),
        Some("/a\nb".to_owned())
    );
    assert_eq!(parse_structured_selection("").unwrap(), None);
    for value in ["/a", "relative\0", "/a\0/b\0", "\0"] {
        assert!(parse_structured_selection(value).is_err());
    }
}

#[test]
fn listing_and_fzf_failures_are_not_cancellation() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let out = temp.path().join("selection");
    let mut request = options(PickKind::File);
    request.roots = Some(vec![temp
        .path()
        .join("missing")
        .to_string_lossy()
        .into_owned()]);
    yinz_test_support::write_warm_script(&bin.join("fzf"), "#!/bin/sh\nexit 130\n");
    let run = |request: &PickOptions, path: String| {
        let script = build_structured(
            &BTreeMap::new(),
            request,
            temp.path(),
            out.to_str().unwrap(),
        )
        .unwrap();
        let status = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("PATH", path)
            .status()
            .unwrap()
            .code();
        assert!(!out.with_extension("candidates").exists());
        status
    };
    let path = format!("{}:/usr/bin:/bin", bin.display());
    assert_eq!(run(&request, path.clone()), Some(10));
    request.roots = None;
    assert_eq!(run(&request, path.clone()), Some(0));
    assert_eq!(fs::read(&out).unwrap(), Vec::<u8>::new());
    yinz_test_support::write_warm_script(&bin.join("fzf"), "#!/bin/sh\nexit 2\n");
    assert_eq!(run(&request, path), Some(12));
    fs::remove_file(bin.join("fzf")).unwrap();
    assert_eq!(run(&request, bin.to_string_lossy().into_owned()), Some(11));
}

#[test]
fn termination_signals_remove_candidates() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    yinz_test_support::write_warm_script(
        &bin.join("fzf"),
        "cat >/dev/null\nkill -\"$PICKER_SIGNAL\" \"$PPID\"\n",
    );
    let out = temp.path().join("selection");
    let script = build_structured(
        &BTreeMap::new(),
        &options(PickKind::File),
        temp.path(),
        out.to_str().unwrap(),
    )
    .unwrap();
    for (signal, code) in [("HUP", 129), ("INT", 130), ("TERM", 143)] {
        let mut child = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("PICKER_SIGNAL", signal)
            .spawn()
            .unwrap();
        let status = yinz_test_support::Watched::new(None);
        let finished = status.clone();
        let waiter = std::thread::spawn(move || {
            *finished.lock() = Some(child.wait().unwrap());
        });
        let status = status.wait_until(
            "signalled picker to exit",
            std::time::Duration::from_secs(30),
            |status| *status,
        );
        waiter.join().unwrap();
        assert_eq!(status.code(), Some(code), "{signal}");
        assert!(!out.with_extension("candidates").exists(), "{signal}");
    }
}

#[test]
fn git_listing_preserves_literal_filenames() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir(&repo).unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(&repo)
        .output()
        .unwrap()
        .status
        .success());
    yinz_test_support::write_warm_script(&root.path().join("fzf"), "cat");
    let output = root.path().join("selection");
    let (mut runtime, _) = plugin(json!({}));
    let script = runtime
        .invoke("ff-script", vec![json!({}), json!(output)])
        .unwrap();
    for name in [" leading and trailing ", "a'\"雪,#?%2C.pdf", "a\r"] {
        fs::write(repo.join(name), "").unwrap();
        let result = Command::new("/bin/sh")
            .args(["-c", script.as_str().unwrap()])
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
        let selection = fs::read_to_string(&output).unwrap();
        assert_eq!(
            runtime
                .invoke("ff-selection", vec![json!(selection), json!(false)])
                .unwrap(),
            json!(repo.join(name))
        );
        fs::remove_file(repo.join(name)).unwrap();
    }
}
