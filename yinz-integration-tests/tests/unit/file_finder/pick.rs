use super::*;

#[test]
fn absolute_roots_work_from_an_exited_pane_but_relative_roots_require_a_cwd() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("held-picker.ts"),
        r#"
import z from 'yinz:plugin';
let root, held;
export default {
  async load(config) {
    await z.waitForPermissions(['RunCommands','ReadApplicationState','MessageAndLaunchOtherPlugins','ChangeApplicationState']);
    root = config.root;
    z.subscribe(['CommandPaneExited']);
    held = z.host.openCommandPaneFloating([{path:'/usr/bin/true'}, null, {}]);
  },
  async update(event) {
    if (!held || event.CommandPaneExited?.[0] !== held.Terminal) return false;
    try {
      if (JSON.stringify(z.host.getFocusedPaneInfo().pane_id) !== JSON.stringify(held))
        throw new Error('exited pane is not focused');
      for (const options of [{}, {roots:['.']}, {roots:[root, '.']}]) {
        let rejected = false;
        try { await z.pick({kind:'file', ...options}); }
        catch (error) {
          if (!/CWD|not running/.test(String(error))) throw error;
          rejected = true;
        }
        if (!rejected) throw new Error('relative roots accepted without cwd');
      }
      const file = await z.pick({kind:'file', roots:[root]});
      if (file !== root + '/book.pdf') throw new Error('wrong selection: ' + file);
      if (JSON.stringify(z.host.getFocusedPaneInfo().pane_id) !== JSON.stringify(held))
        throw new Error('focus was not restored');
      z.host.renameTerminalPane([held.Terminal, 'HELD PICK COMPLETE']);
    } catch (error) {
      console.error('HELD PICK ERROR', String(error));
    }
    return false;
  }
};
"#,
    )
    .unwrap();
    let path = root.path().join("held-picker.integration.test.ts");
    std::fs::write(&path, r#"
import {test, session, assert} from 'yinz:testing';
test('absolute roots from an exited process', {grants:['RunCommands','ReadApplicationState','MessageAndLaunchOtherPlugins','ChangeApplicationState']}, async () => {
  const home = session.fixture({'project/book.pdf':''}, {fzf:'cat'});
  session.start();
  await session.waitForText('yinz-test>');
  session.loadPlugin(import.meta.resolve('./held-picker.ts'), {headless:true, configuration:{root:home + '/project'}});
  await session.waitFor('held pane pick completes', () => {
    assert(!session.logs.includes('HELD PICK ERROR'), session.logs);
    return session.panes.some(pane => pane.title === 'HELD PICK COMPLETE' && pane.is_held);
  });
});
"#).unwrap();
    let report = yinz_server::plugin_testing::run(
        &path,
        &Default::default(),
        None,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn awaited_folder_then_pdf_restores_focus_before_second_picker() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("picker.ts");
    std::fs::write(&source, r#"
import z from 'yinz:plugin';
export default {
  async load() {
    if (!z.getPluginInfo().background) throw new Error('load provenance was lost');
    await z.waitForPermissions(['ReadApplicationState','MessageAndLaunchOtherPlugins','ChangeApplicationState']);
    const focused = z.host.getFocusedPaneInfo();
    try {
      const folder = await z.pick({kind:'directory', roots:['/tmp']});
      if (folder === null) throw new Error('first pick cancelled');
      const file = await z.pick({kind:'file', roots:[folder], extensions:['pdf']});
      if (file !== '/tmp/Papers.pdf/quote\n雪.PDF') throw new Error('path changed: ' + file);
      if ('Terminal' in focused.pane_id) z.host.renameTerminalPane([focused.pane_id.Terminal, 'PICK COMPLETE']);
    } catch (error) {
      console.error('PICK ERROR', String(error));
      if ('Terminal' in focused.pane_id) z.host.renameTerminalPane([focused.pane_id.Terminal, 'PICK ERROR ' + String(error)]);
    }
  },
  render() { return 'PICKER BACKGROUND MUST NEVER SHOW'; }
};
"#).unwrap();
    let url = url::Url::from_file_path(source).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            r##"
pane_frame_style "full"
plugins {{
  picker location="{url}"
  file-finder location="yinz:file-finder" {{
    width "60%"
    fzf_args "--border"
    file_command "exit 77"
    openers_json r#"{{"default":{{"argv":["must-not-run"]}}}}"#
  }}
}}
load_plugins clear-defaults=true {{ "picker"; }}
"##
        ))
        .with_layout(yinz_types::data::LayoutInfo::Stringified(
            "layout { pane cwd=\"/tmp\"; }".into(),
        ))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("picker permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("first picker opens", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("Find file")
    });
    let first = yinz.expect_pty_spawn();
    let script = spawned_command(&first).1.remove(1);
    assert!(script.contains("--border"));
    assert!(!script.contains("exit 77"));
    assert!(!script.contains("must-not-run"));
    let out = |script: &str| {
        PathBuf::from(
            script
                .split_once("out='")
                .unwrap()
                .1
                .split_once('\'')
                .unwrap()
                .0,
        )
    };
    std::fs::write(out(&script), "/tmp/Papers.pdf\0").unwrap();
    first.exit(Some(0));
    let second = yinz.expect_pty_spawn();
    let script = spawned_command(&second).1.remove(1);
    assert!(script.contains("/tmp/Papers.pdf"));
    assert!(script.contains("-iname '*.pdf'"));
    second.output(b"SECOND PICKER READY");
    yinz.wait_until("second picker retains focus", |grid| {
        grid.contains("SECOND PICKER READY")
    });
    yinz.send_stdin(b"x");
    second.wait_for_stdin("input reaches the second picker", |bytes| {
        bytes.contains(&b'x')
    });
    std::fs::write(out(&script), "/tmp/Papers.pdf/quote\n雪.PDF\0").unwrap();
    second.exit(Some(0));
    yinz.wait_until("two awaited picks complete", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("PICK COMPLETE")
    });
    assert!(
        !String::from_utf8_lossy(&yinz.raw_bytes()).contains("PICKER BACKGROUND MUST NEVER SHOW")
    );
    yinz.quit();
}

