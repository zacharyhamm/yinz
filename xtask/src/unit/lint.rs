use super::*;

fn found(source: &str) -> Vec<(&'static str, String)> {
    file_level_suppressions(source)
        .into_iter()
        .map(|s| (s.kind, s.lint))
        .collect()
}

fn allow(lint: &str) -> (&'static str, String) {
    ("allow", lint.to_owned())
}

#[test]
fn a_one_line_allow_is_found() {
    assert_eq!(
        found("//! docs\n#![allow(clippy::module_inception)]\nmod api;\n"),
        vec![("allow", "clippy::module_inception".to_owned())]
    );
}

#[test]
fn an_allow_rustfmt_wrapped_across_lines_is_found_whole() {
    let source = "#![allow(\n    clippy::too_many_arguments,\n    clippy::type_complexity,\n    \
                      clippy::needless_borrow\n)]\nfn main() {}\n";
    assert_eq!(
        found(source),
        vec![
            ("allow", "clippy::too_many_arguments".to_owned()),
            ("allow", "clippy::type_complexity".to_owned()),
            ("allow", "clippy::needless_borrow".to_owned()),
        ]
    );
}

#[test]
fn an_expect_keeps_its_kind_and_drops_its_reason() {
    let source = "#![expect(clippy::duplicate_mod, reason = \"one file, two engines\")]\n";
    assert_eq!(
        found(source),
        vec![("expect", "clippy::duplicate_mod".to_owned())]
    );
}

#[test]
fn a_reason_containing_brackets_does_not_end_the_attribute_early() {
    let source =
        "#![allow(dead_code, reason = \"see foo(), bar)] and \\\" baz\")]\n#![allow(unused)]\n";
    assert_eq!(
        found(source),
        vec![
            ("allow", "dead_code".to_owned()),
            ("allow", "unused".to_owned())
        ]
    );
}

#[test]
fn an_allow_inside_cfg_attr_is_found() {
    let source = "// why\n#![cfg_attr(not(target_arch = \"wasm32\"), allow(dead_code))]\n\
                      #![cfg_attr(test, expect(unused, reason = \"x\"), allow(clippy::x))]\n";
    assert_eq!(
        found(source),
        vec![
            ("allow", "dead_code".to_owned()),
            ("expect", "unused".to_owned()),
            ("allow", "clippy::x".to_owned()),
        ]
    );
}

#[test]
fn nested_cfg_attr_suppressions_are_found() {
    assert_eq!(
        found("#![cfg_attr(all(), cfg_attr(all(), allow(dead_code)))]"),
        vec![allow("dead_code")]
    );

    let source = r#"#![cfg_attr(
            any(),
            allow(unused_imports),
            cfg_attr(
                all(),
                // A nested attribute can contain siblings and further nesting.
                expect(unused_variables, reason = "see foo(), bar)]"),
                cfg_attr(
                    not(target_arch = "wasm32"),
                    /* why */ allow(dead_code, unused_mut, reason = "nested",),
                ),
                deny(missing_docs),
            ),
            expect(clippy::needless_borrow),
        )]"#;
    assert_eq!(
        found(source),
        vec![
            allow("unused_imports"),
            ("expect", "unused_variables".to_owned()),
            allow("dead_code"),
            allow("unused_mut"),
            ("expect", "clippy::needless_borrow".to_owned()),
        ]
    );
}

#[test]
fn nested_cfg_attr_without_file_level_suppressions_is_ignored() {
    let source = r#"
            #![cfg_attr(all(), cfg_attr(all(), deny(missing_docs), doc = "allow(x)"))]
            #[cfg_attr(all(), cfg_attr(all(), allow(dead_code)))]
            fn f() {}
            mod m {
                #![cfg_attr(all(), cfg_attr(all(), expect(unused)))]
            }
        "#;
    assert_eq!(found(source), vec![]);
}

#[test]
fn other_inner_attributes_and_item_attributes_are_not_suppressions() {
    let source = "#![cfg(unix)]\n#![deny(missing_docs)]\n#![doc = \"#![allow(x)]\"]\n\
                      /// a `#![allow(..)]` in a doc comment\n#[allow(dead_code)]\nfn f() {}\n\
                      fn g() { let s = \"#![allow(y)]\"; }\n";
    assert_eq!(found(source), vec![]);
}

