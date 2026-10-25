use super::*;

#[test]
fn a_local_folder_is_keyed_by_its_path_alone() {
    assert_eq!(Location::local("/work/agent").key(), "/work/agent");
}

#[test]
fn a_trailing_separator_does_not_make_a_second_key() {
    assert_eq!(
        Location::local("/work/agent/").key(),
        Location::local("/work/agent").key()
    );
}

#[test]
fn a_key_round_trips_through_awkward_hosts() {
    for host in ["build-box", "user@host", "host:22", "a/b"] {
        let location = Location::new(Some(host), "/work/agent");
        assert_eq!(
            Location::parse(&location.key()),
            location,
            "the key for {host:?} did not survive a round trip"
        );
    }
}

/// A colon is a legal character in a Unix filename, so the path half cannot
/// be found by splitting at the last colon either.
#[test]
fn a_key_round_trips_when_the_path_holds_a_colon() {
    let location = Location::new(Some("build-box"), "/work/a:b");
    assert_eq!(Location::parse(&location.key()), location);
}

#[test]
fn an_unhosted_key_parses_as_this_machine() {
    assert_eq!(
        Location::parse("/work/agent"),
        Location::local("/work/agent")
    );
}

/// Why the key is `@host:path` and not `//host/path`.
///
/// `Path` collapses a leading `//` to one root, so the URL-ish form is *not*
/// disjoint from an ordinary absolute path. Were it the key, a local pane in
/// a folder named after a machine would match that machine's checkout by
/// prefix. Asserted rather than described, so nobody re-derives it.
#[test]
fn the_url_shaped_key_would_have_collided_with_a_path() {
    assert!(Path::new("//build-box/work/x").starts_with("/build-box/work"));
    assert!(!Location::local("/build-box/work/x")
        .starts_with(&Location::new(Some("build-box"), "/work")));
}

#[test]
fn a_folder_is_inside_another_only_on_the_same_machine() {
    let here = Location::local("/work/agent/src");
    assert!(here.starts_with(&Location::local("/work/agent")));
    assert!(!here.starts_with(&Location::new(Some("build-box"), "/work/agent")));

    let there = Location::new(Some("build-box"), "/work/agent/src");
    assert!(there.starts_with(&Location::new(Some("build-box"), "/work/agent")));
    assert!(!there.starts_with(&Location::local("/work/agent")));
}

#[test]
fn containment_is_component_wise() {
    assert!(!Location::local("/work/agent-old").starts_with(&Location::local("/work/agent")));
}
