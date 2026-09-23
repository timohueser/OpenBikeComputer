use super::*;
use serde_json::json;

#[test]
fn commons_category_members_cover_every_page_in_stable_order() {
    let root = obcm_testkit::scratch::scratch_dir("landmarks", "category-pages");
    let continuation = json!({"cmcontinue":"file|next|7","continue":"-||"});
    let captures = [
        (
            "categories/category.json",
            json!({"continue":continuation,"query":{"categorymembers":[
                {"title":"File:B.jpg"},{"title":"File:A.jpg"}
            ]}}),
        ),
        (
            "categories/category-2.json",
            json!({"query":{"categorymembers":[{"title":"File:B.jpg"},{"title":"File:C.jpg"}]}}),
        ),
    ];
    let mut sources = Vec::new();
    for (path, value) in &captures {
        let bytes = serde_json::to_vec(value).unwrap();
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), &bytes).unwrap();
        sources.push(Source {
            path: (*path).into(),
            sha256: hash(&bytes),
            bytes: bytes.len() as u64,
            url: "https://commons.wikimedia.org/w/api.php".into(),
        });
    }
    let place = json!({"commons_categories":[{
        "title":"Category:Example",
        "complete":true,
        "pages":[
            {"path":captures[0].0,"continuation":null},
            {"path":captures[1].0,"continuation":continuation}
        ]
    }]});
    let members = category_members(&root, &sources, &place, &["Category:Example".into()], 2).unwrap();
    assert_eq!(members.into_iter().collect::<Vec<_>>(), ["A.jpg", "B.jpg", "C.jpg"]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn commons_category_rejects_missing_or_unconsumed_pages() {
    let root = obcm_testkit::scratch::scratch_dir("landmarks", "incomplete-category-pages");
    let continuation = json!({"cmcontinue":"file|next|7","continue":"-||"});
    let raw = json!({"continue":continuation,"query":{"categorymembers":[{"title":"File:A.jpg"}]}});
    let bytes = serde_json::to_vec(&raw).unwrap();
    let path = "categories/category.json";
    fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
    fs::write(root.join(path), &bytes).unwrap();
    let sources = [Source {
        path: path.into(),
        sha256: hash(&bytes),
        bytes: bytes.len() as u64,
        url: "https://commons.wikimedia.org/w/api.php".into(),
    }];
    let listing = |pages| {
        json!({"commons_categories":[{
            "title":"Category:Example","complete":true,"pages":pages
        }]})
    };
    let unconsumed = listing(json!([{"path":path,"continuation":null}]));
    assert!(category_members(&root, &sources, &unconsumed, &["Category:Example".into()], 2)
        .unwrap_err()
        .contains("unconsumed"));
    let missing = listing(json!([
        {"path":path,"continuation":null},
        {"path":"categories/missing.json","continuation":continuation}
    ]));
    assert!(category_members(&root, &sources, &missing, &["Category:Example".into()], 2)
        .unwrap_err()
        .contains("unregistered source"));
    let failed = json!({"commons_categories":[{
        "title":"Category:Example","complete":false,"pages":[
            {"path":path,"continuation":null},
            {"path":"categories/missing.json","continuation":continuation}
        ]
    }]});
    assert!(category_members(&root, &sources, &failed, &["Category:Example".into()], 2).unwrap().is_empty());
    let legacy = json!({"commons_categories":[{"title":"Category:Example","path":path}]});
    assert!(category_members(&root, &sources, &legacy, &["Category:Example".into()], 1)
        .unwrap_err()
        .contains("schema 1 cannot prove complete commons category coverage"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn commons_category_rejects_a_page_after_the_terminal_response() {
    let root = obcm_testkit::scratch::scratch_dir("landmarks", "page-after-terminal");
    let captures = [
        ("categories/category.json", json!({"query":{"categorymembers":[{"title":"File:A.jpg"}]}})),
        ("categories/category-2.json", json!({"query":{"categorymembers":[{"title":"File:Injected.jpg"}]}})),
    ];
    let mut sources = Vec::new();
    for (path, value) in &captures {
        let bytes = serde_json::to_vec(value).unwrap();
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), &bytes).unwrap();
        sources.push(Source {
            path: (*path).into(),
            sha256: hash(&bytes),
            bytes: bytes.len() as u64,
            url: "https://commons.wikimedia.org/w/api.php".into(),
        });
    }
    let place = json!({"commons_categories":[{
        "title":"Category:Example","complete":true,"pages":[
            {"path":captures[0].0,"continuation":null},
            {"path":captures[1].0,"continuation":null}
        ]
    }]});
    assert!(category_members(&root, &sources, &place, &["Category:Example".into()], 2)
        .unwrap_err()
        .contains("page follows a terminal response"));
    fs::remove_dir_all(root).unwrap();
}

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
fn offline_compiler_and_filtered_photo_requests_preserve_production_order() {
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
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2).write_to(&mut encoded, image::ImageFormat::Png).unwrap();
    let usable_photo = encoded.into_inner();
    // One `wbgetentities` response holds both places, the way the capture batches them.
    let batch = "entities/batch-0.json";
    let mut entities = serde_json::Map::new();
    for qid in ["Q1", "Q2", "Q3"] {
        let filenames: &[&str] = match qid {
            "Q1" => &["Settled.png"],
            "Q2" => &["A-rejected.png", "B-fallback.png"],
            _ => &["Unrelated.png"],
        };
        let entity = json!({"id":qid,"labels":{"de":{"value":"Burg"}, "es":{"value":"Castillo"}},
        "sitelinks":{"dewiki":{"title":"Burg"}, "enwiki":{"title":"Castle"}, "eswiki":{"title":"Castillo"}},"claims":{
            "P17":[{"mainsnak":{"datavalue":{"value":{"id":"Q29"}}}}],
            "P31":[{"rank":"preferred","mainsnak":{"datavalue":{"value":{"id":"Q999"}}}},
                {"rank":"normal","mainsnak":{"datavalue":{"value":{"id":"Q35666"}}}}],
            "P18":filenames.iter().map(|filename| json!({"mainsnak":{"datavalue":{"value":filename}}})).collect::<Vec<_>>(),
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
        let mut images = Vec::new();
        for filename in filenames {
            let metadata_path = format!("photos/{qid}-{filename}.json");
            let original = match (qid, *filename) {
                ("Q1", _) => Some(usable_photo.clone()),
                ("Q2", "A-rejected.png") => Some(b"not an image".to_vec()),
                _ => None,
            };
            let sha1 = original.as_ref().map(|bytes| {
                <sha1::Sha1 as sha1::Digest>::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>()
            });
            let metadata = json!({"query":{"pages":{"1":{"title":format!("File:{filename}"),"imageinfo":[{
                "url":format!("https://upload.wikimedia.org/{filename}"),"descriptionurl":format!("https://commons.wikimedia.org/wiki/File:{filename}"),
                "timestamp":"2026-01-01T00:00:00Z","sha1":sha1.clone().unwrap_or_else(|| "0".repeat(40)),
                "extmetadata":{"Artist":{"value":"Example"},"Credit":{"value":"Own work"},"LicenseUrl":{"value":"https://creativecommons.org/licenses/by/4.0/"}}
            }]}}}});
            pin(&metadata_path, "https://commons.wikimedia.org/w/api.php", serde_json::to_vec(&metadata).unwrap());
            let mut image = json!({"source":"P18","metadata_path":metadata_path});
            if let Some(bytes) = original {
                let path = format!("photos/{qid}-{filename}");
                pin(&path, &format!("https://upload.wikimedia.org/{filename}"), bytes);
                image["path"] = path.into();
            }
            images.push(image);
        }
        places.push(json!({"qid":qid,"entity_path":batch,"articles":articles,"images":images}));
    }
    pin(batch, "https://www.wikidata.org/", serde_json::to_vec(&json!({"entities":entities})).unwrap());
    let manifest = root.join("manifest.json");
    fs::write(&manifest, serde_json::to_vec(&json!({"schema":1,"sources":sources,"places":places})).unwrap()).unwrap();
    let boundary = root.join("boundary.json");
    fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}"#).unwrap();
    let first = compile(&manifest, &boundary, &root.join("first"), false).unwrap();
    assert_eq!(first.counts.candidates, 3);
    assert_eq!(first.counts.texts, 3);
    assert_eq!(first.counts.images, 1);
    assert_eq!(first.counts.mapped_approaches, None);
    assert_eq!(first.records.iter().map(|record| record.qid.as_str()).collect::<Vec<_>>(), ["Q1", "Q2", "Q3"]);
    assert!(first.records.iter().all(|record| record.default_language == "es"
        && record.variants.len() == 2
        && record.fallback_sources == ["Q29"]));
    assert!(first.records[0].photo.is_some());
    assert!(first.records[1..].iter().all(|record| record.photo.is_none()));
    assert!(first.omissions.iter().all(|omission| omission.reason == "no_usable_captured_image"
        || omission.reason == "image_format"
        || omission.reason.starts_with("en:")));
    assert!(first
        .records
        .iter()
        .all(|record| record.variants.iter().map(|v| v.language.as_str()).collect::<Vec<_>>() == ["de", "es"]));
    compile(&manifest, &boundary, &root.join("second"), false).unwrap();
    assert_eq!(fs::read(root.join("first/content.json")).unwrap(), fs::read(root.join("second/content.json")).unwrap());
    let full_output = root.join("full-requests");
    let full = photo_requests(&manifest, &boundary, &full_output, None).unwrap();
    let request_output = root.join("requests");
    let requests = photo_requests(&manifest, &boundary, &request_output, Some(&BTreeSet::from(["Q2".into()]))).unwrap();
    let expected: Vec<_> = full.requests.iter().filter(|request| request.qid == "Q2").collect();
    assert_eq!(full.requests.iter().map(|request| request.qid.as_str()).collect::<Vec<_>>(), ["Q2", "Q3"]);
    assert_eq!(requests.requests.len(), 1);
    assert_eq!(requests.requests[0].metadata_path, "photos/Q2-B-fallback.png.json");
    assert_eq!(serde_json::to_value(&requests.requests).unwrap(), serde_json::to_value(expected).unwrap());
    assert_eq!(fs::read_dir(&request_output).unwrap().count(), 1);
    assert!(request_output.join(PHOTO_REQUESTS_DOC).is_file());
    fs::write(root.join(batch), b"changed source").unwrap();
    assert!(compile(&manifest, &boundary, &root.join("changed"), false).unwrap_err().contains("source size changed"));
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

    let content = compile(&manifest, &boundary, &root.join("out"), false).unwrap();
    assert_eq!(content.candidate_qids, ["Q2", "Q10", "Q100"], "numeric order, whatever order the manifest lists");
    let batch_of = |qid: &str| batches.iter().position(|(_, qids)| qids.contains(&qid)).unwrap();
    let visited: Vec<usize> = content.candidate_qids.iter().map(|qid| batch_of(qid)).collect();
    assert!(visited.windows(2).all(|pair| pair[0] <= pair[1]), "a response is never returned to: {visited:?}");
    fs::remove_dir_all(root).unwrap();
}
