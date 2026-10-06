use super::*;

#[test]
fn executable_paths_expand_only_leading_home_relative_paths() {
    for (path, home, expected) in [
        (
            "~/.local/bin/yinz",
            Some("/home/session"),
            "/home/session/.local/bin/yinz",
        ),
        ("~/", Some(" /home/session/// "), "/home/session/"),
        ("~/bin/yinz", Some("/"), "/bin/yinz"),
        (
            "~/bin with spaces/yinz",
            Some("/home/session user"),
            "/home/session user/bin with spaces/yinz",
        ),
        ("~/bin/yinz", None, "~/bin/yinz"),
        ("~/bin/yinz", Some(""), "~/bin/yinz"),
        ("~/bin/yinz", Some(" \t "), "~/bin/yinz"),
        (
            "/usr/local/bin/yinz",
            Some("/home/session"),
            "/usr/local/bin/yinz",
        ),
        ("yinz", Some("/home/session"), "yinz"),
        ("~other/bin/yinz", Some("/home/session"), "~other/bin/yinz"),
    ] {
        let env = home
            .map(|home| BTreeMap::from([("HOME".into(), home.into())]))
            .unwrap_or_default();
        assert_eq!(expand_home(path, &env), expected, "{path}, HOME={home:?}");
    }
}
