//! The former script filesystem matrix, through real Scheme guests and the shared dispatcher.
use super::*;
use serde_json::{json, Value};
use std::collections::HashSet;
use yinz_types::data::PluginCommand;

struct Roots {
    host: tempfile::TempDir,
    data: tempfile::TempDir,
    cache: tempfile::TempDir,
    plugins: tempfile::TempDir,
}
impl Roots {
    fn new() -> Self {
        Self {
            host: tempdir().unwrap(),
            data: tempdir().unwrap(),
            cache: tempdir().unwrap(),
            plugins: tempdir().unwrap(),
        }
    }
}
fn permissions() -> HashSet<PermissionType> {
    [PermissionType::ReadFiles, PermissionType::WriteFiles]
        .into_iter()
        .collect()
}
fn run(
    roots: &Roots,
    grants: HashSet<PermissionType>,
    commands: &[Value],
) -> Result<Value, String> {
    // Validate every fixture against the actual command type before the Scheme transport sees it.
    let expressions = commands
        .iter()
        .map(|value| {
            let command: PluginCommand = serde_json::from_value(value.clone()).unwrap();
            let json = serde_json::to_string(&command).unwrap();
            format!(
                "(yinz-command (yinz-json-parse {}))",
                serde_json::to_string(&json).unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let source = format!(
        r#"
      (define (render rows cols)
        (yinz-json-string
          (with-handler (lambda (failure) (hash "Err" (error-object-message failure)))
            (hash "Ok" (begin {expressions})))))
    "#
    );
    let mut env = crate::test_env::test_env();
    env.plugin_cwd = roots.host.path().to_owned();
    env.plugin_own_data_dir = roots.data.path().to_owned();
    env.plugin_own_cache_dir = roots.cache.path().to_owned();
    env.user_plugin_dir = Some(roots.plugins.path().to_owned());
    env.permissions = Arc::new(std::sync::Mutex::new(Some(grants)));
    let path = roots.host.path().join("filesystem.scm");
    let mut plugin = crate::steel::SteelPlugin::new(&source, &path, env, Arc::default()).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    serde_json::from_slice(&plugin.render(1, 400).unwrap()).unwrap()
}
fn write(path: impl AsRef<str>, contents: &str, append: bool) -> Value {
    json!({"FsWrite":{"path":path.as_ref(),"contents":contents,"append":append,"create_dirs":false}})
}
fn read(path: impl AsRef<str>) -> Value {
    json!({"FsRead":{"path":path.as_ref(),"max_bytes":null,"offset":null}})
}
fn remove(path: impl AsRef<str>, recursive: bool) -> Value {
    json!({"FsRemove":{"path":path.as_ref(),"recursive":recursive}})
}
fn rename(from: impl AsRef<str>, to: impl AsRef<str>) -> Value {
    json!({"FsRename":{"from":from.as_ref(),"to":to.as_ref()}})
}
fn metadata(path: impl AsRef<str>, follow: bool) -> Value {
    json!({"FsMetadata":{"path":path.as_ref(),"follow_symlinks":follow}})
}
fn text(value: Value) -> String {
    String::from_utf8(serde_json::from_value(value).unwrap()).unwrap()
}

#[test]
fn steel_filesystem_roots_are_distinct_and_relative_paths_use_host() {
    let roots = Roots::new();
    let temporary = format!("steel-where-{}.txt", std::process::id());
    let tmp = format!("/tmp/{temporary}");
    let result = run(
        &roots,
        permissions(),
        &[
            write("/host/where", "host", false),
            write("/data/where", "data", false),
            write("/cache/where", "cache", false),
            write(&tmp, "tmp", false),
            read("relative"),
        ],
    );
    assert!(result.is_err());
    for (directory, expected) in [
        (roots.host.path(), "host"),
        (roots.data.path(), "data"),
        (roots.cache.path(), "cache"),
    ] {
        assert_eq!(
            std::fs::read_to_string(directory.join("where")).unwrap(),
            expected
        );
    }
    let path = yinz_runtime::consts::YINZ_TMP_DIR.join(temporary);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "tmp");
    std::fs::remove_file(path).unwrap();
    std::fs::write(roots.host.path().join("relative"), "found").unwrap();
    assert_eq!(
        text(run(&roots, permissions(), &[read("relative")]).unwrap()),
        "found"
    );
}

#[test]
fn steel_filesystem_rejects_parent_traversal_from_every_root() {
    let roots = Roots::new();
    for command in [
        read("/host/../../../../etc/passwd"),
        read("/data/../../../../etc/passwd"),
        write("/host/../escaped.txt", "x", false),
    ] {
        let error = run(&roots, permissions(), &[command]).unwrap_err();
        assert!(
            error.contains("outside the plugin's own folders"),
            "{error}"
        );
    }
    assert!(!roots
        .host
        .path()
        .parent()
        .unwrap()
        .join("escaped.txt")
        .exists());
}

#[cfg(unix)]
#[test]
fn steel_filesystem_removing_symlinks_preserves_targets() {
    for recursive in [false, true] {
        let roots = Roots::new();
        std::fs::create_dir(roots.host.path().join("target")).unwrap();
        std::fs::write(roots.host.path().join("target/keep"), "kept").unwrap();
        for target in ["target", "target/keep", "missing"] {
            let link = roots.host.path().join("link");
            std::os::unix::fs::symlink(target, &link).unwrap();
            run(&roots, permissions(), &[remove("/host/link", recursive)]).unwrap();
            assert!(std::fs::symlink_metadata(&link).is_err());
            assert_eq!(
                std::fs::read_to_string(roots.host.path().join("target/keep")).unwrap(),
                "kept"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn steel_filesystem_renaming_symlinks_preserves_targets() {
    let roots = Roots::new();
    std::fs::create_dir(roots.host.path().join("target")).unwrap();
    std::fs::write(roots.host.path().join("target/keep"), "kept").unwrap();
    for target in ["target", "target/keep", "missing"] {
        let link = roots.host.path().join("link");
        let moved = roots.host.path().join("moved");
        std::os::unix::fs::symlink(target, &link).unwrap();
        run(
            &roots,
            permissions(),
            &[rename("/host/link", "/host/moved")],
        )
        .unwrap();
        assert!(std::fs::symlink_metadata(link).is_err());
        assert_eq!(std::fs::read_link(&moved).unwrap(), PathBuf::from(target));
        assert_eq!(
            std::fs::read_to_string(roots.host.path().join("target/keep")).unwrap(),
            "kept"
        );
        std::fs::remove_file(moved).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn steel_filesystem_rename_replaces_destination_link() {
    let roots = Roots::new();
    std::fs::write(roots.host.path().join("target"), "kept").unwrap();
    std::fs::write(roots.host.path().join("source"), "replacement").unwrap();
    std::os::unix::fs::symlink("target", roots.host.path().join("link")).unwrap();
    run(
        &roots,
        permissions(),
        &[rename("/host/source", "/host/link")],
    )
    .unwrap();
    assert!(!roots.host.path().join("source").exists());
    assert!(!roots.host.path().join("link").is_symlink());
    assert_eq!(
        std::fs::read_to_string(roots.host.path().join("link")).unwrap(),
        "replacement"
    );
    assert_eq!(
        std::fs::read_to_string(roots.host.path().join("target")).unwrap(),
        "kept"
    );
}

#[cfg(unix)]
#[test]
fn steel_filesystem_metadata_distinguishes_links_and_follows_only_inside_root() {
    let roots = Roots::new();
    std::fs::write(roots.host.path().join("target"), "kept").unwrap();
    for target in ["target", "missing"] {
        let link = roots.host.path().join("link");
        std::os::unix::fs::symlink(target, &link).unwrap();
        let result = run(&roots, permissions(), &[metadata("/host/link", false)]).unwrap();
        assert_eq!(
            (
                result["is_symlink"].clone(),
                result["is_file"].clone(),
                result["is_dir"].clone()
            ),
            (json!(true), json!(false), json!(false))
        );
        if target == "target" {
            let result = run(&roots, permissions(), &[metadata("/host/link", true)]).unwrap();
            assert_eq!(
                (result["is_symlink"].clone(), result["is_file"].clone()),
                (json!(false), json!(true))
            );
        }
        std::fs::remove_file(link).unwrap();
    }
    assert_eq!(
        run(&roots, permissions(), &[metadata("/host", false)]).unwrap()["is_dir"],
        json!(true)
    );
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), roots.host.path().join("escape")).unwrap();
    for command in [metadata("/host/escape", true), read("/host/escape/secret")] {
        let error = run(&roots, permissions(), &[command]).unwrap_err();
        assert!(
            error.contains("outside the plugin's own folders"),
            "{error}"
        );
    }
}

#[cfg(unix)]
#[test]
fn steel_filesystem_dangling_symlink_cannot_create_outside_root() {
    let roots = Roots::new();
    let outside = tempdir().unwrap();
    let target = outside.path().join("must-not-be-created");
    let link = roots.host.path().join("escape");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    for path in ["/host/escape", "/host/escape/", "/host/escape/child"] {
        for append in [false, true] {
            let mut command = write(path, "escaped", append);
            command["FsWrite"]["create_dirs"] = json!(true);
            let error = run(&roots, permissions(), &[command]).unwrap_err();
            assert!(error.contains("unresolved symlink"), "{error}");
            assert!(!target.exists());
            assert!(link.is_symlink());
        }
    }
}

#[cfg(unix)]
#[test]
fn steel_filesystem_refuses_fifos_and_preserves_regular_write_modes() {
    use std::os::unix::fs::OpenOptionsExt;
    let roots = Roots::new();
    let fifo = roots.host.path().join("fifo");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    for append in [false, true] {
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        let error = run(
            &roots,
            permissions(),
            &[write("/host/fifo", "data", append)],
        )
        .unwrap_err();
        assert!(error.contains("not a regular file"), "{error}");
        drop(reader);
        assert!(run(
            &roots,
            permissions(),
            &[write("/host/fifo", "data", append)]
        )
        .is_err());
    }
    assert_eq!(
        text(
            run(
                &roots,
                permissions(),
                &[
                    write("/host/file", "original", false),
                    write("/host/file", "new", false),
                    write("/host/file", "+append", true),
                    read("/host/file")
                ]
            )
            .unwrap()
        ),
        "new+append"
    );
}

#[test]
fn steel_filesystem_plugin_writes_need_author_permission_and_rename_cannot_escape() {
    let roots = Roots::new();
    let command = write(
        "/plugins/keys.scm",
        "(define (render rows cols) \"hello\")",
        false,
    );
    let error = run(&roots, permissions(), std::slice::from_ref(&command)).unwrap_err();
    assert!(error.contains("AuthorPlugins"), "{error}");
    assert!(!roots.plugins.path().join("keys.scm").exists());
    let mut grants = permissions();
    grants.insert(PermissionType::AuthorPlugins);
    run(&roots, grants, &[command]).unwrap();
    assert_eq!(
        std::fs::read_to_string(roots.plugins.path().join("keys.scm")).unwrap(),
        "(define (render rows cols) \"hello\")"
    );
    std::fs::write(roots.host.path().join("source"), "x").unwrap();
    let outside = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let target = outside.path().join("stolen");
    let error = run(
        &roots,
        permissions(),
        &[rename("/host/source", target.to_str().unwrap())],
    )
    .unwrap_err();
    assert!(error.contains("FullHdAccess"), "{error}");
    assert!(roots.host.path().join("source").exists() && !target.exists());
}

#[cfg(unix)]
#[test]
fn steel_filesystem_full_disk_permission_allows_absolute_io_and_preserves_link_entries() {
    let roots = Roots::new();
    // /tmp is a virtual root: a directory in this checkout is outside every virtual root.
    let outside = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let target = outside.path().join("target");
    let link = outside.path().join("link");
    std::fs::write(&target, "original").unwrap();
    std::os::unix::fs::symlink("target", &link).unwrap();
    let path = link.to_str().unwrap();
    let error = run(&roots, permissions(), &[read(path)]).unwrap_err();
    assert!(error.contains("FullHdAccess"), "{error}");
    let mut grants = permissions();
    grants.insert(PermissionType::FullHdAccess);
    assert_eq!(
        text(run(&roots, grants.clone(), &[read(path)]).unwrap()),
        "original"
    );
    run(
        &roots,
        grants.clone(),
        &[write(path, "original+updated", false)],
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "original+updated"
    );
    assert_eq!(
        run(&roots, grants.clone(), &[metadata(path, false)]).unwrap()["is_symlink"],
        json!(true)
    );
    run(&roots, grants, &[remove(path, false)]).unwrap();
    assert!(!link.is_symlink());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original+updated");
}

#[test]
fn steel_project_configuration_requires_write_permission_and_revision() {
    let roots = Roots::new();
    assert!(std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(roots.host.path())
        .output()
        .unwrap()
        .status
        .success());
    std::fs::create_dir(roots.host.path().join(".yinz")).unwrap();
    let path = roots.host.path().join(".yinz/agent-exit.kdl");
    let source = "agent_exit { default_branch \"main\"; }";
    let replacement = "agent_exit { default_branch \"trunk\"; }";
    std::fs::write(&path, source).unwrap();
    let readonly = [PermissionType::ReadFiles]
        .into_iter()
        .collect::<HashSet<_>>();
    assert_eq!(
        run(
            &roots,
            readonly.clone(),
            &[json!({"ProjectConfigRead":{"directory":"."}})]
        )
        .unwrap()["source"],
        json!(source)
    );
    let command =
        json!({"ProjectConfigWrite":{"directory":".","source":replacement,"expected":source}});
    let error = run(&roots, readonly, std::slice::from_ref(&command)).unwrap_err();
    assert!(error.contains("WriteFiles"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    assert_eq!(
        run(&roots, permissions(), std::slice::from_ref(&command)).unwrap()["source"],
        json!(replacement)
    );
    assert!(run(&roots, permissions(), &[command]).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), replacement);
}

#[test]
fn steel_plugin_check_syntax_is_available_without_semantic_permissions() {
    let roots = Roots::new();
    let check = |source: &str, mode| json!({"CheckPlugin":{"name":"check.scm","source":source,"mode":mode}});
    let no_grants = HashSet::new();
    assert!(run(&roots, no_grants.clone(), &[read("/host/input")])
        .unwrap_err()
        .contains("ReadFiles"));
    let error = run(
        &roots,
        no_grants.clone(),
        &[check("(define (render rows cols) \"ok\")", "Semantic")],
    )
    .unwrap_err();
    assert!(error.contains("ReadFiles"), "{error}");
    let valid = run(
        &roots,
        no_grants.clone(),
        &[check("(define (render rows cols) \"ok\")", "Syntax")],
    )
    .unwrap();
    assert_eq!(valid["diagnostics"], json!([]));
    let invalid = run(&roots, no_grants, &[check("(define (render", "Syntax")]).unwrap();
    assert!(!invalid["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn steel_filesystem_binary_directory_operations_and_escape_endpoints_are_confined() {
    let roots = Roots::new();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "unchanged").unwrap();
    std::os::unix::fs::symlink(outside.path(), roots.host.path().join("link")).unwrap();
    let bytes = vec![0_u8, 128, 255, 10];
    let binary = json!({"FsWrite":{"path":"/host/files/nested/b","contents":bytes,"append":false,"create_dirs":false}});
    let result = run(
        &roots,
        permissions(),
        &[
            json!({"FsCreateDir":{"path":"/host/files/nested","all":true}}),
            binary,
            read("/host/files/nested/b"),
        ],
    )
    .unwrap();
    assert_eq!(result, json!(bytes));
    assert!(run(
        &roots,
        permissions(),
        &[json!({"FsRead":{"path":"/host/files/nested/b","max_bytes":2,"offset":null}})]
    )
    .is_err());
    assert_eq!(
        text(
            run(
                &roots,
                permissions(),
                &[
                    write("/host/files/nested/a", "hé🙂", false),
                    write("/host/files/nested/a", "!", true),
                    read("/host/files/nested/a"),
                ]
            )
            .unwrap()
        ),
        "hé🙂!"
    );
    let entries = run(
        &roots,
        permissions(),
        &[json!({"FsList":{"path":"/host/files/nested"}})],
    )
    .unwrap();
    let paths = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["/host/files/nested/a", "/host/files/nested/b"]);
    assert_eq!(
        run(
            &roots,
            permissions(),
            &[metadata("/host/files/missing", true)]
        )
        .unwrap(),
        Value::Null
    );
    assert_eq!(
        run(
            &roots,
            permissions(),
            &[metadata("/host/files/nested/b", true)]
        )
        .unwrap()["len"],
        json!(4)
    );
    for path in [
        "/host/../escape",
        "/host/link/secret",
        "/host/link/new/nested",
    ] {
        for command in [
            read(path),
            json!({"FsRead":{"path":path,"offset":0,"max_bytes":1}}),
            json!({"FsWrite":{"path":path,"contents":"bad","append":false,"create_dirs":true}}),
            rename("/host/files/nested/b", path),
            rename(path, "/host/files/from-outside"),
        ] {
            assert!(
                run(&roots, permissions(), &[command]).is_err(),
                "escaped via {path}"
            );
        }
    }
    assert_eq!(
        run(
            &roots,
            permissions(),
            &[
                rename("/host/files/nested/b", "/host/files/moved"),
                read("/host/files/moved"),
            ]
        )
        .unwrap(),
        json!(bytes)
    );
    assert_eq!(
        run(
            &roots,
            permissions(),
            &[metadata("/host/files/nested/b", true)]
        )
        .unwrap(),
        Value::Null
    );
    assert_eq!(
        run(
            &roots,
            permissions(),
            &[remove("/host/files", true), metadata("/host/files", true)]
        )
        .unwrap(),
        Value::Null
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("secret")).unwrap(),
        "unchanged"
    );
    assert!(!outside.path().join("new").exists());
}
