use super::*;

fn project_with_remote(remote: &str) -> Project {
    Project {
        remote: Some(remote.to_owned()),
        ..project()
    }
}

#[test]
fn git_log_is_rooted_bounded_and_can_page() {
    let args =
        Args::parse(r#"{"path":"src","before_commit":"abcdef12","max_results":999}"#).unwrap();
    let target = resolve_in_root(&project(), args.path.as_deref()).unwrap();
    assert_eq!(
        git_argv("git_log", &args, &project(), &target).unwrap(),
        [
            "git",
            "-C",
            "/work/hocus",
            "--no-pager",
            "log",
            "--no-color",
            "--date=iso-strict",
            "--format=%H%x09%aI%x09%an%x09%s",
            "--max-count=500",
            "--skip=1",
            "--end-of-options",
            "abcdef12",
            "--",
            "/work/hocus/src",
        ]
    );
}

#[test]
fn git_show_accepts_only_a_hash_and_opts_into_a_patch() {
    let args = Args::parse(r#"{"commit":"abcdef12","patch":true}"#).unwrap();
    let argv = git_argv("git_show", &args, &project(), "/work/hocus").unwrap();
    assert!(argv.contains(&"--patch".to_owned()));
    assert!(argv.contains(&"--no-ext-diff".to_owned()));
    assert!(argv.contains(&"--no-textconv".to_owned()));
    assert_eq!(argv.last().unwrap(), "/work/hocus");
    for commit in ["HEAD", "--help", "abc", "abcd;touch /tmp/x"] {
        let args = Args::parse(&format!(r#"{{"commit":"{commit}"}}"#)).unwrap();
        assert!(git_argv("git_show", &args, &project(), "/work/hocus").is_err());
    }
}

#[test]
fn git_tools_require_the_active_project_pathspec() {
    let mut nested = project();
    nested.root = "/work/monorepo/packages/hocus".to_owned();
    for (name, arguments) in [("git_log", "{}"), ("git_show", r#"{"commit":"abcdef12"}"#)] {
        let argv = git_argv(
            name,
            &Args::parse(arguments).unwrap(),
            &nested,
            &nested.root,
        )
        .unwrap();
        assert_eq!(
            &argv[argv.len() - 2..],
            ["--", "/work/monorepo/packages/hocus"]
        );
    }
}

#[test]
fn github_remote_spellings_keep_host_owner_and_repository() {
    for remote in [
        "https://github.com/openai/codex.git",
        "git@github.com:openai/codex.git",
        "ssh://git@github.example.com/openai/codex.git",
    ] {
        let (host, owner, repository) = github_repository(Some(remote)).unwrap();
        assert_eq!(owner, "openai");
        assert_eq!(repository, "codex");
        assert!(host.starts_with("github"));
    }
}

#[test]
fn github_read_is_get_only_and_stays_beneath_the_active_repository() {
    let project = project_with_remote("git@github.com:openai/codex.git");
    let args = Args::parse(r#"{"endpoint":"pulls?state=open&per_page=10"}"#).unwrap();
    assert_eq!(
        github_argv(&args, &project).unwrap(),
        [
            "gh",
            "api",
            "--method",
            "GET",
            "--hostname",
            "github.com",
            "repos/openai/codex/pulls?state=open&per_page=10",
        ]
    );
    for endpoint in [
        "../user",
        "pulls//1",
        "%2e%2e/user",
        "https://api.github.com/user",
    ] {
        let args = Args::parse(&format!(r#"{{"endpoint":"{endpoint}"}}"#)).unwrap();
        assert!(github_argv(&args, &project).is_err());
    }
}

#[test]
fn github_read_requires_a_repository_remote() {
    assert!(github_argv(&Args::default(), &project()).is_err());
}
