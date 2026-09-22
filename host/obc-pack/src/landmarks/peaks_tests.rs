use super::*;
use serde_json::json;

struct Fixture {
    root: std::path::PathBuf,
    sources: Vec<Value>,
}
impl Fixture {
    fn pin(&mut self, path: &str, bytes: Vec<u8>) {
        let target = self.root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, &bytes).unwrap();
        self.sources.push(
            json!({"path":path,"url":format!("https://example.test/{path}"),"bytes":bytes.len(),"sha256":hash(&bytes)}),
        );
    }
    fn json(&mut self, path: &str, value: Value) {
        self.pin(path, serde_json::to_vec(&value).unwrap());
    }
    /// One captured Commons file: distinct pixels, its own credits, categories, camera and depicts.
    fn photo(
        &mut self,
        name: &str,
        shade: u8,
        license: &str,
        categories: &[&str],
        camera: Option<(f64, f64)>,
        depicts: Option<&str>,
    ) -> Value {
        let mut buffer = std::io::Cursor::new(vec![]);
        let pixels = image::RgbImage::from_pixel(2, 2, image::Rgb([shade, 0, 0]));
        image::DynamicImage::ImageRgb8(pixels).write_to(&mut buffer, image::ImageFormat::Png).unwrap();
        let bytes = buffer.into_inner();
        let sha1 = <sha1::Sha1 as sha1::Digest>::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
        let path = format!("photos/{name}.png");
        self.pin(&path, bytes);
        let mut extended = json!({"Artist":{"value":"Example"},"LicenseUrl":{"value":license}});
        if let Some((latitude, longitude)) = camera {
            extended["GPSLatitude"] = json!({"value": latitude.to_string()});
            extended["GPSLongitude"] = json!({"value": longitude.to_string()});
        }
        let metadata = format!("meta/{name}.json");
        self.json(
            &metadata,
            json!({"query":{"pages":{"1":{"pageid":1,"title":format!("File:{name}.png"),
            "categories": categories.iter().map(|title| json!({"title": title})).collect::<Vec<_>>(),
            "imageinfo":[{"url":format!("https://example.test/{path}"),
            "descriptionurl":format!("https://commons.wikimedia.org/wiki/File:{name}.png"),
            "timestamp":"2026-09-22T00:00:00Z","sha1":sha1,"extmetadata":extended}]}}}}),
        );
        let mut image = json!({"filename":format!("{name}.png"),"path":path,"metadata_path":metadata});
        if let Some(qid) = depicts {
            let media = format!("media/{name}.json");
            self.json(
                &media,
                json!({"entities":{"M1":{"statements":{"P180":[{"mainsnak":{"datavalue":{"value":{"id":qid}}}}]}}}}),
            );
            image["depicts_path"] = json!(media);
        }
        image
    }
    fn article(&mut self, id: &str, language: &str, title: &str, body: &str) -> Value {
        let path = format!("articles/{id}-{language}.json");
        let html = format!("articles/{id}-{language}.html");
        self.json(&path, json!({"query":{"pages":{"42":{"pageid":42,"title":title,"revisions":[{"revid":24}]}}}}));
        self.pin(&html,format!(r#"<script>{{"wgRevisionId":24}}</script><div id="mw-content-text"><div class="mw-parser-output"><p>{body}</p></div></div><div id="footer-info-copyright">Contributors <a href="https://creativecommons.org/licenses/by-sa/4.0/">CC BY-SA 4.0</a></div>"#).into_bytes());
        if language == "es" {
            let original = fs::read_to_string(self.root.join(&html)).unwrap();
            self.sources.retain(|source| source["path"] != html);
            self.pin(&html, original.replace("https://creativecommons.org/licenses/by-sa/4.0/", "/wiki/Wikipedia:Texto_de_la_Licencia_Creative_Commons_Atribuci%C3%B3n-CompartirIgual_4.0_Internacional").into_bytes());
        }
        json!({"language":language,"title":title,"revision":24,"path":path,"html_path":html,"url":format!("https://example.test/{html}")})
    }
}

#[test]
fn peak_catalogue_keeps_explicit_associations_and_shared_assets_independent_of_entity_location() {
    let mut f = Fixture { root: obcm_testkit::scratch::scratch_dir("landmarks", "peaks"), sources: vec![] };
    let mut places = vec![];
    for (id, body) in [("Q1", "A mountain massif. It has two summits."), ("Q2", "No sentence"), ("Q3", "A small hill.")]
    {
        // Q1 has no P625. Q3's article entity is far outside the OSM boundary.
        f.json(
            &format!("entities/{id}.json"),
            json!({"entities":{id:{"id":id,"labels":{"en":{"value":"Mountain"}},
                "sitelinks":{"enwiki":{"title":"Mountain"},"dewiki":{"title":"Berg"},"eswiki":{"title":"Montaña"}},"claims":{
                "P625":[{"mainsnak":{"datavalue":{"value":{"latitude":80,"longitude":100}}}}],
                "P18":[{"mainsnak":{"datavalue":{"value":"Photo.png"}}}]
            }}}}),
        );
        if id == "Q1" {
            let path = f.root.join("entities/Q1.json");
            let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            value["entities"]["Q1"]["claims"].as_object_mut().unwrap().remove("P625");
            f.sources.retain(|s| s["path"] != "entities/Q1.json");
            f.json("entities/Q1.json", value);
        }
        let mut articles = vec![f.article(id, "en", "Mountain", body)];
        if id == "Q1" {
            articles.push(f.article(id, "de", "Berg", "Ein hoher Berg."));
            articles.push(f.article(id, "es", "Montaña", "Una gran montaña."));
        }
        places.push(json!({"qid":id,"articles":articles,"images":[]}));
    }
    let mut pixels = std::io::Cursor::new(vec![]);
    image::DynamicImage::new_rgb8(2, 2).write_to(&mut pixels, image::ImageFormat::Png).unwrap();
    let bytes = pixels.into_inner();
    let sha1 = <sha1::Sha1 as sha1::Digest>::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
    f.pin("photo.png", bytes);
    for (id, license) in
        [("good", "https://creativecommons.org/licenses/by/4.0/"), ("bad", "https://example.test/nonfree")]
    {
        f.json(&format!("{id}.json"),json!({"query":{"pages":{"1":{"title":"File:Photo.png","imageinfo":[{
            "url":"https://example.test/photo.png","descriptionurl":"https://commons.wikimedia.org/wiki/File:Photo.png","timestamp":"2026-09-16T00:00:00Z","sha1":sha1,
            "extmetadata":{"Artist":{"value":"Example"},"LicenseUrl":{"value":license}}
        }]}}}}));
    }
    places[0]["images"] =
        json!([{"source":"P18","filename":"Photo.png","path":"photo.png","metadata_path":"good.json"}]);
    places[2]["images"] =
        json!([{"source":"P18","filename":"Photo.png","path":"photo.png","metadata_path":"bad.json"}]);
    let direct = f.article("wiki-en-42", "en", "Hill", "An isolated hill.");
    places.push(json!({"qid":"wiki-en-42","articles":[direct],"images":[]}));
    f.json("links/Q10.json", json!({"entities":{"Q10":{"id":"Q1","redirects":{"from":"Q10","to":"Q1"}}}}));
    for id in ["Q2", "Q3"] {
        f.json(&format!("links/{id}.json"), json!({"entities":{id:{"id":id}}}));
    }
    f.json("links/en.json",json!({"query":{"redirects":[{"from":"Old summit","to":"Mountain"}],"pages":{"1":{"pageid":1,"title":"Mountain","pageprops":{"wikibase_item":"Q1"}}}}}));
    f.json(
        "links/it.json",
        json!({"query":{"pages":{"2":{"pageid":2,"title":"Collina","langlinks":[{"lang":"en","*":"Hill"}]}}}}),
    );
    f.json(
        "links/hill.json",
        json!({"query":{"pages":{"42":{"pageid":42,"title":"Hill","langlinks":[{"lang":"it","*":"Collina"}]}}}}),
    );
    let nodes:Vec<_>=[(1,"wikidata","Q10"),(2,"wikipedia","en:Old summit"),(3,"wikidata","Q3"),(4,"wikidata","Q2"),(5,"note","unlinked"),(6,"wikipedia","it:Collina")].into_iter().map(|(id,k,v)|json!({"node_id":id,"latitude":0.5,"longitude":0.5,"tags":{"natural":"peak","name":"Original full summit name",k:v}})).collect();
    f.json("summits.json", json!({"schema":1,"osm_sha256":"a".repeat(64),"summits":nodes}));
    let mut resolutions: Vec<_> = [
        (1, "wikidata", "Q10"),
        (2, "wikipedia", "en"),
        (3, "wikidata", "Q3"),
        (4, "wikidata", "Q2"),
        (6, "wikipedia", "it"),
    ]
    .into_iter()
    .map(|(id, kind, path)| json!({"node_id":id,"kind":kind,"path":format!("links/{path}.json"),"status":"resolved"}))
    .collect();
    resolutions[4]["canonical_path"] = json!("links/hill.json");
    resolutions[4]["canonical_language"] = json!("en");
    let manifest = f.root.join("manifest.json");
    fs::write(&manifest,serde_json::to_vec(&json!({"schema":1,"sources":f.sources,"places":places,"peaks":{"summits_path":"summits.json","resolutions":resolutions}})).unwrap()).unwrap();
    let boundary = f.root.join("boundary.json");
    fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}"#).unwrap();
    let first = peaks::compile(&manifest, &boundary, &f.root.join("first"), false).unwrap();
    assert_eq!((first.counts.captured, first.counts.candidates, first.counts.texts, first.counts.images), (6, 5, 3, 1));
    assert_eq!(
        first.associations.iter().map(|a| (a.node_id, a.article_id.as_str())).collect::<Vec<_>>(),
        [(1, "Q1"), (2, "Q1"), (3, "Q3"), (6, "wiki-en-42")]
    );
    assert_eq!(first.records[0].article.variants.len(), 3);
    assert!(first.records[2].article.variants.iter().all(|v| v.language == "en"));
    assert!(first.omissions.iter().any(|o| o.reason == "no_explicit_link"));
    assert!(first.omissions.iter().any(|o| o.qid == "Q2" && o.reason == "no_usable_captured_language"));
    assert!(first.omissions.iter().any(|o| o.qid == "Q3" && o.reason == "unsupported_license"));
    assert_eq!(fs::read_dir(f.root.join("first")).unwrap().count(), 2);
    assert!(super::compile(&manifest, &boundary, &f.root.join("landmarks"), false)
        .unwrap_err()
        .contains("peak compiler"));
    assert!(serde_json::from_slice::<Content>(&fs::read(f.root.join("first/peaks.json")).unwrap()).is_err());
    peaks::compile(&manifest, &boundary, &f.root.join("second"), false).unwrap();
    for file in fs::read_dir(f.root.join("first")).unwrap() {
        let file = file.unwrap();
        assert_eq!(fs::read(file.path()).unwrap(), fs::read(f.root.join("second").join(file.file_name())).unwrap());
    }
    fs::remove_dir_all(f.root).unwrap();
}

