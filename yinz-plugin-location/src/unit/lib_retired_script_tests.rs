use super::*;
#[test]
fn lua_urls_fail_before_aliases() {
    for url in [
        "old.lua",
        "/tmp/old.lua",
        "file:old.lua",
        "file:/tmp/old.lua",
        "https://example.com/old.lua",
        "https://example.com/old%2Elua",
    ] {
        assert!(parse_plugin_location(url, None)
            .unwrap_err()
            .to_string()
            .contains("Lua plugins are no longer supported"));
        assert!(run_plugin_or_alias_from_url(url, &None, None, None).is_err());
    }
}
