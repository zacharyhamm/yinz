use super::*;

fn record(path: &str, body: &str) -> String {
    format!("\u{1}{path}\u{2}{body}\n")
}

fn skill(name: &str, body: &str) -> Skill {
    Skill {
        name: name.to_owned(),
        description: String::new(),
        body: body.to_owned(),
        path: format!("/p/.hocus/skills/{name}/SKILL.md"),
        source: Source::Project,
        warnings: Vec::new(),
    }
}

// -- the frontmatter -----------------------------------------------------

#[test]
fn a_fenced_header_gives_the_name_and_description() {
    let text = "---\nname: bench\ndescription: Run the benchmarks.\n---\n\nDo the thing.\n";
    let listing = parse_listing(Source::User, &record("/u/bench/SKILL.md", text));
    assert_eq!(listing.skills.len(), 1, "{listing:?}");
    let skill = &listing.skills[0];
    assert_eq!(skill.name, "bench");
    assert_eq!(skill.description, "Run the benchmarks.");
    assert_eq!(skill.body, "Do the thing.");
    assert_eq!(skill.path, "/u/bench/SKILL.md");
    assert!(skill.warnings.is_empty());
}

#[test]
fn a_block_scalar_description_is_folded_and_a_nested_map_is_stepped_over() {
    let text = "---\ndescription: |\n  First line.\n  Second line.\nmetadata:\n  author: me\n  \
                version: 1\nname: 'quoted'\n---\nBody.\n";
    let listing = parse_listing(Source::User, &record("/u/quoted/SKILL.md", text));
    let skill = &listing.skills[0];
    assert_eq!(skill.description, "First line. Second line.");
    assert_eq!(skill.body, "Body.");
    assert!(skill.warnings.is_empty(), "{:?}", skill.warnings);
}

#[test]
fn no_header_means_the_first_line_describes_it() {
    let text = "# Review\n\nLook at the diff.\n";
    let listing = parse_listing(Source::User, &record("/u/review/SKILL.md", text));
    let skill = &listing.skills[0];
    assert_eq!(skill.description, "# Review");
    assert_eq!(skill.body, "# Review\n\nLook at the diff.");
}

#[test]
fn an_unclosed_header_is_a_warning_and_all_body() {
    let text = "---\nname: x\nSomething.\n";
    let listing = parse_listing(Source::User, &record("/u/x/SKILL.md", text));
    let skill = &listing.skills[0];
    assert_eq!(skill.body, text.trim());
    assert!(skill.warnings[0].contains("never closed"));
}

#[test]
fn the_folder_names_the_skill_and_a_disagreeing_header_warns() {
    let text = "---\nname: other\n---\nBody.\n";
    let listing = parse_listing(Source::User, &record("/u/actual/SKILL.md", text));
    let skill = &listing.skills[0];
    assert_eq!(skill.name, "actual");
    assert!(skill.warnings[0].contains("the folder wins"));
}

#[test]
fn unknown_keys_are_ignored() {
    let text = "---\nallowed-tools: Bash(*)\nmodel: sonnet\ndisable-model-invocation: \
                true\n---\nBody.\n";
    let listing = parse_listing(Source::User, &record("/u/k/SKILL.md", text));
    assert_eq!(listing.skills.len(), 1);
    assert!(listing.skills[0].warnings.is_empty());
}

// -- what is refused -----------------------------------------------------

#[test]
fn a_bad_folder_name_or_an_empty_body_is_skipped_with_a_reason() {
    let stdout = [
        record("/u/Bad Name/SKILL.md", "Body."),
        record("/u/-flag/SKILL.md", "Body."),
        record("/u/empty/SKILL.md", "---\nname: empty\n---\n\n"),
        record("/u/fine/SKILL.md", "Body."),
    ]
    .concat();
    let listing = parse_listing(Source::User, &stdout);
    assert_eq!(listing.skills.len(), 1);
    assert_eq!(listing.skills[0].name, "fine");
    let names: Vec<&str> = listing.skipped.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Bad Name", "-flag", "empty"]);
    assert!(listing.skipped[2].1.contains("no body"));
}

#[test]
fn a_body_past_the_cap_is_skipped() {
    let long = "x".repeat(MAX_SKILL_BYTES + 1);
    let listing = parse_listing(Source::User, &record("/u/long/SKILL.md", &long));
    assert!(listing.skills.is_empty());
    assert!(listing.skipped[0].1.contains("over"));

    let exact = "x".repeat(MAX_SKILL_BYTES);
    let listing = parse_listing(Source::User, &record("/u/exact/SKILL.md", &exact));
    assert_eq!(listing.skills.len(), 1);
}

#[test]
fn a_cut_listing_keeps_what_was_whole_and_says_so() {
    let mut stdout = record("/u/one/SKILL.md", "One.");
    stdout.push_str("\u{1}/u/two/SKILL.md\u{2}Two, cut mid");
    stdout.push_str("\n[truncated]");
    let listing = parse_listing(Source::User, &stdout);
    assert_eq!(listing.skills.len(), 1);
    assert_eq!(listing.skills[0].name, "one");
    assert!(listing.errors[0].contains("cut"));
}