#[test]
fn focused_plugin_can_pick_then_query_plugin_ids_in_the_same_callback() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("focused-picker.ts");
    std::fs::write(&source, r#"
import z from 'yinz:plugin';
let state = 'starting';
export default {
  async load() {
    await z.waitForPermissions(['ReadApplicationState','MessageAndLaunchOtherPlugins','ChangeApplicationState']);
    z.subscribe(['Key']);
    state = 'READY TO PICK';
    z.requestRender();
  },
  async update(event) {
    if (event.Key?.bare_key?.Char !== 'p') return false;
    try {
      const pending = z.pick({kind:'file'});
      const ids = z.host.getPluginIdsForUrl('yinz:file-finder');
      if (ids.length !== 1) throw new Error('picker driver missing');
      if (await pending !== null) throw new Error('expected cancellation');
      state = 'PICK AND QUERY COMPLETE';
    } catch (error) {
      state = 'PICK ERROR ' + String(error);
    }
    return true;
  },
  render() { return state; }
};
"#).unwrap();
    let url = url::Url::from_file_path(source).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_layout(yinz_types::data::LayoutInfo::Stringified(format!(
            "layout {{ pane cwd=\"{}\" {{ plugin location=\"{url}\"; }}; }}",
            root.path().display()
        )))
        .start();
    yinz.wait_until("focused picker permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("focused plugin ready", |grid| {
        grid.contains("READY TO PICK")
    });
    yinz.send_stdin(b"p");
    let picker = yinz.expect_pty_spawn();
    let script = spawned_command(&picker).1.remove(1);
    let out = script
        .split_once("out='")
        .unwrap()
        .1
        .split_once('\'')
        .unwrap()
        .0;
    std::fs::write(out, "").unwrap();
    picker.exit(Some(0));
    yinz.wait_until("pick and synchronous query finish", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("PICK AND QUERY COMPLETE")
    });
    yinz.quit();
}
