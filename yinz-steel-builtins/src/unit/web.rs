//! Static checks for the builtin web app's shipped asset tree.
//!
//! These run without Node so the asset tree, its import graph, the vendored
//! libraries and the HTML sinks are verified by every `cargo x test`.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{asset_dir, builtin_path, install};

/// Extensions the asset loader serves and the builtin generation retains.
const SERVED_EXTENSIONS: &[&str] = &[
    "js",
    "mjs",
    "css",
    "svg",
    "png",
    "ico",
    "webmanifest",
    "json",
    "woff2",
    "txt",
];
/// Installed but deliberately unserved provenance records.
const UNSERVED_RECORDS: &[&str] = &["vendor/VENDOR.md"];
const MAX_DIRECTORY_DEPTH: usize = 6;
const MAX_SERVED_FILES: usize = 256;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
/// Module scripts the page templates load.
const ENTRY_MODULES: &[&str] = &["app.js", "login.js"];
/// JavaScript files intentionally shipped without being imported from an entry module.
const UNREACHABLE_MODULES: &[&str] = &[];

fn static_dir() -> PathBuf {
    asset_dir().join("web/static")
}

fn web_app_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../yinz-steel/src/web_app.scm");
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"))
}

/// Every regular file beneath `root`, as sorted `/`-separated relative paths.
fn tree_files(root: &Path) -> Vec<String> {
    fn walk(root: &Path, directory: &Path, files: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                walk(root, &entry.path(), files);
            } else {
                assert!(kind.is_file(), "not a regular file: {:?}", entry.path());
                let relative = entry.path().strip_prefix(root).unwrap().to_owned();
                files.push(relative.to_str().unwrap().replace('\\', "/"));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    files
}

fn read_static(relative: &str) -> String {
    let path = static_dir().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"))
}

fn extension(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
}

fn script_files() -> Vec<String> {
    tree_files(&static_dir())
        .into_iter()
        .filter(|path| matches!(extension(path), "js" | "mjs"))
        .collect()
}

/// Scripts written for this app, excluding the audited vendor directory.
fn application_scripts() -> Vec<String> {
    script_files()
        .into_iter()
        .filter(|path| !path.starts_with("vendor/"))
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

// ------------------------------------------------------------- registry/install

#[test]
fn web_is_a_registered_builtin_that_installs_its_whole_tree() {
    assert!(yinz_runtime::consts::BUILTIN_PLUGIN_NAMES.contains(&"web"));
    let source = asset_dir();
    assert!(source.join("web.scm").is_file());
    assert!(source.join("web.test.scm").is_file());
    let data = tempfile::tempdir().unwrap();
    install(&source, data.path()).unwrap();
    let installed = data.path().join("plugins/builtins");
    assert_eq!(
        builtin_path(data.path(), "web").unwrap(),
        installed.join("web.scm")
    );
    let mut shipped = vec!["web.scm".to_owned(), "web.test.scm".to_owned()];
    shipped.extend(
        tree_files(&source.join("web"))
            .into_iter()
            .map(|path| format!("web/{path}")),
    );
    assert!(shipped.contains(&"web/static/app.js".to_owned()));
    for path in &shipped {
        assert_eq!(
            std::fs::read(installed.join(path)).unwrap(),
            std::fs::read(source.join(path)).unwrap(),
            "installed {path} differs from the shipped file"
        );
    }
    assert_eq!(
        tree_files(&installed.join("web")),
        tree_files(&source.join("web"))
    );
}

// ---------------------------------------------------------------- tree checks

#[test]
fn static_tree_serves_only_allowlisted_files_within_limits() {
    let root = static_dir();
    let mut served = 0;
    let mut total = 0;
    for path in tree_files(&root) {
        let depth = path.matches('/').count();
        assert!(depth <= MAX_DIRECTORY_DEPTH, "{path} is too deep");
        if UNSERVED_RECORDS.contains(&path.as_str()) {
            continue;
        }
        assert!(
            SERVED_EXTENSIONS.contains(&extension(&path)),
            "{path} would be installed but never served"
        );
        let length = std::fs::metadata(root.join(&path)).unwrap().len();
        assert!(length <= MAX_FILE_BYTES, "{path} exceeds the file limit");
        served += 1;
        total += length;
    }
    assert!(served <= MAX_SERVED_FILES, "{served} served files");
    assert!(total <= MAX_TOTAL_BYTES, "{total} served bytes");
    for record in UNSERVED_RECORDS {
        assert!(root.join(record).is_file(), "missing record {record}");
    }
}

/// Map an absolute `/static/...` URL path to the static tree, failing on escapes.
fn static_url_exists(url: &str) -> bool {
    let Some(relative) = url.strip_prefix("/static/") else {
        return false;
    };
    let relative = Path::new(relative);
    relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
        && static_dir().join(relative).is_file()
}

#[test]
fn page_and_manifest_references_exist() {
    let source = web_app_source();
    let literal = Regex::new(r#""(/static/[^"]*)""#).unwrap();
    let mut referenced = BTreeSet::new();
    for capture in literal.captures_iter(&source) {
        let url = &capture[1];
        if url == "/static/" {
            continue; // The asset mount point itself.
        }
        assert!(
            static_url_exists(url),
            "web_app.scm references missing {url}"
        );
        referenced.insert(url.to_owned());
    }
    for expected in [
        "/static/app.js",
        "/static/login.js",
        "/static/app.css",
        "/static/manifest.webmanifest",
        "/static/icon.svg",
    ] {
        assert!(
            referenced.contains(expected),
            "pages no longer load {expected}"
        );
    }
    assert!(source.contains(r#"(yinz-web-assets "/static/" "builtin:web/static")"#));
    // Every `/static/` literal inside the static tree must exist too.
    let embedded = Regex::new(r#"(/static/[^"'`)\s]+)"#).unwrap();
    for path in tree_files(&static_dir()) {
        if !matches!(extension(&path), "js" | "css" | "webmanifest") {
            continue;
        }
        let text = read_static(&path);
        for capture in embedded.captures_iter(&text) {
            assert!(
                static_url_exists(&capture[1]),
                "{path} references missing {}",
                &capture[1]
            );
        }
    }
}

fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
    assert_eq!(&bytes[12..16], b"IHDR", "PNG must start with IHDR");
    let word = |offset: usize| u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
    (word(16), word(20))
}

#[test]
fn manifest_icons_match_files_and_install_without_a_service_worker() {
    let manifest: Value = serde_json::from_str(&read_static("manifest.webmanifest")).unwrap();
    assert_eq!(manifest["display"], "standalone");
    assert_eq!(manifest["start_url"], "/");
    assert_eq!(manifest["scope"], "/");
    // start_url is the app page the builtin publishes.
    assert!(web_app_source().contains(r#"(yinz-web-page "/" "#));
    assert!(
        manifest
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.to_ascii_lowercase().contains("serviceworker")),
        "manifest must not declare a service worker"
    );
    let icons = manifest["icons"].as_array().unwrap();
    assert!(icons.len() >= 3);
    for icon in icons {
        let src = icon["src"].as_str().unwrap();
        assert!(static_url_exists(src), "missing icon {src}");
        let relative = src.strip_prefix("/static/").unwrap();
        let declared = icon["type"].as_str().unwrap();
        let sizes = icon["sizes"].as_str().unwrap();
        match extension(relative) {
            "png" => {
                assert_eq!(declared, "image/png", "{src}");
                let (width, height) =
                    png_dimensions(&std::fs::read(static_dir().join(relative)).unwrap());
                assert_eq!(sizes, format!("{width}x{height}"), "{src}");
            },
            "svg" => {
                assert_eq!(declared, "image/svg+xml", "{src}");
                assert_eq!(sizes, "any", "{src}");
            },
            other => panic!("unexpected icon type {other} for {src}"),
        }
    }
    for path in script_files() {
        assert!(
            !read_static(&path).contains("serviceWorker"),
            "{path} registers a service worker"
        );
    }
}

// --------------------------------------------------------------- import graph

/// Module specifiers in static/dynamic imports and re-exports, including minified code.
fn module_specifiers(source: &str) -> Vec<String> {
    let patterns = [
        // `import x from "y"`, `export { x } from "y"`, minified `from"y"`.
        r#"\bfrom\s*(?:'([^'\n]*)'|"([^"\n]*)")"#,
        // Side-effect `import "y"`.
        r#"\bimport\s*(?:'([^'\n]*)'|"([^"\n]*)")"#,
        // Dynamic `import("y")`; a non-literal argument captures nothing and fails below.
        r#"\bimport\s*\(\s*(?:'([^'\n]*)'|"([^"\n]*)"|`([^`\n]*)`)?"#,
    ];
    let mut specifiers = Vec::new();
    for pattern in patterns {
        for capture in Regex::new(pattern).unwrap().captures_iter(source) {
            let specifier = capture.iter().skip(1).flatten().next().map_or_else(
                || "<non-literal dynamic import>".to_owned(),
                |m| m.as_str().to_owned(),
            );
            specifiers.push(specifier);
        }
    }
    specifiers
}

/// Resolve a relative specifier from `importer`, both relative to static/.
fn resolve_specifier(importer: &str, specifier: &str) -> Result<String, String> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return Err(format!("{importer} imports non-relative {specifier:?}"));
    }
    let mut parts: Vec<&str> = importer.split('/').collect();
    parts.pop();
    for part in specifier.split('/') {
        match part {
            "." => {},
            ".." => {
                parts
                    .pop()
                    .ok_or_else(|| format!("{importer} imports {specifier:?} outside static/"))?;
            },
            "" => return Err(format!("{importer} imports malformed {specifier:?}")),
            part => parts.push(part),
        }
    }
    let resolved = parts.join("/");
    if static_dir().join(&resolved).is_file() {
        Ok(resolved)
    } else {
        Err(format!("{importer} imports missing {specifier:?}"))
    }
}

/// Every module reachable from `entries`, mapped to its resolved imports.
fn import_graph(entries: &[&str]) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut graph = BTreeMap::new();
    let mut pending: Vec<String> = entries.iter().map(|entry| (*entry).to_owned()).collect();
    while let Some(module) = pending.pop() {
        if graph.contains_key(&module) {
            continue;
        }
        let imports = module_specifiers(&read_static(&module))
            .iter()
            .map(|specifier| resolve_specifier(&module, specifier))
            .collect::<Result<Vec<_>, _>>()?;
        pending.extend(imports.iter().cloned());
        graph.insert(module, imports);
    }
    Ok(graph)
}

#[test]
fn import_graph_is_relative_confined_and_complete() {
    let graph = import_graph(ENTRY_MODULES).unwrap();
    // The vendored hooks build must import Preact through its rewritten specifier.
    assert_eq!(graph["vendor/hooks.module.js"], ["vendor/preact.module.js"]);
    assert!(graph["views/ui.js"].contains(&"vendor/htm.module.js".to_owned()));
    let reachable: BTreeSet<&str> = graph.keys().map(String::as_str).collect();
    for path in script_files() {
        assert!(
            reachable.contains(path.as_str()) || UNREACHABLE_MODULES.contains(&path.as_str()),
            "{path} is shipped but not imported from {ENTRY_MODULES:?}"
        );
    }
}

#[test]
fn import_parser_rejects_unsafe_specifiers() {
    assert_eq!(
        module_specifiers(
            "import a from './a.js';\nexport { b } from \"../b.js\";\nimport './c.js';\n\
             const d = import('./d.js');\nimport{x as y}from\"./e.js\";"
        ),
        ["./a.js", "../b.js", "./e.js", "./c.js", "./d.js"]
    );
    assert_eq!(
        module_specifiers("const e = import(name);"),
        ["<non-literal dynamic import>"]
    );
    for specifier in [
        "preact",
        "https://example.com/x.js",
        "//example.com/x.js",
        "/static/app.js",
        "../../escape.js",
        "./missing.js",
        "<non-literal dynamic import>",
    ] {
        assert!(
            resolve_specifier("views/ui.js", specifier).is_err(),
            "{specifier} must be rejected"
        );
    }
    assert_eq!(
        resolve_specifier("views/ui.js", "../vendor/preact.module.js").unwrap(),
        "vendor/preact.module.js"
    );
}

// ---------------------------------------------------------- vendor provenance

struct VendorRecord {
    packages: BTreeMap<String, (String, String)>,
    shipped: BTreeMap<String, String>,
}

/// Cells of every Markdown table row whose first cell is not a header or rule.
fn table_rows(markdown: &str) -> Vec<Vec<String>> {
    markdown
        .lines()
        .filter(|line| line.starts_with('|') && !line.starts_with("|---"))
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().trim_matches('`').to_owned())
                .collect()
        })
        .collect()
}

