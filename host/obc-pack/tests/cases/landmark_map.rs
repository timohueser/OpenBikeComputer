use obc_formats::{
    io::SliceSource,
    obcm::{landmarks::*, PoiApproach, PoiMetadata, SourceId, POI_HOURS_REF_NONE},
};
use obc_pack::{
    landmark_map,
    landmarks::{Attribution, Content, Photo, Record, TextVariant},
    poi::LandmarkLink,
};
use obc_reader::{
    landmarks::{page, LandmarkDirectory},
    photo::{PhotoDecoder, Progress},
};
use sha2::{Digest, Sha256};

fn credit() -> Attribution {
    Attribution {
        source_url: "https://de.wikipedia.org/w/index.php?title=Burg&oldid=1".into(),
        revision: "1".into(),
        license_url: "https://creativecommons.org/licenses/by-sa/4.0/".into(),
        original_notices: "Autoren".into(),
    }
}

fn photo_credit() -> Attribution {
    Attribution {
        source_url: "https://commons.wikimedia.org/wiki/File:Burg.jpg".into(),
        revision: "2026-01-01T00:00:00Z".into(),
        license_url: "https://creativecommons.org/licenses/by/4.0/".into(),
        original_notices: r#"{"Artist":{"value":"<a href=\"//commons.wikimedia.org/wiki/User:A\">A</a>"}}"#.into(),
    }
}