#[test]
fn peak_discovery_matches_the_map_classifier_and_ignores_way_and_unnamed_objects() {
    let tags = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    assert!(peaks::is_summit(&tags(&[("natural", "peak"), ("name", "Mönch")])));
    assert!(!peaks::is_summit(&tags(&[("natural", "peak")])));
    assert!(!peaks::is_summit(&tags(&[("natural", "peak"), ("name", " ")])));
    assert!(!peaks::is_summit(&tags(&[("natural", "peak"), ("name", "Hill"), ("amenity", "drinking_water")])));
    let root = obcm_testkit::scratch::scratch_dir("landmarks", "peak-discovery");
    let boundary = root.join("boundary.json");
    fs::write(
        &boundary,
        r#"{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}}"#,
    )
    .unwrap();
    let osm = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/peak-discovery.osm.pbf"));
    peaks::discover(osm, &boundary, &root.join("summits.json")).unwrap();
    let source: peaks::SummitSource = serde_json::from_slice(&fs::read(root.join("summits.json")).unwrap()).unwrap();
    assert_eq!(source.summits.iter().map(|s| s.node_id).collect::<Vec<_>>(), [1]);
    assert_eq!(source.summits[0].tags["name"], "A summit name longer than twenty four bytes");
    assert_eq!(source.summits[0].tags["wikidata"], "Q1");
    assert_eq!(source.osm_sha256, hash(&fs::read(osm).unwrap()));
    fs::remove_dir_all(root).unwrap();
}

