use super::*;

#[test]
fn deno_allocation_exhaustion_is_contained_in_subprocesses() {
    const CHILD: &str = "ZELLIJ_DENO_LIMIT_PROBE";
    if let Ok(resource) = std::env::var(CHILD) {
        let limits = Limits::with_budgets("quota-probe.ts".into(), 32 << 20, 16 << 20);
        if resource == "checker" {
            let directory = tempfile::tempdir().unwrap();
            let source: String = (0..100_000)
                .map(|i| format!("export const n{i} = {i};\n"))
                .collect();
            let error = super::super::checker::check_with_limits(
                &directory.path().join("check.ts"),
                Some(&source),
                limits,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("heap budget"), "{error:#}");
            return;
        }
        let mut runtime = limits
            .runtime(RuntimeOptions {
                extensions: vec![super::super::zellij_host::init()],
                ..Default::default()
            })
            .unwrap();
        let script = match resource.as_str() {
            "heap" => "const held = []; while (true) held.push(new Array(32768).fill(42));",
            "buffer" => "const held = []; while (true) held.push(new ArrayBuffer(1024 * 1024));",
            "caught" => "try { new ArrayBuffer(32 * 1024 * 1024); } catch (_) {}",
            "shared" => "const held = []; while (true) held.push(new SharedArrayBuffer(1024 * 1024));",
            "encode" => "const held = []; while (true) held.push(Deno.core.ops.op_zellij_encode('a'.repeat(1024 * 1024)));",
            _ => unreachable!(),
        };
        let result = runtime.execute_script("quota-probe.js", script);
        if resource != "caught" {
            assert!(result.is_err());
        }
        let error = limits.check().unwrap_err().to_string();
        assert!(error.contains("quota-probe.ts"), "{error}");
        assert!(
            error.contains(if resource == "heap" {
                "heap budget"
            } else {
                "backing-store budget"
            }),
            "{error}"
        );
        drop(runtime);
        assert_eq!(limits.live_buffers.load(Ordering::Acquire), 0);
        let healthy = Limits::new("replacement.ts".into());
        let mut replacement = healthy.runtime(RuntimeOptions::default()).unwrap();
        replacement.execute_script("healthy.js", "1 + 1").unwrap();
        return;
    }
    for resource in ["heap", "buffer", "caught", "shared", "encode", "checker"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "deno::limits::tests::deno_allocation_exhaustion_is_contained_in_subprocesses",
                "--nocapture",
            ])
            .env(CHILD, resource)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "{resource}: quota child failed: {status}");
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{resource}: quota subprocess exceeded deadline");
            }
            // Process completion has no Watched sender; bounded OS polling only.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[test]
fn deno_unbudgeted_memory_constructors_are_unavailable() {
    let limits = Limits::new("constructors.ts".into());
    let mut runtime = limits.runtime(RuntimeOptions::default()).unwrap();
    runtime
        .execute_script(
            "constructors.js",
            r#"
        if (typeof WebAssembly !== 'undefined') throw Error('unbudgeted Wasm');
        for (const C of [ArrayBuffer, SharedArrayBuffer, new Uint8Array(1).buffer.constructor]) {
            let refused = false;
            try { new C(1, {maxByteLength: 1024}); } catch (_) { refused = true; }
            if (!refused) throw Error('unbudgeted resizable buffer');
            if (new C(1).byteLength !== 1) throw Error('fixed buffer is broken');
        }
    "#,
        )
        .unwrap();
}

#[test]
fn deno_buffer_accounting_releases_frees_and_checks_overflow() {
    let limits = Limits::with_budgets("allocator.ts".into(), 32 << 20, 1024);
    let buffer = allocate_bytes(&limits, 1024, true);
    assert!(!buffer.is_null());
    unsafe { free(&limits, buffer, 1024) };
    assert_eq!(limits.live_buffers.load(Ordering::Acquire), 0);
    assert!(!limits.reserve(usize::MAX));
    assert!(!limits.reserve(1));
    assert!(limits.check().is_err());
}

