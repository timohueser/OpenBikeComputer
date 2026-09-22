use super::*;
use serde_json::json;

#[test]
fn photo_revision_and_required_creator_come_from_captured_metadata() {
    let root = obcm_testkit::scratch::scratch_dir("landmarks", "photo-revision");
    let image = image::DynamicImage::new_rgb8(2, 2);
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let bytes = bytes.into_inner();
    let upstream =
        <sha1::Sha1 as sha1::Digest>::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let capture = json!({"path":"image.png", "metadata_path":"metadata.json"});
    let allowed = BTreeSet::from(["Image.png".into()]);
    fs::write(root.join("image.png"), &bytes).unwrap();
    for (expected_sha1, license, artist, error) in [
        (upstream.as_str(), "licenses/by/4.0", Some("Example"), None),
        (
            "0000000000000000000000000000000000000000",
            "licenses/by/4.0",
            Some("Example"),
            Some("photo_revision_mismatch"),
        ),
        (upstream.as_str(), "licenses/by/4.0", None, Some("photo_creator_missing")),
        (upstream.as_str(), "licenses/by-sa/3.0", Some("<span> </span>"), Some("photo_creator_missing")),
        (upstream.as_str(), "publicdomain/zero/1.0", None, None),
    ] {
        let metadata = json!({"query":{"pages":{"1":{"title":"File:Image.png","imageinfo":[{
            "url":"https://upload.wikimedia.org/image.png", "descriptionurl":"https://commons.wikimedia.org/wiki/File:Image.png",
            "timestamp":"2026-01-01T00:00:00Z", "sha1":expected_sha1,
            "extmetadata":{"Artist":{"value":artist}, "Credit":{"value":"Own work"},
                "LicenseUrl":{"value":format!("https://creativecommons.org/{license}/")}}
        }]}}}});
        let raw = serde_json::to_vec(&metadata).unwrap();
        fs::write(root.join("metadata.json"), &raw).unwrap();
        let sources = [
            Source {
                path: "image.png".into(),
                url: "https://upload.wikimedia.org/image.png".into(),
                bytes: bytes.len() as u64,
                sha256: hash(&bytes),
            },
            Source {
                path: "metadata.json".into(),
                url: "https://commons.wikimedia.org/w/api.php".into(),
                bytes: raw.len() as u64,
                sha256: hash(&raw),
            },
        ];
        let result = assets::photo(&root, &sources, &capture, &allowed, "Q1");
        if let Some(error) = error {
            assert_eq!(result.unwrap_err(), error);
        } else {
            assert_eq!(result.unwrap().0.bytes, 51_840);
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn offline_compiler_preserves_colocated_sites_and_boundary_fallback_with_no_photo() {
    let root = obcm_testkit::scratch::scratch_dir("landmarks", "compile");
    let mut sources = Vec::new();
    let mut pin = |path: &str, url: &str, bytes: Vec<u8>| {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, &bytes).unwrap();
        sources.push(json!({"path":path,"url":url,"bytes":bytes.len(),"sha256":hash(&bytes)}));
    };
    pin(
        "classes/Q23413.json",
        "https://www.wikidata.org/wiki/Special:EntityData/Q23413.json",
        serde_json::to_vec(&json!({"entities":{"Q23413":{"claims":{"P279":[]}}}})).unwrap(),
    );
    pin(
        "classes/Q999-redirect.json",
        "https://www.wikidata.org/w/api.php?action=wbgetentities&ids=Q999&redirects=yes",
        serde_json::to_vec(&json!({"entities":{"Q999":{"id":"Q23413", "redirects":{"from":"Q999","to":"Q23413"}, "claims":{"P279":[]}}}})).unwrap(),
    );
    pin("locales/Q29.json", "https://www.wikidata.org/wiki/Special:EntityData/Q29.json",
        serde_json::to_vec(&json!({"entities":{"Q29":{"id":"Q29", "claims":{"P37":[{"mainsnak":{"datavalue":{"value":{"id":"Q1321"}}}}]}}}})).unwrap());
    let mut places = Vec::new();
    // One `wbgetentities` response holds both places, the way the capture batches them.
    let batch = "entities/batch-0.json";
    let mut entities = serde_json::Map::new();
    for qid in ["Q1", "Q2"] {
        let entity = json!({"id":qid,"labels":{"de":{"value":"Burg"}, "es":{"value":"Castillo"}},
        "sitelinks":{"dewiki":{"title":"Burg"}, "enwiki":{"title":"Castle"}, "eswiki":{"title":"Castillo"}},"claims":{
            "P17":[{"mainsnak":{"datavalue":{"value":{"id":"Q29"}}}}],
            "P31":[{"rank":"preferred","mainsnak":{"datavalue":{"value":{"id":"Q999"}}}},
                {"rank":"normal","mainsnak":{"datavalue":{"value":{"id":"Q35666"}}}}],
            "P625":[{"mainsnak":{"datavalue":{"value":{"latitude":0.0,"longitude":0.0,"globe":"http://www.wikidata.org/entity/Q2"}}}}]
        }});
        entities.insert(qid.into(), entity);
        let query = json!({"query":{"pages":{"1":{"title":"Burg","revisions":[{"revid":42}]}}}});
        let path = format!("articles/{qid}.json");
        pin(&path, "https://de.wikipedia.org/w/api.php", serde_json::to_vec(&query).unwrap());
        let html_path = format!("articles/{qid}.html");
        let url = "https://de.wikipedia.org/w/index.php?title=Burg&oldid=42";
        let html = r#"<script>{"wgRevisionId":42}</script><div id="mw-content-text"><div class="mw-parser-output"><p>Die Burg ist alt. Sie steht am Fluss.</p></div></div><div id="footer-info-copyright">Contributors <a href="https://creativecommons.org/licenses/by-sa/4.0/">CC BY-SA 4.0</a></div>"#;
        pin(&html_path, url, html.as_bytes().to_vec());
        let mut articles =
            vec![json!({"language":"de","title":"Burg","revision":42,"url":url,"path":path,"html_path":html_path})];
        for (language, title, body) in
            [("en", "Castle", "No complete sentence"), ("es", "Castillo", "El castillo es antiguo.")]
        {
            let path = format!("articles/{language}-{qid}.json");
            let html_path = format!("articles/{language}-{qid}.html");
            let url = format!("https://{language}.wikipedia.org/w/index.php?title={title}&oldid=42");
            pin(
                &path,
                &url,
                serde_json::to_vec(&json!({"query":{"pages":{"1":{"title":title,"revisions":[{"revid":42}]}}}}))
                    .unwrap(),
            );
            pin(&html_path, &url, html.replace("Die Burg ist alt. Sie steht am Fluss.", body).as_bytes().to_vec());
            articles.push(
                json!({"language":language,"title":title,"revision":42,"url":url,"path":path,"html_path":html_path}),
            );
        }
        places.push(json!({"qid":qid,"entity_path":batch,"articles":articles,"images":[]}));
    }
    pin(batch, "https://www.wikidata.org/", serde_json::to_vec(&json!({"entities":entities})).unwrap());
    let manifest = root.join("manifest.json");
    fs::write(&manifest, serde_json::to_vec(&json!({"schema":1,"sources":sources,"places":places})).unwrap()).unwrap();
    let boundary = root.join("boundary.json");
    fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}"#).unwrap();
    let first = compile(&manifest, &boundary, &root.join("first")).unwrap();
    assert_eq!(first.counts.candidates, 2);
    assert_eq!(first.counts.texts, 2);
    assert_eq!(first.counts.images, 0);
    assert_eq!(first.counts.mapped_approaches, None);
    assert_eq!(first.records.iter().map(|record| record.qid.as_str()).collect::<Vec<_>>(), ["Q1", "Q2"]);
    assert!(first.records.iter().all(|record| record.default_language == "es"
        && record.photo.is_none()
        && record.variants.len() == 2
        && record.fallback_sources == ["Q29"]));
    assert!(first
        .omissions
        .iter()
        .all(|omission| omission.reason == "no_usable_captured_image" || omission.reason.starts_with("en:")));
    assert!(first
        .records
        .iter()
        .all(|record| record.variants.iter().map(|v| v.language.as_str()).collect::<Vec<_>>() == ["de", "es"]));
    compile(&manifest, &boundary, &root.join("second")).unwrap();
    assert_eq!(fs::read(root.join("first/content.json")).unwrap(), fs::read(root.join("second/content.json")).unwrap());
    fs::write(root.join(batch), b"changed source").unwrap();
    assert!(compile(&manifest, &boundary, &root.join("changed")).unwrap_err().contains("source size changed"));
    fs::remove_dir_all(root).unwrap();
}

/// The compiler visits places in the order the capture asked for them, so the response it holds is
/// the one the next place needs. Lexicographic order would put `Q10` and `Q100` before `Q2` and
/// send it back to the first response, which costs a read, a digest and a parse of a whole batch.
#[test]
fn places_are_visited_in_the_order_the_capture_batched_them() {
    let root = obcm_testkit::scratch::scratch_dir("landmarks", "batch-order");
    let mut sources = Vec::new();
    let mut pin = |path: &str, bytes: Vec<u8>| {
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), &bytes).unwrap();
        sources.push(json!({"path":path,"url":"https://www.wikidata.org/","bytes":bytes.len(),"sha256":hash(&bytes)}));
    };
    pin("classes/Q23413.json", serde_json::to_vec(&json!({"entities":{"Q23413":{"claims":{"P279":[]}}}})).unwrap());
    // Two responses, filled the way the capture fills them: numeric QID order, fifty at a time.
    let batches: [(&str, &[&str]); 2] =
        [("entities/batch-0.json", &["Q2", "Q10"]), ("entities/batch-1.json", &["Q100"])];
    let mut places = Vec::new();
    for (path, qids) in batches {
        let qids = qids.to_vec();
        let mut entities = serde_json::Map::new();
        for qid in qids {
            entities.insert(qid.into(), json!({"id":qid,"labels":{},"sitelinks":{},"claims":{
                "P31":[{"mainsnak":{"datavalue":{"value":{"id":"Q23413"}}}}],
                "P625":[{"mainsnak":{"datavalue":{"value":{"latitude":0.5,"longitude":0.5,"globe":"http://www.wikidata.org/entity/Q2"}}}}]
            }}));
            places.push(json!({"qid":qid,"entity_path":path,"articles":[],"images":[]}));
        }
        pin(path, serde_json::to_vec(&json!({"entities":entities})).unwrap());
    }
    places.reverse();
    let manifest = root.join("manifest.json");
    fs::write(&manifest, serde_json::to_vec(&json!({"schema":1,"sources":sources,"places":places})).unwrap()).unwrap();
    let boundary = root.join("boundary.json");
    fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}"#).unwrap();

    let content = compile(&manifest, &boundary, &root.join("out")).unwrap();
    assert_eq!(content.candidate_qids, ["Q2", "Q10", "Q100"], "numeric order, whatever order the manifest lists");
    let batch_of = |qid: &str| batches.iter().position(|(_, qids)| qids.contains(&qid)).unwrap();
    let visited: Vec<usize> = content.candidate_qids.iter().map(|qid| batch_of(qid)).collect();
    assert!(visited.windows(2).all(|pair| pair[0] <= pair[1]), "a response is never returned to: {visited:?}");
    fs::remove_dir_all(root).unwrap();
}