fn parse_vendor_record(markdown: &str) -> VendorRecord {
    let mut packages = BTreeMap::new();
    let mut shipped = BTreeMap::new();
    for row in table_rows(markdown) {
        match row.as_slice() {
            [name, version, _license, tarball, _, _] if name != "Package" => {
                packages.insert(name.clone(), (version.clone(), tarball.clone()));
            },
            [file, _source, _upstream, digest] if file != "Shipped file" => {
                shipped.insert(file.clone(), digest.clone());
            },
            _ => {},
        }
    }
    VendorRecord { packages, shipped }
}

/// Check every file in a vendor directory against the digests in its VENDOR.md.
fn verify_vendor_directory(directory: &Path) -> Result<VendorRecord, String> {
    let markdown = std::fs::read_to_string(directory.join("VENDOR.md"))
        .map_err(|error| format!("read VENDOR.md: {error}"))?;
    let record = parse_vendor_record(&markdown);
    let present: BTreeSet<String> = tree_files(directory)
        .into_iter()
        .filter(|file| file != "VENDOR.md")
        .collect();
    let recorded: BTreeSet<String> = record.shipped.keys().cloned().collect();
    if present != recorded {
        return Err(format!(
            "vendor files {present:?} differ from VENDOR.md {recorded:?}"
        ));
    }
    for (file, expected) in &record.shipped {
        let bytes =
            std::fs::read(directory.join(file)).map_err(|error| format!("{file}: {error}"))?;
        let actual = sha256_hex(&bytes);
        if &actual != expected {
            return Err(format!(
                "{file} has SHA-256 {actual}, VENDOR.md records {expected}"
            ));
        }
    }
    Ok(record)
}