const FREE: &str = "https://creativecommons.org/licenses/by/4.0/";

/// Each peak isolates one rule, so the expected photo of a peak changes if that rule is removed.
#[test]
fn peak_photo_ranking_shows_the_peak_and_a_rejected_candidate_is_not_the_end() {
    let mut f = Fixture { root: obcm_testkit::scratch::scratch_dir("landmarks", "peak-photos"), sources: vec![] };
    let (views, fallback, camera, depicts, mut wanted) = {
        let mut make = |name: &str,
                        shade: u8,
                        license: &str,
                        categories: &[&str],
                        camera: Option<(f64, f64)>,
                        depicts: Option<&str>,
                        kind: &str| {
            let mut value = f.photo(name, shade, license, categories, camera, depicts);
            value["source"] = json!(kind);
            value
        };
        (
            // A view of the peak outranks a plain image claim. A subcategory of views from the peak
            // is refused outright.
            vec![
                make("c", 30, FREE, &["Category:Views of Alpspitz"], None, None, "P4291"),
                make("d", 40, FREE, &[], None, None, "P18"),
                make("e", 50, FREE, &["Category:Views from the Alpspitz in winter"], None, None, "P18"),
            ],
            // A file that is only in the views subcategory is not a member of the entity's own
            // category, so the category pool cannot admit it. The first claim is unusable and the
            // second is not.
            vec![
                make("g", 60, FREE, &["Category:Views of Hochblassen"], None, None, "commons-category"),
                make("x", 70, "https://example.test/nonfree", &[], None, None, "P18"),
                make("y", 80, FREE, &[], None, None, "P18"),
            ],
            // The camera of the alphabetically first claim stands on the summit.
            vec![make("n", 90, FREE, &[], Some((0.5, 0.5)), None, "P18"), make("p", 100, FREE, &[], None, None, "P18")],
            // A category member that depicts the peak outranks a plain image claim.
            vec![
                make("h", 110, FREE, &["Category:Watzmann"], None, Some("Q8"), "commons-category"),
                make("m", 120, FREE, &[], None, None, "P18"),
            ],
            // Metadata for three candidates and bytes for none of them.
            vec![
                make("r", 130, FREE, &[], None, None, "P18"),
                make("s", 140, FREE, &[], None, None, "P18"),
                make("t", 150, FREE, &[], None, None, "P18"),
            ],
        )
    };
    let claim =
        |files: &[&str]| files.iter().map(|file| json!({"mainsnak":{"datavalue":{"value":file}}})).collect::<Vec<_>>();
    for image in &mut wanted {
        image.as_object_mut().unwrap().remove("path");
    }
    let mut places = vec![];
    let peaks = [
        ("Q5", "Alpspitz", views, claim(&["d.png", "e.png"]), claim(&["c.png"])),
        ("Q6", "Hochblassen", fallback, claim(&["x.png", "y.png"]), claim(&[])),
        ("Q7", "Zugspitze", camera, claim(&["n.png", "p.png"]), claim(&[])),
        ("Q8", "Watzmann", depicts, claim(&["m.png"]), claim(&[])),
        ("Q9", "Hochkalter", wanted, claim(&["r.png", "s.png", "t.png"]), claim(&[])),
    ];
    for (id, name, images, lead, panorama) in peaks {
        f.json(
            &format!("entities/{id}.json"),
            json!({"entities":{id:{"id":id,"labels":{"en":{"value":name}},"sitelinks":{"enwiki":{"title":name}},
                "claims":{
                    "P625":[{"mainsnak":{"datavalue":{"value":{"latitude":0.5,"longitude":0.5}}}}],
                    "P373":[{"mainsnak":{"datavalue":{"value":name}}}],
                    "P18":lead,
                    "P4291":panorama}}}}),
        );
        let article = f.article(id, "en", name, "A limestone summit.");
        let mut place = json!({"qid":id,"articles":[article],"images":images});
        if let Some(member) = place["images"].as_array().unwrap().iter().find(|i| i["source"] == "commons-category") {
            let listing = format!("categories/{id}.json");
            f.json(&listing,json!({"query":{"categorymembers":[{"title":format!("File:{}", member["filename"].as_str().unwrap())}]}}));
            place["commons_categories"] = json!([{"title": format!("Category:{name}"), "path": listing}]);
        }
        places.push(place);
        f.json(&format!("links/{id}.json"), json!({"entities":{id:{"id":id}}}));
    }
    let linked = [(1i64, "Q5"), (2, "Q6"), (3, "Q7"), (4, "Q8"), (5, "Q9")];
    let nodes: Vec<_> = linked.iter().map(|(id, qid)| {
        json!({"node_id":id,"latitude":0.5,"longitude":0.5,"tags":{"natural":"peak","name":"Summit","wikidata":qid}})
    }).collect();
    f.json("summits.json", json!({"schema":1,"osm_sha256":"a".repeat(64),"summits":nodes}));
    let resolutions: Vec<_> = linked
        .iter()
        .map(|(id, qid)| json!({"node_id":id,"kind":"wikidata","path":format!("links/{qid}.json"),"status":"resolved"}))
        .collect();
    let manifest = f.root.join("manifest.json");
    fs::write(&manifest,serde_json::to_vec(&json!({"schema":1,"sources":f.sources,"places":places,"peaks":{"summits_path":"summits.json","resolutions":resolutions}})).unwrap()).unwrap();
    let boundary = f.root.join("boundary.json");
    fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}"#).unwrap();
    let result = peaks::compile(&manifest, &boundary, &f.root.join("out"), true).unwrap();
    let chosen =
        |name: &str| hash(&photo::prepare(&fs::read(f.root.join(format!("photos/{name}.png"))).unwrap()).unwrap());
    let selected: Vec<_> =
        result.records.iter().filter_map(|r| r.article.photo.as_ref()).map(|p| p.sha256.clone()).collect();
    assert_eq!(selected, ["c", "y", "p", "h"].map(chosen));
    // Q9 has no acquired bytes, so the compiler asks for the two best and says nothing about a
    // missing photo until the capture has answered.
    assert_eq!(
        result.photo_requests.iter().map(|r| (r.qid.as_str(), r.filename.as_str())).collect::<Vec<_>>(),
        [("Q9", "r.png"), ("Q9", "s.png")]
    );
    assert!(result.omissions.iter().all(|o| o.qid != "Q9"));
    // A compile no capture drives asks for nothing and says why the record has no photo.
    let shipped = peaks::compile(&manifest, &boundary, &f.root.join("shipped"), false).unwrap();
    assert!(shipped.photo_requests.is_empty());
    assert!(shipped.omissions.iter().any(|o| o.qid == "Q9" && o.reason == "no_usable_captured_image"));
    let document = fs::read_to_string(f.root.join("shipped/peaks.json")).unwrap();
    assert!(!document.contains("photo_requests"), "a shipped catalogue carries no request list");
    let reasons: BTreeSet<_> = result.omissions.iter().map(|o| o.reason.as_str()).collect();
    assert!(reasons.contains("views_from_the_site"), "a view from the summit is refused");
    assert!(reasons.contains("photo_identity_mismatch"), "a category member proves its own membership");
    assert!(reasons.contains("unsupported_license"));
    fs::remove_dir_all(f.root).unwrap();
}

