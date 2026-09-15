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
    let mut places = Vec::new();
    for qid in ["Q1", "Q2"] {
        let entity = json!({"entities":{qid:{"id":qid,"labels":{"de":{"value":"Burg"}},
        "sitelinks":{"dewiki":{"title":"Burg"}},"claims":{
            "P31":[{"rank":"preferred","mainsnak":{"datavalue":{"value":{"id":"Q999"}}}},
                {"rank":"normal","mainsnak":{"datavalue":{"value":{"id":"Q35666"}}}}],
            "P625":[{"mainsnak":{"datavalue":{"value":{"latitude":0.0,"longitude":0.0,"globe":"http://www.wikidata.org/entity/Q2"}}}}]
        }}}});
        pin(&format!("entities/{qid}.json"), "https://www.wikidata.org/", serde_json::to_vec(&entity).unwrap());
        let query = json!({"query":{"pages":{"1":{"title":"Burg","revisions":[{"revid":42}]}}}});
        let path = format!("articles/{qid}.json");
        pin(&path, "https://de.wikipedia.org/w/api.php", serde_json::to_vec(&query).unwrap());
        let html_path = format!("articles/{qid}.html");
        let url = "https://de.wikipedia.org/w/index.php?title=Burg&oldid=42";
        let html = r#"<script>{"wgRevisionId":42}</script><div id="mw-content-text"><div class="mw-parser-output"><p>Die Burg ist alt. Sie steht am Fluss.</p></div></div><div id="footer-info-copyright">Contributors <a href="https://creativecommons.org/licenses/by-sa/4.0/">CC BY-SA 4.0</a></div>"#;
        pin(&html_path, url, html.as_bytes().to_vec());
        places.push(json!({"qid":qid,"articles":[{"language":"de","title":"Burg","revision":42,"url":url,"path":path,"html_path":html_path}],"images":[]}));
    }
    let manifest = root.join("manifest.json");
    fs::write(&manifest, serde_json::to_vec(&json!({"schema":1,"sources":sources,"places":places})).unwrap()).unwrap();
    let boundary = root.join("boundary.json");
    fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}"#).unwrap();
    let first = compile(&manifest, &boundary, "en", &root.join("first")).unwrap();
    assert_eq!(first.counts.candidates, 2);
    assert_eq!(first.counts.texts, 2);
    assert_eq!(first.counts.images, 0);
    assert_eq!(first.counts.mapped_approaches, None);
    assert_eq!(first.records.iter().map(|record| record.qid.as_str()).collect::<Vec<_>>(), ["Q1", "Q2"]);
    assert!(first.records.iter().all(|record| record.language == "de" && record.photo.is_none()));
    assert!(first.omissions.iter().all(|omission| omission.reason == "no_usable_captured_image"));
    compile(&manifest, &boundary, "en", &root.join("second")).unwrap();
    assert_eq!(fs::read(root.join("first/content.json")).unwrap(), fs::read(root.join("second/content.json")).unwrap());
    fs::write(root.join("entities/Q1.json"), b"changed source").unwrap();
    assert!(compile(&manifest, &boundary, "en", &root.join("changed")).unwrap_err().contains("source size changed"));
    fs::remove_dir_all(root).unwrap();
}