#[test]
fn vendored_libraries_match_their_recorded_provenance() {
    let record = verify_vendor_directory(&static_dir().join("vendor")).unwrap();
    let packages: Vec<(&str, &str, &str)> = record
        .packages
        .iter()
        .map(|(name, (version, tarball))| (name.as_str(), version.as_str(), tarball.as_str()))
        .collect();
    assert_eq!(
        packages,
        [
            (
                "htm",
                "3.1.1",
                "https://registry.npmjs.org/htm/-/htm-3.1.1.tgz"
            ),
            (
                "preact",
                "10.29.8",
                "https://registry.npmjs.org/preact/-/preact-10.29.8.tgz"
            ),
        ]
    );
    let recorded: Vec<&str> = record.shipped.keys().map(String::as_str).collect();
    assert_eq!(
        recorded,
        [
            "LICENSE-htm.txt",
            "LICENSE-preact.txt",
            "hooks.module.js",
            "htm.module.js",
            "preact.module.js"
        ]
    );
    assert!(read_static("vendor/LICENSE-preact.txt").contains("The MIT License"));
    assert!(read_static("vendor/LICENSE-htm.txt").contains("Apache License"));
}

#[test]
fn vendor_verification_rejects_tampered_and_unrecorded_files() {
    let copy = |name: &str| {
        let directory = tempfile::tempdir().unwrap();
        for file in tree_files(&static_dir().join("vendor")) {
            std::fs::copy(
                static_dir().join("vendor").join(&file),
                directory.path().join(&file),
            )
            .unwrap();
        }
        assert!(
            verify_vendor_directory(directory.path()).is_ok(),
            "{name}: pristine copy"
        );
        directory
    };
    let tampered = copy("tampered");
    let target = tampered.path().join("preact.module.js");
    let mut bytes = std::fs::read(&target).unwrap();
    bytes.extend_from_slice(b"\n;globalThis.x=1;");
    std::fs::write(&target, bytes).unwrap();
    let error = verify_vendor_directory(tampered.path()).err().unwrap();
    assert!(error.contains("preact.module.js has SHA-256"), "{error}");

    let unrecorded = copy("unrecorded");
    std::fs::write(unrecorded.path().join("extra.module.js"), "export {}").unwrap();
    assert!(verify_vendor_directory(unrecorded.path()).is_err());
}

