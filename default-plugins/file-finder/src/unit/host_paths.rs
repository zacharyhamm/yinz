use super::*;

#[test]
fn host_paths_preserve_lexical_spelling_under_the_root_mount() {
    for (host, plugin) in [
        ("/", "/host/"),
        ("/tmp", "/host/tmp"),
        ("/tmp/a", "/host/tmp/a"),
        ("/data", "/host/data"),
        ("/data/a", "/host/data/a"),
        ("/cache", "/host/cache"),
        ("/cache/a", "/host/cache/a"),
        ("/host", "/host/host"),
        ("/host/a", "/host/host/a"),
        ("/home/user/a", "/host/home/user/a"),
        ("/plugins/a", "/host/plugins/a"),
        ("/tmp-other/a", "/host/tmp-other/a"),
        ("/private/tmp/a", "/host/private/tmp/a"),
        ("/var/a", "/host/var/a"),
        ("/private/var/a", "/host/private/var/a"),
        ("/tmp/sub/book, one.pdf", "/host/tmp/sub/book, one.pdf"),
    ] {
        assert_eq!(
            project::host_path(Path::new(HOST_MOUNT), Path::new(host)),
            Path::new(plugin)
        );
    }
}
