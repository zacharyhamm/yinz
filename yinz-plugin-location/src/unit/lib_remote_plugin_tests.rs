use super::*;

#[test]
fn remote_plugin_urls_are_refused() {
    for url in [
        "https://example.com/plugin.wasm",
        "http://example.com/plugin",
        "https://example.com/plugin.ts?v=1",
    ] {
        let error = parse_plugin_location_with_relative(url, None).unwrap_err();
        assert!(
            matches!(error, PluginsConfigError::RemotePluginUnsupported(_)),
            "{url}: {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("remote plugins are not supported") && message.contains(url),
            "{message}"
        );
        // Not mistaken for an alias named after the url.
        assert!(run_plugin_or_alias_from_url(url, &None, None, None).is_err());
    }
}
