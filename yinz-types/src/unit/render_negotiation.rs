use super::*;

#[test]
fn render_negotiation_intersects_versions_features_and_media() {
    let mut offer = Offer::reader(None);
    assert_eq!(offer.features & SCROLL, 0);
    assert_ne!(offer.features & SHIFTS, 0);
    offer.features |= SCROLL | 0x8000;
    offer.versions = vec![3, 2, 3, 4, 5];
    let selected = offer.select(None).unwrap();
    assert_eq!(
        selected,
        Agreement {
            media: INLINE | CHUNKED,
            ..Agreement::default()
        }
    );
    let synthetic = offer
        .intersect(&[2, 3], SHIFTS | SCROLL | GEOMETRY, None, true)
        .unwrap();
    assert_eq!(
        (synthetic.version, synthetic.features),
        (3, SHIFTS | SCROLL | GEOMETRY)
    );
    assert_eq!(
        offer
            .intersect(&[2], GEOMETRY | SCROLL, None, true)
            .unwrap()
            .features,
        GEOMETRY
    );
    for (producer, reader, media) in [
        (PRODUCED & !GEOMETRY, READABLE, INLINE),
        (PRODUCED, READABLE & !GEOMETRY, INLINE),
        (PRODUCED, READABLE, SHARED),
        (PRODUCED, READABLE, CHUNKED),
        (PRODUCED, READABLE, 0),
    ] {
        offer.features = reader;
        offer.media = media | CHUNKED;
        let a = offer.intersect(&[4], producer, None, true).unwrap();
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
    assert_eq!(shared.media, INLINE | SHARED | CHUNKED);
    assert!(offer.accepts(&shared, Some(&response)));
    assert!(!offer.accepts(&shared, Some(&[3; 16])));
    assert!(!offer.accepts(&shared, None));
    assert!(!Offer::reader(None).accepts(&shared, Some(&response)));
    let portable = offer.select(None).unwrap();
    assert_eq!(portable.media, INLINE | CHUNKED);
    assert!(offer.accepts(&portable, Some(&response)));
    assert!(!offer.accepts(
        &Agreement {
            proof_response: response.clone(),
            ..portable
        },
        Some(&response)
    ));
}

#[test]
fn render_negotiation_chunked_follows_kitty_and_the_offer() {
    let forwarded = Offer::reader(None);
    assert_eq!(forwarded.media, INLINE | CHUNKED);
    let selected = forwarded.select(None).unwrap();
    assert_eq!(selected.media, INLINE | CHUNKED);
    assert!(forwarded.accepts(&selected, None));
    // Text-only masking drops Kitty, and with it every media bit.
    let mut text = Offer::reader(None);
    text.features = 0;
    assert_eq!(text.select(None).unwrap().media, 0);
    // Sixel alone carries inline media but never chunks.
    let mut sixel = Offer::reader(None);
    sixel.features &= !KITTY;
    let a = sixel.select(None).unwrap();
    assert_eq!(a.media, INLINE);
    assert!(sixel.accepts(&a, None));
    assert!(!sixel.accepts(
        &Agreement {
            media: INLINE | CHUNKED,
            ..a
        },
        None
    ));
    // A reader from before chunking never offers bit 4, so it is never selected.
    let mut old = Offer::reader(None);
    old.media = INLINE;
    let a = old.select(None).unwrap();
    assert_eq!(a.media, INLINE);
    assert!(!old.accepts(
        &Agreement {
            media: INLINE | CHUNKED,
            ..a
        },
        None
    ));
}

#[test]
fn render_negotiation_new_reader_accepts_an_older_server_agreement() {
    // Exactly what a server from before chunking selects for each offer.
    let older = Agreement {
        version: 4,
        features: PRODUCED,
        media: INLINE,
        proof_response: Vec::new(),
    };
    let forwarded = Offer::reader(None);
    assert!(forwarded.accepts(&older, None));
    let local = Offer::reader(Some(MediaProof {
        root: "/tmp".into(),
        basename: "probe".into(),
        challenge: vec![1; 16],
    }));
    let response = vec![2; 16];
    let older_shared = Agreement {
        media: INLINE | SHARED,
        proof_response: response.clone(),
        ..older.clone()
    };
    assert!(local.accepts(&older_shared, Some(&response)));
    assert!(!local.accepts(&older_shared, Some(&[3; 16])));
    // This build's own selections are accepted too.
    let own = forwarded.select(None).unwrap();
    assert_eq!(own.media, INLINE | CHUNKED);
    assert!(forwarded.accepts(&own, None));
    // Chunks without Kitty are refused, and so is an unoffered chunked bit.
    let mut no_kitty = Offer::reader(None);
    no_kitty.features &= !KITTY;
    assert!(!no_kitty.accepts(
        &Agreement {
            features: PRODUCED & !KITTY,
            media: INLINE | CHUNKED,
            ..older.clone()
        },
        None
    ));
    let mut unoffered = Offer::reader(None);
    unoffered.media &= !CHUNKED;
    assert!(!unoffered.accepts(&own, None));
}
