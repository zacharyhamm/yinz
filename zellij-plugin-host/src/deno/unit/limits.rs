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
            .args(["--exact", "plugins::deno::limits::tests::deno_allocation_exhaustion_is_contained_in_subprocesses", "--nocapture"])
            .env(CHILD, resource)
            .spawn().unwrap();
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
