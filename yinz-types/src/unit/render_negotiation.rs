use super::*;

#[test]
fn render_negotiation_intersects_versions_features_and_media() {
    let mut offer = Offer::reader(None);
    assert_eq!(offer.features & SCROLL, 0);
    assert_ne!(offer.features & SHIFTS, 0);
    offer.features |= SCROLL | 0x8000;
    offer.versions = vec![3, 2, 3, 4, 5];
    let selected = offer.select(None).unwrap();
    assert_eq!(selected, Agreement::default());
    let synthetic = offer
        .intersect(&[2, 3], SHIFTS | SCROLL | GEOMETRY, None)
        .unwrap();
    assert_eq!(
        (synthetic.version, synthetic.features),
        (3, SHIFTS | SCROLL | GEOMETRY)
    );
    assert_eq!(
        offer
            .intersect(&[2], GEOMETRY | SCROLL, None)
            .unwrap()
            .features,
        GEOMETRY
    );
    for (producer, reader, media) in [
        (PRODUCED & !GEOMETRY, READABLE, INLINE),
        (PRODUCED, READABLE & !GEOMETRY, INLINE),
        (PRODUCED, READABLE, SHARED),
        (PRODUCED, READABLE, 0),
    ] {
        offer.features = reader;
        offer.media = media;
        let a = offer.intersect(&[4], producer, None).unwrap();
        assert_eq!(a.features & (KITTY | SIXEL), 0);
        assert_eq!(a.media, 0);
        assert!(offer.accepts(&a, None));
        assert!(!offer.accepts(
            &Agreement {
                features: a.features | KITTY,
                ..a
            },
            None
        ));
    }
    for versions in [vec![], vec![2, 3, 9]] {
        offer.versions = versions;
        let err = offer.select(None).unwrap_err();
        assert!(err.contains(&format!("offered {:?}, writable [4]", offer.versions)));
    }
}

#[test]
fn render_negotiation_bounds_the_offered_versions_in_a_rejection() {
    let mut offer = Offer::reader(None);
    offer.versions = vec![u32::MAX; 100_000];
    let reason = offer.select(None).unwrap_err();
    assert_eq!(reason.matches("4294967295").count(), 8);
    assert!(reason.contains("(+99992 more), writable [4]"));
    offer.versions.push(4);
    assert_eq!(offer.select(None).unwrap().version, 4);
}

#[test]
fn render_negotiation_shared_requires_the_exact_offered_challenge_response() {
    let offer = Offer::reader(Some(MediaProof {
        root: "/tmp".into(),
        basename: "probe".into(),
        challenge: vec![1; 16],
    }));
    let response = vec![2; 16];
    let shared = offer.select(Some(response.clone())).unwrap();
    assert_eq!(shared.media, INLINE | SHARED);
    assert!(offer.accepts(&shared, Some(&response)));
    assert!(!offer.accepts(&shared, Some(&[3; 16])));
    assert!(!offer.accepts(&shared, None));
    assert!(!Offer::reader(None).accepts(&shared, Some(&response)));
    let portable = offer.select(None).unwrap();
    assert_eq!(portable.media, INLINE);
    assert!(offer.accepts(&portable, Some(&response)));
    assert!(!offer.accepts(
        &Agreement {
            proof_response: response.clone(),
            ..portable
        },
        Some(&response)
    ));
}