#[test]
fn a_body_ending_in_the_marker_is_not_a_cut() {
    let stdout = record("/u/one/SKILL.md", "Ends with\n[truncated]");
    let listing = parse_listing(Source::User, &stdout);
    assert_eq!(listing.skills.len(), 1);
    assert!(listing.errors.is_empty());
}

#[test]
fn an_empty_listing_is_nothing_at_all() {
    assert_eq!(parse_listing(Source::User, ""), Listing::default());
}

// -- the registry ----------------------------------------------------------

#[test]
fn the_project_tier_shadows_the_user_tier_and_names_are_listed_once() {
    let mut registry = Registry::default();
    let reserved = BTreeSet::new();
    registry.replace(
        Source::User,
        Listing {
            skills: vec![skill("bench", "user"), skill("hello", "hi")],
            ..Listing::default()
        },
        &reserved,
    );
    registry.replace(
        Source::Project,
        Listing {
            skills: vec![skill("bench", "project")],
            ..Listing::default()
        },
        &reserved,
    );
    assert_eq!(registry.get("bench").unwrap().body, "project");
    assert_eq!(registry.get("hello").unwrap().body, "hi");
    assert_eq!(registry.names(), ["bench", "hello"]);

    registry.clear(Source::Project);
    assert_eq!(registry.get("bench").unwrap().body, "user");
}

#[test]
fn a_reserved_name_is_dropped_and_replacing_a_tier_keeps_the_other() {
    let mut registry = Registry::default();
    let reserved = BTreeSet::from(["close_pane".to_owned()]);
    registry.replace(
        Source::User,
        Listing {
            skills: vec![skill("close_pane", "no"), skill("ok", "yes")],
            ..Listing::default()
        },
        &reserved,
    );
    assert!(registry.get("close_pane").is_none());
    assert_eq!(registry.skipped()[0].1, "close_pane");
    assert!(registry.skipped()[0].2.contains("taken"));

    registry.replace(Source::Project, Listing::default(), &reserved);
    assert!(registry.get("ok").is_some(), "the user tier survived");
    registry.set_error(Source::Project, "no such folder".to_owned());
    assert_eq!(registry.errors(), [(Source::Project, "no such folder")]);
}

#[test]
fn a_tier_holds_at_most_the_cap() {
    let mut registry = Registry::default();
    let skills: Vec<Skill> = (0..MAX_SKILLS + 1)
        .map(|n| skill(&format!("s{n:02}"), "b"))
        .collect();
    registry.replace(
        Source::User,
        Listing {
            skills,
            ..Listing::default()
        },
        &BTreeSet::new(),
    );
    assert_eq!(registry.names().len(), MAX_SKILLS);
    assert_eq!(registry.skipped().len(), 1);
}

// -- what goes on the wire -------------------------------------------------

#[test]
fn the_tail_is_appended_when_the_body_has_no_placeholder() {
    let out = expand(
        &skill("bench", "Run the benchmarks."),
        "  fix the flaky test ",
    );
    assert_eq!(
        out,
        "<skill name=\"bench\" path=\"/p/.hocus/skills/bench/SKILL.md\">\nRun the \
         benchmarks.\n</skill>\n\nfix the flaky test"
    );
    let bare = expand(&skill("bench", "Run the benchmarks."), "");
    assert!(bare.ends_with("</skill>"), "{bare}");
}

#[test]
fn the_placeholder_takes_the_tail_instead() {
    let out = expand(
        &skill("greet", "Say hello to $ARGUMENTS, twice: $ARGUMENTS."),
        "Ann",
    );
    assert!(out.contains("Say hello to Ann, twice: Ann."), "{out}");
    assert!(out.ends_with("</skill>"), "not appended as well: {out}");
    let empty = expand(&skill("greet", "Hello $ARGUMENTS."), "");
    assert!(empty.contains("Hello ."));
}

#[test]
fn a_typed_line_splits_into_a_name_and_a_tail() {
    assert_eq!(split_command("/bench"), Some(("bench", "")));
    assert_eq!(
        split_command("  /bench  fix the test  "),
        Some(("bench", "fix the test"))
    );
    assert_eq!(split_command("/"), None);
    assert_eq!(split_command("bench"), None);
}

#[test]
fn the_scan_is_one_shell_loop_over_the_roots() {
    let argv = scan_argv(&["/u/skills".to_owned(), "/p/.hocus/skills".to_owned()]);
    assert_eq!(&argv[..2], ["sh", "-c"]);
    assert!(argv[2].contains("/*/SKILL.md"));
    assert!(argv[2].contains(&format!("head -c {}", MAX_SKILL_BYTES + 1)));
    assert!(argv[2].contains("printf '\\001%s\\002'"));
    // A missing folder is an empty tier, but one that cannot be entered and a
    // file that cannot be read are failures, and the script has to say so.
    assert!(argv[2].contains("exit $status"), "{}", argv[2]);
    assert!(
        argv[2].contains("[ ! -r \"$root\" ] || [ ! -x \"$root\" ]"),
        "{}",
        argv[2]
    );
    assert!(argv[2].contains("if ! head -c"), "{}", argv[2]);
    assert_eq!(&argv[3..], ["sh", "/u/skills", "/p/.hocus/skills"]);
}
