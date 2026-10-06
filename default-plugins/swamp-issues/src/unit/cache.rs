use super::*;

fn snapshot(stamp: i64) -> Cache {
    Cache {
        catalogue: Some(Search {
            issues: vec![],
            fetched_at: stamp,
        }),
        text_search: Some((
            "query".into(),
            Search {
                issues: vec![],
                fetched_at: stamp,
            },
        )),
        ..Default::default()
    }
}

#[test]
fn a_grant_is_remembered_and_a_denial_forgets_it() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("swamp-issues").join("permitted");
    assert!(!remembers_permission(&marker));
    remember_permission(&marker, false).unwrap();
    assert!(
        !remembers_permission(&marker),
        "denying first writes nothing"
    );
    remember_permission(&marker, true).unwrap();
    assert!(remembers_permission(&marker));
    remember_permission(&marker, true).unwrap();
    assert!(remembers_permission(&marker));
    remember_permission(&marker, false).unwrap();
    assert!(!remembers_permission(&marker));
}

#[test]
fn identities_use_the_full_key_and_normalized_server_with_distinct_anonymous_access() {
    let anonymous = Credentials::anonymous();
    let mut credentials = anonymous.clone();
    credentials.server_url.push('/');
    assert_eq!(identity(&credentials), identity(&anonymous));
    let mut identities = std::collections::BTreeSet::from([identity(&anonymous)]);
    for key in ["", "anonymous", "same-prefix-one", "same-prefix-two"] {
        credentials.api_key = Some(key.into());
        assert!(identities.insert(identity(&credentials)));
    }
    credentials.username = Some("different-user".into());
    assert!(identities.insert(identity(&credentials)));
    credentials.server_url = "https://other.example.test".into();
    assert!(identities.insert(identity(&credentials)));
}

#[test]
fn concurrent_replacements_always_publish_complete_json_and_clean_up_temporary_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.json");
    snapshot(0).write(&path).unwrap();
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        for writer in 1..=2 {
            let path = &path;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for stamp in 0..30 {
                    snapshot(writer * 100 + stamp).write(path).unwrap();
                }
            });
        }
        barrier.wait();
        for _ in 0..100 {
            let cache = Cache::read(&path).expect("every read sees a complete version");
            assert_eq!(
                cache.catalogue.unwrap().fetched_at,
                cache.text_search.unwrap().1.fetched_at
            );
        }
    });
    assert!(Cache::read(&path).unwrap().catalogue.unwrap().fetched_at > 0);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);

    let blocked = directory.path().join("blocked.json");
    fs::create_dir(&blocked).unwrap();
    assert!(snapshot(999).write(&blocked).is_err());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    assert!(
        Cache::read(&path).is_some(),
        "failed replacement leaves the prior cache intact"
    );
}
