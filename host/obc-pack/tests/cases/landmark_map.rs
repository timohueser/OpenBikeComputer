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
        display_pages: vec!["Quelle und Autoren".into()],
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
            attribution: credit(),
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
    let landmarks = landmark_map::load(&path, &links, bbox).unwrap();
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
    let key = landmark_map::fingerprint(&path).unwrap();
    content.records[0].name = "Andere Burg".into();
    std::fs::write(&path, serde_json::to_vec(&content).unwrap()).unwrap();
    assert_ne!(key, landmark_map::fingerprint(&path).unwrap());
    std::fs::write(root.join("photo.rgb222"), vec![0; PHOTO_PIXELS]).unwrap();
    assert!(landmark_map::fingerprint(&path).is_err(), "a cache hit must verify declared photo hashes");
    assert!(landmark_map::load(&path, &links, bbox).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