// -------------------------------------------------------------- HTML sinks

/// Tokens whose every use in shipped JavaScript must be accounted for.
const SINK_TOKENS: &[&str] = &[
    "dangerouslySetInnerHTML",
    "innerHTML",
    "outerHTML",
    "insertAdjacentHTML",
    "document.write",
    "eval(",
    "Function(",
    "import(",
    "setAttribute",
    ".style",
    "serviceWorker",
    "<script",
    "importmap",
];

/// Audited sink tokens in each vendored renderer file; absent tokens must not occur.
///
/// Preact's DOM diff is the renderer behind `dangerouslySetInnerHTML`, so its dist
/// necessarily writes `innerHTML`, and it sets attributes and `style` properties
/// from props. The application only reaches the HTML sink through `ServerHtml`
/// (checked below). Vendor files are not exempt: these counts are paired with
/// the VENDOR.md digest check, so changing any vendor file requires re-auditing
/// it and updating both records.
const VENDOR_SINKS: &[(&str, &[(&str, usize)])] = &[
    ("vendor/hooks.module.js", &[]),
    ("vendor/htm.module.js", &[]),
    (
        "vendor/preact.module.js",
        &[
            ("dangerouslySetInnerHTML", 2),
            ("innerHTML", 3),
            ("setAttribute", 2),
            (".style", 4),
        ],
    ),
];