/// A linked summit with a photo and no article in any supported language.
#[test]
fn a_photo_alone_is_a_peak_record_and_never_a_landmark() {
    let mut f = Fixture { root: obcm_testkit::scratch::scratch_dir("landmarks", "photo-only"), sources: vec![] };
    let image = f.photo("s", 20, FREE, &[], None, None);
    let entity = json!({"entities":{"Q4":{"id":"Q4","labels":{"de":{"value":"Schafberg"}},"sitelinks":{},"claims":{
        "P625":[{"mainsnak":{"datavalue":{"value":{"latitude":0.5,"longitude":0.5,"globe":"http://www.wikidata.org/entity/Q2"}}}}],
        "P31":[{"mainsnak":{"datavalue":{"value":{"id":"Q23413"}}}}],
        "P18":[{"mainsnak":{"datavalue":{"value":"s.png"}}}]}}}});
    f.json("entities/Q4.json", entity);
    f.json("links/Q4.json", json!({"entities":{"Q4":{"id":"Q4"}}}));
    f.json("classes/Q23413.json", json!({"entities":{"Q23413":{"claims":{"P279":[]}}}}));
    let mut place = json!({"qid":"Q4","name":"Schafberg","articles":[],"images":[image]});
    place["images"][0]["source"] = json!("P18");
    let summit = json!({"node_id":1,"latitude":0.5,"longitude":0.5,"tags":{"natural":"peak","name":"Schafberg","wikidata":"Q4"}});
    f.json("summits.json", json!({"schema":1,"osm_sha256":"a".repeat(64),"summits":[summit]}));
    let resolutions = json!([{"node_id":1,"kind":"wikidata","path":"links/Q4.json","status":"resolved"}]);
    let boundary = f.root.join("boundary.json");
    fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}"#).unwrap();
    let mut snapshot = json!({"schema":1,"sources":f.sources,"places":[place]});
    let landmarks = f.root.join("landmarks.json");
    fs::write(&landmarks, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    snapshot["peaks"] = json!({"summits_path":"summits.json","resolutions":resolutions});
    let manifest = f.root.join("manifest.json");
    fs::write(&manifest, serde_json::to_vec(&snapshot).unwrap()).unwrap();

    let peaks = peaks::compile(&manifest, &boundary, &f.root.join("peaks"), false).unwrap();
    assert_eq!((peaks.counts.texts, peaks.counts.images), (0, 1));
    let record = &peaks.records[0].article;
    assert!(record.variants.is_empty() && record.default_language.is_empty() && record.fallback_sources.is_empty());
    assert_eq!((record.name.as_str(), record.photo.is_some()), ("Schafberg", true));
    assert_eq!(peaks.associations.iter().map(|a| a.node_id).collect::<Vec<_>>(), [1]);
    assert!(peaks.omissions.iter().any(|o| o.qid == "Q4" && o.reason == "no_usable_captured_language"));

    let sites = super::compile(&landmarks, &boundary, &f.root.join("sites"), false).unwrap();
    assert!(sites.records.is_empty() && sites.counts.candidates == 1);
    assert!(sites.omissions.iter().any(|o| o.qid == "Q4" && o.reason == "no_usable_captured_language"));
    assert_eq!(fs::read_dir(f.root.join("sites")).unwrap().count(), 1, "a refused landmark writes no photo");
    fs::remove_dir_all(f.root).unwrap();
}