#[test]
fn a_host_built_byte_buffer_is_charged_to_the_backing_store_budget_and_released_by_v8() {
    let limits = Limits::with_budgets("host-bytes.ts".into(), 32 << 20, 4 << 20);
    let mut runtime = limits.runtime(RuntimeOptions::default()).unwrap();
    // The runtime's own bootstrap holds a few buffers; only the remainder is ours.
    let baseline = limits.live_buffers.load(Ordering::Acquire);
    let chunk = vec![7u8; 1 << 20];
    let fits = ((4 << 20) - baseline) / chunk.len();
    let mut held = Vec::new();
    let mut refused = None;
    for _ in 0..=fits {
        deno_core::scope!(scope, runtime);
        match counted_bytes(scope, &limits, &chunk) {
            Ok(array) => {
                assert_eq!(array.byte_length(), chunk.len());
                held.push(v8::Global::new(scope, array));
            },
            Err(error) => {
                refused = Some(error.to_string());
                break;
            },
        }
    }
    assert_eq!(
        held.len(),
        fits,
        "every whole 1 MiB chunk under the budget fits"
    );
    assert_eq!(
        limits.live_buffers.load(Ordering::Acquire),
        baseline + fits * chunk.len()
    );
    let refused = refused.expect("the chunk past the budget is refused");
    assert!(refused.contains("backing-store budget"), "{refused}");
    assert!(
        limits.check().is_err(),
        "refusal is permanent, like the guest's own"
    );
    let bytes_seen: usize = {
        deno_core::scope!(scope, runtime);
        let array = v8::Local::new(scope, &held[0]);
        let mut copy = vec![0u8; 8];
        array.copy_contents(&mut copy);
        copy.iter().map(|byte| *byte as usize).sum()
    };
    assert_eq!(bytes_seen, 7 * 8, "the guest sees the host's bytes");
    drop(held);
    drop(runtime);
    assert_eq!(
        limits.live_buffers.load(Ordering::Acquire),
        0,
        "V8 released every host-built store through the counted deleter"
    );
    // An allocation that never reached V8 is released by its own drop.
    let limits = Limits::with_budgets("host-bytes-drop.ts".into(), 32 << 20, 4 << 20);
    drop(CountedAllocation::new(&limits, 1024).unwrap());
    assert_eq!(limits.live_buffers.load(Ordering::Acquire), 0);
}

#[test]
fn native_widgets_release_their_budget_on_collection_and_disposal() {
    let limits = Limits::new("native-ui.ts".into());
    let mut runtime = limits
        .runtime(RuntimeOptions {
            extensions: vec![super::super::zellij_host::init()],
            ..Default::default()
        })
        .unwrap();
    runtime.execute_script("sdk", super::super::SDK).unwrap();
    let baseline = limits.live_buffers.load(Ordering::Acquire);
    runtime.execute_script("widgets", "globalThis.widget = zellij.ui.textInput({value:'日'.repeat(1000)}); globalThis.markdown = zellij.ui.markdown('---\\n\\n'.repeat(100)); markdown.render({width:100,height:10});").unwrap();
    let held = limits.live_buffers.load(Ordering::Acquire);
    assert!(held > baseline + 100_000);
    runtime.v8_isolate().low_memory_notification();
    assert_eq!(limits.live_buffers.load(Ordering::Acquire), held);
    runtime
        .execute_script("drop", "widget = null; markdown = null;")
        .unwrap();
    runtime.v8_isolate().low_memory_notification();
    assert_eq!(limits.live_buffers.load(Ordering::Acquire), baseline);
    runtime
        .execute_script("replacement", "globalThis.widget = zellij.ui.textInput();")
        .unwrap();
    drop(runtime);
    assert_eq!(limits.live_buffers.load(Ordering::Acquire), 0);
}

#[test]
fn native_markdown_cache_is_charged_before_width_amplification() {
    let limits = Limits::with_budgets("native-ui.ts".into(), 32 << 20, 16 << 20);
    let mut runtime = limits
        .runtime(RuntimeOptions {
            extensions: vec![super::super::zellij_host::init()],
            ..Default::default()
        })
        .unwrap();
    runtime.execute_script("sdk", super::super::SDK).unwrap();
    runtime
        .execute_script(
            "markdown",
            "globalThis.md = zellij.ui.markdown('---\\n\\n'.repeat(100));",
        )
        .unwrap();
    assert!(runtime
        .execute_script("cache", "md.render({width:10000,height:1});")
        .is_err());
    assert!(limits.exhausted());
    drop(runtime);
    assert_eq!(limits.live_buffers.load(Ordering::Acquire), 0);
}