fn sink_counts(source: &str) -> BTreeMap<&'static str, usize> {
    SINK_TOKENS
        .iter()
        .map(|token| (*token, count(source, token)))
        .filter(|(_, found)| *found > 0)
        .collect()
}

#[test]
fn vendor_sink_tokens_match_the_audit_for_the_recorded_digests() {
    verify_vendor_directory(&static_dir().join("vendor")).unwrap();
    let audited: BTreeSet<&str> = VENDOR_SINKS.iter().map(|(path, _)| *path).collect();
    let shipped: BTreeSet<String> = script_files()
        .into_iter()
        .filter(|path| path.starts_with("vendor/"))
        .collect();
    assert_eq!(
        shipped.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        audited
    );
    for (path, expected) in VENDOR_SINKS {
        let expected: BTreeMap<&str, usize> = expected.iter().copied().collect();
        assert_eq!(sink_counts(&read_static(path)), expected, "{path}");
    }
}

#[test]
fn application_html_sink_is_only_server_html() {
    let mut sinks = Vec::new();
    for path in application_scripts() {
        for (offset, _) in read_static(&path).match_indices("dangerouslySetInnerHTML") {
            sinks.push((path.clone(), offset));
        }
    }
    let ui = read_static("views/ui.js");
    let start = ui
        .find("export function ServerHtml(")
        .expect("ServerHtml component");
    let end = start + ui[start..].find("\n}").expect("end of ServerHtml");
    assert_eq!(sinks.len(), 1, "{sinks:?}");
    let (path, offset) = &sinks[0];
    assert_eq!(path, "views/ui.js");
    assert!((start..end).contains(offset), "sink outside ServerHtml");
}

#[test]
fn application_scripts_avoid_other_dynamic_html_and_code_sinks() {
    let forbidden = [
        ("innerHTML", r"innerHTML"),
        ("outerHTML", r"outerHTML"),
        ("insertAdjacentHTML", r"insertAdjacentHTML"),
        ("document.write", r"document\s*\.\s*write"),
        ("eval", r"\beval\s*\("),
        ("Function constructor", r"\bFunction\s*\("),
        ("style property", r"\.style\b"),
        ("style attribute", r#"setAttribute\s*\(\s*['"`]style"#),
        ("inline style", r"\bstyle\s*="),
        ("string event handler", r#"\bon[a-zA-Z]+\s*=\s*["']"#),
        ("script element", r"(?i)<script"),
        ("import map", r"(?i)importmap"),
        ("javascript: URL", r"(?i)javascript:"),
    ];
    let patterns: Vec<(&str, Regex)> = forbidden
        .iter()
        .map(|(name, pattern)| (*name, Regex::new(pattern).unwrap()))
        .collect();
    for path in application_scripts() {
        let source = read_static(&path);
        for (name, pattern) in &patterns {
            if let Some(found) = pattern.find(&source) {
                panic!("{path} uses {name}: {:?}", found.as_str());
            }
        }
    }
}

// ------------------------------------------------------------ page templates

#[test]
fn page_templates_load_external_modules_without_inline_code() {
    let source = web_app_source();
    // Raw HTML is permitted only from string literals written in this file.
    for (offset, _) in source.match_indices("(yinz-html-raw") {
        let argument = source[offset + "(yinz-html-raw".len()..].trim_start();
        assert!(
            argument.starts_with('"'),
            "yinz-html-raw with a computed argument"
        );
    }
    let scripts: Vec<&str> = Regex::new(r#"\(yinz-h\s+"script"[^\n]*"#)
        .unwrap()
        .find_iter(&source)
        .map(|found| found.as_str())
        .collect();
    assert_eq!(
        scripts,
        [r#"(yinz-h "script" (hash "type" "module" "src" src))) (hash-ref context "scripts"))))"#],
        "pages must load only external module scripts"
    );
    assert!(source.contains(r#"(if (equal? page "login") "/static/login.js" "/static/app.js")"#));
    assert!(source.contains(r#"(yinz-html-json-script "yinz-config" (hash-ref context "config"))"#));
    assert!(
        Regex::new(r#""viewport" "content" "[^"]*viewport-fit=cover[^"]*""#)
            .unwrap()
            .is_match(&source)
    );
    assert!(
        !source.contains(r#""style""#),
        "templates must not use inline style"
    );
    assert!(
        !Regex::new(r#""on[a-z]+""#).unwrap().is_match(&source),
        "templates must not use inline event handlers"
    );
}