#[test]
fn code_chars_skip_comments_and_literals() {
    // Every `]` below is inside a comment or a literal; everything else is code.
    let source = "a '\"' \"]\" // ]\nb /* ] /* ] */ ] */ c r#\"]\"# b'\\'' '\\u{1F600}' 'é' \
                      impl<'a> 'l: loop {} /*/ ] */ d /**/ e";
    let code = code_only(source);
    assert_eq!(
        code.split_whitespace().collect::<Vec<_>>().join(" "),
        "a b c r# b impl<'a> 'l: loop {} d e"
    );
    // Neither an unterminated literal nor a lone trailing quote panics.
    assert_eq!(code_only("x '"), "x '");
    assert_eq!(code_only("x \"unterminated"), "x ");
    assert_eq!(code_only("/* unterminated"), "");
    assert_eq!(code_only("'é' => {}"), " => {}");
}

#[test]
fn an_allow_inside_a_comment_or_string_is_not_a_suppression() {
    for source in [
        "/*\n#![allow(dead_code)]\n*/\n",
        "/* /* */ #![allow(x)] */\n",
        "const S: &str = \"\n#![allow(x)]\n\";\n",
        "const S: &str = r##\"\"# #![allow(x)]\"##;\n",
        "// #![allow(x)]\n//! #![allow(y)]\n",
    ] {
        assert_eq!(found(source), vec![], "{source:?}");
    }
}

#[test]
fn a_char_literal_or_multiline_string_in_an_attribute_does_not_hide_later_ones() {
    for source in [
        "#![doc = '\"']\n#![allow(x)]\n",
        "#![doc = \"a\nb\"]\n#![allow(x)]\n",
        "#![allow(x, reason = \"see http://x/*y*/\")]\n",
    ] {
        assert_eq!(found(source), vec![allow("x")], "{source:?}");
    }
}

#[test]
fn inner_attributes_inside_an_item_body_are_not_file_level() {
    let source = "#![allow(a)]\nmod t {\n    #![allow(b)]\n}\nfn f() {\n    #![allow(c)]\n}\n\
                      macro_rules! m { () => { #![allow(d)] }; }\n";
    assert_eq!(found(source), vec![allow("a")]);
}

#[test]
fn a_comment_inside_a_wrapped_list_is_not_a_lint() {
    assert_eq!(
        found("#![allow(\n    dead_code, // why\n    unused,\n)]\n"),
        vec![allow("dead_code"), allow("unused")]
    );
    assert_eq!(
        found("#![cfg_attr(test, // why\n    allow(x))]\n"),
        vec![allow("x")]
    );
}

#[test]
fn comments_around_attribute_names_do_not_hide_suppressions() {
    for (attribute, kind) in [
        ("allow(dead_code)", "allow"),
        ("expect(dead_code)", "expect"),
        ("cfg_attr(test, allow(dead_code))", "allow"),
    ] {
        for comment in ["/* why */", "// why\n"] {
            for body in [
                format!("{comment}{attribute}"),
                attribute.replacen('(', &format!("{comment}("), 1),
            ] {
                let source = format!("#![{body}]");
                assert_eq!(
                    found(&source),
                    vec![(kind, "dead_code".to_owned())],
                    "{source}"
                );
            }
        }
    }
}

#[test]
fn a_shebang_and_adjacent_or_unterminated_attributes() {
    assert_eq!(
        found("#!/usr/bin/env cargo\n#![allow(x)]\n"),
        vec![allow("x")]
    );
    assert_eq!(
        found("#![allow(x)]#![allow(y)]"),
        vec![allow("x"), allow("y")]
    );
    assert_eq!(found("#![allow(x"), vec![]);
}

/// Restores `path`'s mode on drop, so a failed assertion cannot leave a tempdir that
/// cannot be deleted.
#[cfg(unix)]
struct RestoreMode<'a>(&'a Path);

#[cfg(unix)]
impl Drop for RestoreMode<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
    }
}

/// Makes `path` unreadable until the guard drops.
#[cfg(unix)]
fn locked(path: &Path) -> RestoreMode<'_> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    RestoreMode(path)
}

/// Root reads a mode-000 directory regardless, so the two tests below prove nothing there.
#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(unix)]
#[test]
fn rust_sources_skips_an_unreadable_subdirectory() {
    if is_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let locked_dir = root.path().join("locked");
    std::fs::create_dir(&locked_dir).unwrap();
    std::fs::write(root.path().join("a.rs"), "").unwrap();
    std::fs::write(locked_dir.join("b.rs"), "").unwrap();
    let _restore = locked(&locked_dir);

    let mut files = Vec::new();
    rust_sources(root.path(), root.path(), &mut files).unwrap();

    assert!(files.contains(&PathBuf::from("a.rs")), "{files:?}");
    assert!(!files.contains(&PathBuf::from("locked/b.rs")), "{files:?}");
}

#[cfg(unix)]
#[test]
fn rust_sources_fails_when_the_root_is_unreadable() {
    if is_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let _restore = locked(root.path());

    let mut files = Vec::new();
    assert!(rust_sources(root.path(), root.path(), &mut files).is_err());
}