#[test]
fn source_join_content_pool_and_independent_photo_readback() {
    let root = obcm_testkit::scratch::scratch_dir("landmark-map", "join");
    let pixels: Vec<u8> = (0..PHOTO_PIXELS).map(|i| ((i / 216 + i % 216) % 64) as u8).collect();
    std::fs::write(root.join("photo.rgb222"), &pixels).unwrap();
    let digest: String = Sha256::digest(&pixels).iter().map(|b| format!("{b:02x}")).collect();
    let record = |qid: &str, lon: f64| Record {
        qid: qid.into(),
        name: "Burg".into(),
        category: 1,
        latitude: 47.0,
        longitude: lon,
        default_language: "de".into(),
        fallback_sources: vec![],
        variants: vec![
            TextVariant { language: "de".into(), text_pages: vec!["Eine Burg.".into()], attribution: credit() },
            TextVariant { language: "en".into(), text_pages: vec!["A castle.".into()], attribution: credit() },
        ],
        photo: Some(Photo {
            path: "photo.rgb222".into(),
            sha256: digest.clone(),
            bytes: PHOTO_PIXELS,
            attribution: photo_credit(),
        }),
    };
    let mut content = Content {
        schema: 2,
        input_sha256: "input".into(),
        policy_sha256: "policy".into(),
        category_policy_sha256: "categories".into(),
        languages: vec!["en".into(), "de".into(), "fr".into(), "es".into()],
        source_coverage: serde_json::json!({}),
        counts: Default::default(),
        candidate_qids: vec![],
        photo_requests: vec![],
        records: vec![record("Q2", 8.01), record("Q1", 8.0)],
        omissions: vec![],
    };
    let path = root.join("content.json");
    std::fs::write(&path, serde_json::to_vec(&content).unwrap()).unwrap();
    let approach = PoiApproach { source: SourceId::osm(1, 42), lat: 46_999_999, lon: 7_999_999, profile_mask: 1 };
    let linked = PoiMetadata { source: SourceId::osm(2, 9), approach: Some(approach) };
    let links = [
        LandmarkLink {
            metadata: PoiMetadata { source: SourceId::osm(1, 1), approach: None },
            position: None,
            wikidata: Some("Q1".into()),
            wikipedia: None,
            hours: None,
        },
        LandmarkLink {
            metadata: linked,
            position: None,
            wikidata: Some("Q1".into()),
            wikipedia: None,
            hours: obc_pack::hours::parse("Mo-Fr 09:00-17:00"),
        },
    ];
    let bbox = (7_000_000, 46_000_000, 9_000_000, 48_000_000);
    let artifact = [path.clone()];
    let landmarks = landmark_map::load(&artifact, &links, bbox).unwrap();
    assert_eq!(landmarks[0].record.osm, Some(linked));
    assert!(landmarks[0].hours.is_some(), "a non-service source retains its hours");
    assert_eq!(landmarks[1].record.osm, None);
    let bytes = landmark_map::serialize(&landmarks, &[3, POI_HOURS_REF_NONE]).unwrap();
    let source = SliceSource(&bytes);
    let directory = LandmarkDirectory::read(&source).unwrap();
    let first = directory.record(&source, 0).unwrap();
    let second = directory.record(&source, 1).unwrap();
    assert_eq!(first.hours_ref, 3);
    assert_eq!(first.photo, second.photo, "identical content shares one compressed blob");
    let article = directory.article(&source, &first, *b"de").unwrap();
    let english = directory.article(&source, &first, *b"es").unwrap();
    let english_text = directory.content(&source, english.text, MAX_TEXT_BYTES).unwrap();
    assert_eq!(page(&english_text, 1, 0, &mut [0; MAX_PAGE_BYTES]).unwrap(), "A castle.");
    let text = directory.content(&source, article.text, MAX_TEXT_BYTES).unwrap();
    assert_eq!(page(&text, 1, 0, &mut [0; MAX_PAGE_BYTES]).unwrap(), "Eine Burg.");
    for (reference, expected) in [
        (
            article.attribution,
            [
                "de.wikipedia.org/?oldid=1",
                "Burg",
                "Wikipedia contributors",
                "CC BY-SA 4.0 creativecommons.org/licenses/by-sa/4.0/",
            ],
        ),
        (
            first.photo_attribution,
            ["Wikimedia Commons", "Burg.jpg", "A", "CC BY 4.0 creativecommons.org/licenses/by/4.0/"],
        ),
    ] {
        let credit = directory.content(&source, reference, MAX_ATTRIBUTION_BYTES).unwrap();
        for (index, expected) in expected.into_iter().enumerate() {
            assert_eq!(page(&credit, CREDIT_FIELDS, index as u16, &mut [0; MAX_PAGE_BYTES]).unwrap(), expected);
        }
    }
    let photo = directory.content(&source, first.photo, PHOTO_MAX_COMPRESSED as u32).unwrap();
    let mut decoder = PhotoDecoder::new();
    let mut decoded = vec![];
    for _ in 0..1024 {
        if decoder
            .step(&photo, |offset, chunk| {
                assert_eq!(offset, decoded.len());
                decoded.extend_from_slice(chunk);
            })
            .unwrap()
            == Progress::Complete
        {
            break;
        }
    }
    assert_eq!(decoded, pixels);
    assert!(landmark_map::serialize(&[landmarks[1].clone(), landmarks[0].clone()], &[POI_HOURS_REF_NONE, 3]).is_err());
    assert!(landmark_map::serialize(&[landmarks[0].clone(), landmarks[0].clone()], &[3, 3]).is_err());
    let key = landmark_map::fingerprint(&artifact).unwrap();
    content.records[0].name = "Andere Burg".into();
    std::fs::write(&path, serde_json::to_vec(&content).unwrap()).unwrap();
    assert_ne!(key, landmark_map::fingerprint(&artifact).unwrap());
    std::fs::write(root.join("photo.rgb222"), vec![0; PHOTO_PIXELS]).unwrap();
    assert!(landmark_map::fingerprint(&artifact).is_err(), "a cache hit must verify declared photo hashes");
    assert!(landmark_map::load(&artifact, &links, bbox).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

/// Neighbouring regions overlap, so a cell can be handed the same place twice. One record per QID
/// reaches the map, both sides of a border reach it, and the winner is the same for either order.
#[test]
fn overlapping_artifacts_merge_by_qid() {
    let root = obcm_testkit::scratch::scratch_dir("landmark-map", "merge");
    let record = |qid: &str, name: &str| Record {
        qid: qid.into(),
        name: name.into(),
        category: 1,
        latitude: 47.0,
        longitude: 8.0,
        default_language: "de".into(),
        fallback_sources: vec![],
        variants: vec![TextVariant {
            language: "de".into(),
            text_pages: vec!["Eine Burg.".into()],
            attribution: credit(),
        }],
        photo: None,
    };
    let artifact = |region: &str, records: Vec<Record>| {
        let dir = root.join(region);
        std::fs::create_dir_all(&dir).unwrap();
        let content = Content {
            schema: 2,
            input_sha256: region.into(),
            policy_sha256: "policy".into(),
            category_policy_sha256: "categories".into(),
            languages: vec!["en".into(), "de".into(), "fr".into(), "es".into()],
            source_coverage: serde_json::json!({}),
            counts: Default::default(),
            candidate_qids: vec![],
            photo_requests: vec![],
            records,
            omissions: vec![],
        };
        let path = dir.join("content.json");
        std::fs::write(&path, serde_json::to_vec(&content).unwrap()).unwrap();
        path
    };
    let west = artifact("west", vec![record("Q1", "Westburg"), record("Q7", "Grenzburg")]);
    let east = artifact("east", vec![record("Q7", "Grenzburg-Ost"), record("Q9", "Ostburg")]);
    let bbox = (7_000_000, 46_000_000, 9_000_000, 48_000_000);

    let merged = landmark_map::load(&[west.clone(), east.clone()], &[], bbox).unwrap();
    assert_eq!(
        merged.iter().map(|l| l.record.qid).collect::<Vec<_>>(),
        vec![1, 7, 9],
        "one record per QID, and a cell on the seam carries both sides"
    );
    let reversed = landmark_map::load(&[east.clone(), west.clone()], &[], bbox).unwrap();
    assert!(
        merged.iter().zip(&reversed).all(|(a, b)| a.record.encode() == b.record.encode() && a.content == b.content),
        "the merge does not depend on the order the artifacts arrive in"
    );
    // The two Q7 records differ in the name blob alone, so the lower name digest is the winner.
    fn lower<'a>(a: &'a str, b: &'a str) -> &'a str {
        if <[u8; 32]>::from(Sha256::digest(a)) < <[u8; 32]>::from(Sha256::digest(b)) {
            a
        } else {
            b
        }
    }
    assert_eq!(merged[1].content[0], lower("Grenzburg", "Grenzburg-Ost").as_bytes());
    assert_eq!(
        landmark_map::fingerprint(&[west.clone(), east.clone()]).unwrap(),
        landmark_map::fingerprint(&[east, west]).unwrap(),
        "the cell cache key is over the set of artifacts, not their order"
    );
    std::fs::remove_dir_all(root).unwrap();
}
