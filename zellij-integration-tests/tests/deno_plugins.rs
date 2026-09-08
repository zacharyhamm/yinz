#![cfg(unix)]

//! A mixed-runtime session: the real permission prompt, event delivery, reload and shutdown.

use std::fs;
use zellij_cli::args::CliAction;
use zellij_integration_tests::{LayoutInfo, TestRunner, TERMINAL_SIZE};

const TYPESCRIPT: &str = r#"
import { zellij, definePlugin } from "zellij:plugin";
import { label } from "./dependency.ts";
let state: string = "starting";
export default definePlugin({
  async load() {
    zellij.subscribe(["PermissionRequestResult", "Key"]);
    await zellij.waitForPermissions(["ChangeApplicationState"]);
    const ids = zellij.command("GetPluginIds");
    if (typeof ids !== "object" || ids === null || !("plugin_id" in ids)) {
      throw new Error("missing decoded plugin identity");
    }
    const key = zellij.waitForEvent("Key");
    state = `${label} granted; awaiting key`;
    zellij.requestRender();
    await key;
    await zellij.sleep(0);
    zellij.command("RenameTabWithId", [0, `${label} host command`]);
    state = `${label} awaited key rendered`;
    zellij.requestRender();
    // Keep a task awaiting another key when the session closes.
    await zellij.waitForEvent("Key");
  },
  async render() {
    await zellij.sleep(0);
    return state;
  }
});
"#;

#[test]
fn deno_lua_and_wasm_share_a_session_through_permissions_events_reload_and_close() {
    let directory = tempfile::tempdir().unwrap();
    let plugins = directory.path().join("plugins");
    fs::create_dir(&plugins).unwrap();
    fs::write(plugins.join("acceptance.ts"), TYPESCRIPT).unwrap();
    let dependency = plugins.join("dependency.ts");
    fs::write(&dependency, "export const label: string = 'first';").unwrap();
    fs::write(
        plugins.join("companion.lua"),
        r#"
return { render = function() return "Lua companion is alive" end }
"#,
    )
    .unwrap();

    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!("user_plugin_dir \"{}\"", plugins.display()))
        .with_layout(LayoutInfo::Stringified(
            r#"
layout {
  tab name="wasm companion" {
    pane size=1 borderless=true { plugin location="zellij:tab-bar"; }
    pane size=4 { plugin location="file:companion.lua"; }
    pane focus=true { plugin location="file:acceptance.ts"; }
  }
}
"#
            .to_owned(),
        ))
        .start();

    zellij.wait_until(
        "all runtimes loaded and TypeScript requested permission",
        |grid| {
            grid.contains("wasm companion")
                && grid.contains("Lua companion is alive")
                && grid.contains("Allow? (y/n)")
        },
    );
    zellij.send_stdin(b"y");
    zellij.wait_until(
        "the async load resumed after the permission grant",
        |grid| grid.contains("first granted; awaiting key") && !grid.contains("Allow? (y/n)"),
    );
    zellij.send_stdin(b"a");
    zellij.wait_until(
        "the awaited key produced a TS frame and renamed the Wasm tab bar",
        |grid| {
            grid.contains("first awaited key rendered")
                && grid.contains("first host command")
                && grid.contains("Lua companion is alive")
        },
    );

    // Only the dependency changes. The entrypoint and its pane stay the same.
    fs::write(&dependency, "export const label: string = 'second';").unwrap();
    let (code, output) = zellij.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugins.join("acceptance.ts").display()),
        configuration: None,
    });
    assert_eq!(code, 0, "{output}");
    zellij.wait_until(
        "reload imports the changed dependency and retains the permission grant",
        |grid| grid.contains("second granted; awaiting key") && !grid.contains("Allow? (y/n)"),
    );
    zellij.send_stdin(b"b");
    let grid = zellij.wait_until(
        "the replacement instance handles input alongside Lua and Wasm",
        |grid| {
            grid.contains("second awaited key rendered")
                && grid.contains("second host command")
                && grid.contains("Lua companion is alive")
        },
    );
    assert!(!grid.contains("first awaited key rendered"), "{grid}");
    // quit joins the real client and server with the harness's bounded shutdown checks.
    zellij.quit();
}