#[test]
fn rust_sources_skips_vendored_dependencies() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("vendor/steel-core")).unwrap();
    std::fs::create_dir_all(root.path().join("src/vendor")).unwrap();
    std::fs::write(root.path().join("vendor/steel-core/lib.rs"), "").unwrap();
    std::fs::write(root.path().join("src/vendor/adapter.rs"), "").unwrap();
    let mut files = Vec::new();
    rust_sources(root.path(), root.path(), &mut files).unwrap();
    assert_eq!(files, vec![PathBuf::from("src/vendor/adapter.rs")]);
}

#[test]
fn add_plugin_constructions_are_counted_and_patterns_are_not() {
    let source = r#"
        fn send(senders: &Senders) {
            // ScreenInstruction::AddPlugin(in a comment)
            let text = "ScreenInstruction::AddPlugin(in a literal)";
            senders.send(ScreenInstruction::AddPlugin(a, b, Some((c, d))));
            match instruction {
                ScreenInstruction::AddPlugin(.., id, _) => {},
                ScreenInstruction::AddPlugin(..) | ScreenInstruction::Other => {},
                ScreenInstruction::AddPlugin(x) if x => {},
            }
            if let ScreenInstruction::AddPlugin(..) = instruction {}
            assert!(matches!(instruction, ScreenInstruction::AddPlugin(..)));
            send(&senders, ScreenInstruction::AddPlugin(3));
        }
        #[cfg(test)]
        #[allow(unused)]
        mod tests {
            fn built() { ScreenInstruction::AddPlugin(1); }
        }
        fn after() { let built = ScreenInstruction::AddPlugin(2); }
    "#;
    assert_eq!(add_plugin_constructions(source), 3);
}

#[test]
fn add_plugin_constructions_through_any_path_are_counted() {
    for (source, expected) in [
        (
            "use ScreenInstruction as I; fn f() { send(I::AddPlugin(1)); }",
            1,
        ),
        (
            "impl ScreenInstruction { fn park() -> Self { Self::AddPlugin(1) } }",
            1,
        ),
        // The import, and the bare call it allows.
        (
            "use yinz_bus::screen::ScreenInstruction::AddPlugin; fn f() { send(AddPlugin(1)); }",
            2,
        ),
        (
            "use yinz_bus::screen::ScreenInstruction::AddPlugin as Park; fn f() { send(Park(1)); }",
            1,
        ),
        (
            "pub(crate) use yinz_bus::screen::ScreenInstruction::{self, AddPlugin};",
            1,
        ),
        ("fn f() { send(ScreenInstruction :: AddPlugin (1)); }", 1),
        (
            "fn f() { send(yinz_bus::screen::ScreenInstruction::\n    AddPlugin(1)); }",
            1,
        ),
        ("fn f() { items.map(ScreenInstruction::AddPlugin); }", 1),
        // A glob import of the variants, which lets the bare name pass as a function.
        (
            "use yinz_bus::screen::ScreenInstruction::*; fn f() { items.map(AddPlugin); }",
            1,
        ),
        (
            "use yinz_bus::screen::{ScreenInstruction :: { self, * }, Other};",
            1,
        ),
        ("use yinz_bus::screen::*; use MyScreenInstruction::*;", 0),
        ("fn f() { match i { I::AddPlugin(..) => {}, _ => {} } }", 0),
        ("fn f() -> bool { matches!(i, Self::AddPlugin(..)) }", 0),
        ("fn f() -> bool { matches!(i, I :: AddPlugin(..)) }", 0),
        ("pub enum ScreenInstruction { AddPlugin(u32), Other }", 0),
        (
            "pub enum ScreenContext { AddPlugin, Other } fn f() { ScreenContext::AddPlugin }",
            0,
        ),
        ("fn f() { send(MyAddPlugin(1)); AddPluginLater(2); }", 0),
        (
            "// I::AddPlugin(1)\nfn f() { let s = \"Self::AddPlugin(1)\"; }",
            0,
        ),
    ] {
        assert_eq!(add_plugin_constructions(source), expected, "{source}");
    }
}

#[test]
fn add_plugin_owners_check_fails_on_a_source_it_cannot_read() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("aliased.rs");
    std::fs::write(&file, [0xff, 0xfe, b'(']).unwrap();
    let error = add_plugin_offenders(root.path()).unwrap_err();
    assert!(format!("{error:#}").contains("aliased.rs"), "{error:#}");

    std::fs::write(&file, "fn f() { send(I::AddPlugin(1)); }").unwrap();
    assert_eq!(
        add_plugin_offenders(root.path()).unwrap(),
        ["aliased.rs: 1"]
    );
}
