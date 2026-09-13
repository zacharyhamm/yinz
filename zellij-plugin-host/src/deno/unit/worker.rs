use super::super::checker;
use super::*;
use zellij_types::authoring::PluginCheckMode;

fn recursive_source(extension: &str, source: String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("deep.{extension}"));
    let error = checker::check_syntax(&path, &source).unwrap_err();
    assert!(
        format!("{error:#}").contains("compilation worker"),
        "{error:#}"
    );
    assert!(checker::check_semantic_source(&path, &source).is_err());
    for mode in [PluginCheckMode::Syntax, PluginCheckMode::Semantic] {
        let report = crate::plugin_authoring::check_source(&path, &source, mode);
        assert!(!report.diagnostics.is_empty());
    }
    checker::check_syntax(&path, "export default {};").unwrap();
    checker::check_semantic_source(&path, "export default {};").unwrap();
    assert!(
        analysis::check(&path, "export default {};", PluginCheckMode::Semantic)
            .diagnostics
            .is_empty()
    );
}

fn nested(prefix: &str, suffix: &str, depth: usize) -> String {
    format!("{}0{}", prefix.repeat(depth), suffix.repeat(depth))
}

#[test]
fn recursive_js_arrays_are_contained() {
    recursive_source(
        "js",
        format!("export default {};", nested("[", "]", 10_000)),
    );
}
#[test]
fn recursive_ts_arrays_are_contained() {
    recursive_source(
        "ts",
        format!("export default {};", nested("[", "]", 10_000)),
    );
}
#[test]
fn recursive_objects_are_contained() {
    recursive_source(
        "js",
        format!("export default {};", nested("{a:", "}", 10_000)),
    );
}
#[test]
fn recursive_unary_expressions_are_contained() {
    recursive_source(
        "js",
        format!("export default {};", nested("!", "", 100_000)),
    );
}
#[test]
fn recursive_binary_chains_are_contained() {
    recursive_source(
        "ts",
        format!("export default {};", nested("1+", "", 100_000)),
    );
}
#[test]
fn recursive_types_are_contained() {
    recursive_source("ts", format!("type Deep = {};", nested("{a:", "}", 10_000)));
}

#[test]
fn semantic_dependency_native_failure_is_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let dep = dir.path().join("dep.ts");
    let main = dir.path().join("main.ts");
    let source = "import './dep.ts'; export default {};";
    std::fs::write(
        &dep,
        format!(
            "export default {}0{};",
            "[".repeat(10_000),
            "]".repeat(10_000)
        ),
    )
    .unwrap();
    let error = checker::check_semantic_source(&main, source).unwrap_err();
    assert!(format!("{error:#}").contains("dep.ts"), "{error:#}");
    std::fs::write(&dep, "export default {};").unwrap();
    checker::check_semantic_source(&main, source).unwrap();
}

#[test]
fn worker_transport_rejects_abnormal_exit_and_invalid_output() {
    for script in [
        "printf garbage",
        "printf '{\"Ok\":{\"Emitted\":{\"text\":\"ok\",\"source_map\":null}}}'; exit 7",
        "kill -KILL $$",
    ] {
        assert!(exchange(Command::new("/bin/sh").args(["-c", script]), b"", DEADLINE).is_err());
    }
    assert!(exchange(&mut Command::new("/does/not/exist"), b"", DEADLINE).is_err());
}

#[test]
fn worker_timeout_reaps_the_child_even_when_input_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let error = exchange(
        Command::new("/bin/sh")
            .args(["-c", "echo $$ > \"$1\"; exec sleep 60", "worker"])
            .arg(&pid_file),
        &vec![b' '; 1024 * 1024],
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(error.to_string().contains("deadline"), "{error:#}");
    let pid: i32 = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[cfg(target_os = "linux")]
#[test]
fn worker_timeout_kills_descendants_that_hold_its_pipes_open() {
    // Act as init for this test so we can inspect the orphan's exit status without
    // depending on the machine's init process to reap it promptly.
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("descendant");
    let result = supervise(
        Command::new("/bin/sh")
            .args(["-c", "sleep 60 & echo $! > \"$1\"; exit 0", "worker"])
            .arg(&pid_file),
        &[],
        Duration::from_secs(2),
    );
    let pid: i32 = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 0, 0, 0, 0) },
        0
    );
    assert!(result.unwrap_err().to_string().contains("deadline"));
    assert!(libc::WIFSIGNALED(status));
    assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
}

#[test]
fn worker_drains_both_pipes_before_accepting_a_response() {
    let output = exchange(Command::new("/bin/sh").args(["-c", "head -c 1000000 /dev/zero >&2; cat >/dev/null; printf '{\"Ok\":{\"Emitted\":{\"text\":\"ok\",\"source_map\":null}}}'"]), &vec![b' '; 1024 * 1024], DEADLINE).unwrap();
    assert!(matches!(output, Output::Emitted { text, .. } if text == "ok"));
}

#[test]
fn worker_frame_limits_cover_requests_and_both_output_streams() {
    assert!(exchange(
        &mut Command::new("/does/not/exist"),
        &vec![0; MAX + 1],
        DEADLINE
    )
    .unwrap_err()
    .to_string()
    .contains("request exceeds"));
    // stdout and stderr use the same bounded reader.
    assert!(drain(&mut std::io::Cursor::new([1]), &mut vec![0; MAX])
        .unwrap_err()
        .to_string()
        .contains("output exceeds"));
}
