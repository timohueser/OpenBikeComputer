//! Whole-map source preparation, combining drawing harvest and graph construction.

use obc_draw::ingest::ingest_osm_ways;
use obc_map_core::config::Config;
use obc_map_core::progress::Progress;
use obc_network::nav::{self, NavGraph};
use obc_pbf::bbox::Bbox;

pub struct Ingested {
    pub source: obc_draw::ingest::Ingested,
    pub nav_graph: NavGraph,
}

pub fn ingest_osm(
    paths: &[String],
    config: &Config,
    bbox: Option<Bbox>,
    progress: &Progress,
) -> Result<Ingested, String> {
    let (source, ways) = ingest_osm_ways(paths, config, bbox, progress)?;
    let (nav_graph, stats) = nav::build_graph_with(&ways, config.routing.min_component_edges);
    progress.log(nav::format_summary(&nav_graph, &stats));
    Ok(Ingested { source, nav_graph })
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_draw::geom::Geom;
    use obc_pbf::scan::Keyed;
    use obc_pbf::selection::IdSet;
    use std::collections::HashMap;

    const TINY_PBF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny.osm.pbf");

    fn is_polygon(g: &Geom) -> bool {
        matches!(g, Geom::Polygon { .. })
    }

    fn sources(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_string()).collect()
    }

    fn quiet() -> Progress {
        Progress::silent()
    }

    fn assert_metadata(paths: &[String], config: &Config, ingested: &Ingested) {
        let metadata = obc_places::osm::harvest(paths, config, &quiet()).expect("metadata harvest");
        assert_eq!(metadata.pois, ingested.source.pois, "place order, identities, schedules and approaches");
        assert_eq!(metadata.links.len(), ingested.source.landmark_links.len());
        for (a, b) in metadata.links.iter().zip(&ingested.source.landmark_links) {
            assert_eq!(
                (&a.metadata, a.position, &a.wikidata, &a.wikipedia, &a.hours),
                (&b.metadata, b.position, &b.wikidata, &b.wikipedia, &b.hours),
                "article links and approach selection",
            );
        }
        let (graph, _) = nav::build_graph_with(&metadata.ways, config.routing.min_component_edges);
        assert_eq!(graph.nodes, ingested.nav_graph.nodes, "the same source topology");
        assert_eq!(graph.edges, ingested.nav_graph.edges, "the same geometry and way kinds");
    }

    /// Everything an ingest produced, flattened into one comparable value — enough to say "these
    /// two runs are the same map", including order, which decides the packed bytes downstream.
    fn shape(ing: &Ingested) -> Vec<String> {
        let mut out: Vec<String> = ing
            .source
            .features
            .iter()
            .map(|f| format!("F {} {} {:?}", f.style_id, f.min_lod, f.geom.bounds()))
            .collect();
        out.extend(ing.source.coastlines.iter().map(|c| format!("C {c:?}")));
        out.extend(
            ing.source.pois.iter().map(|p| format!("P {} {} {} {:?}", p.subtype, p.lon_udeg, p.lat_udeg, p.name)),
        );
        out.push(format!("nav {} nodes, {} edges", ing.nav_graph.nodes.len(), ing.nav_graph.edges.len()));
        out
    }

    /// The `tiny.osm` truth table: relations assembled (R1's lake with a hole, R2's two forest
    /// outers) plus lines and closed-way polygons, giving 10 features.
    #[test]
    fn tiny_truth_table() {
        // The fixture is committed in-repo; a missing one is a hard failure, not a skip.
        assert!(
            std::path::Path::new(TINY_PBF).exists(),
            "corpus fixture missing: {TINY_PBF}. It is committed; rebuild from tiny/tiny.osm via \
             builder/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let ing = ingest_osm(&sources(&[TINY_PBF]), &cfg, None, &quiet()).expect("ingest");

        // W8 (way 109) is the only coastline; nodes 29,30 ⇒ 2 points.
        assert_eq!(ing.source.coastlines.len(), 1, "exactly one coastline");
        assert_eq!(ing.source.coastlines[0].len(), 2);

        // Multiset of (style_id, is_polygon).
        let mut counts: HashMap<(u8, bool), usize> = HashMap::new();
        for f in &ing.source.features {
            *counts.entry((f.style_id, is_polygon(&f.geom))).or_insert(0) += 1;
        }
        let n = |id: u8, poly: bool| counts.get(&(id, poly)).copied().unwrap_or(0);

        assert_eq!(n(50, true), 3, "W5 closed forest + R2's two outer rings ⇒ 3 polygons");
        assert_eq!(n(36, true), 1, "R1 natural=water ⇒ 1 polygon (lake)");
        assert_eq!(n(15, true), 1, "W11 highway=pedestrian area=yes ⇒ 1 polygon");
        assert_eq!(n(12, false), 1, "W6 closed highway=residential ⇒ 1 line");
        assert_eq!(n(5, false), 1, "W7 highway=primary ⇒ 1 line");
        assert_eq!(n(3, false), 1, "W7b highway=trunk ⇒ 1 line");
        assert_eq!(n(63, false), 1, "W9 admin_level=2 ⇒ 1 line");
        assert_eq!(n(36, false), 1, "W12 natural=water area=no ⇒ 1 line");

        // R1 is a lake WITH an island (one hole).
        let lake = ing.source.features.iter().find(|f| f.style_id == 36 && is_polygon(&f.geom)).expect("water polygon");
        match &lake.geom {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "R1 has one hole"),
            _ => unreachable!(),
        }

        assert_eq!(n(12, true), 0, "no residential blob (closed-line-way fix)");
        // 5 polygons (3 forest, 1 pedestrian, 1 water lake) + 5 lines.
        assert_eq!(ing.source.features.len(), 10, "10 features total");
    }

    /// End-to-end POI extraction over the hand-authored `poi.osm` fixture, whose header comment is
    /// the truth table: node and closed-way classification, name folding, and both dedup pairs.
    #[test]
    fn poi_fixture_end_to_end() {
        const POI_PBF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/poi.osm.pbf");
        assert!(
            std::path::Path::new(POI_PBF).exists(),
            "corpus fixture missing: {POI_PBF}. It is committed; rebuild from poi/poi.osm via \
             builder/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let ing = ingest_osm(&sources(&[POI_PBF]), &cfg, None, &quiet()).expect("ingest");
        assert_metadata(&sources(&[POI_PBF]), &cfg, &ing);

        // 16 candidates (13 nodes + 3 way-centroids), 2 dedup-dropped ⇒ 14 kept.
        assert_eq!(ing.source.pois.len(), 16, "distinct OSM identities survive");

        let find = |name: Option<&str>, subtype: u8| {
            ing.source
                .pois
                .iter()
                .find(|p| p.subtype == subtype && p.name.as_deref() == name)
                .unwrap_or_else(|| panic!("missing poi subtype {subtype} name {name:?}: {:?}", ing.source.pois))
        };

        // N1: named water node, exact µdeg grid.
        let n1 = find(Some("Marktbrunnen"), 1);
        assert_eq!((n1.lat_udeg, n1.lon_udeg, n1.from_node), (47_995_000, 7_850_000, true));
        // The node and building retain separate source identities.
        let n2 = find(Some("Edeka Mueller"), 13);
        assert_eq!((n2.lat_udeg, n2.lon_udeg, n2.from_node), (47_989_900, 7_859_900, true));
        // N3: CJK name folded to empty ⇒ unnamed.
        let n3 = find(None, 1);
        assert_eq!((n3.lat_udeg, n3.lon_udeg), (47_980_000, 7_840_000));
        // Nearby service identities stay distinct.
        find(Some("Brunnen A"), 1);
        assert!(ing.source.pois.iter().any(|p| p.subtype == 2), "the separate spring remains available");
        // W2: unnamed campsite way ⇒ POI at the ring centroid.
        let w2 = find(None, 5);
        assert_eq!((w2.lat_udeg, w2.lon_udeg, w2.from_node), (48_000_200, 7_870_200, false));
        // N4 (amenity=parking) never classified.
        assert_eq!(obc_places::metadata::format_counts(&ing.source.pois, 0).matches("water 4").count(), 1);

        // The settlement rows: one of each class, the fall-backs, and the area centroid.
        let city = find(Some("Testville"), 21);
        assert_eq!(city.population, Some(250_000));
        assert_eq!(find(Some("Kleinstadt"), 22).population, None);
        find(Some("Grüßau"), 23);
        find(Some("A very long settlement n"), 24);
        find(Some("Tokyo"), 23);
        find(Some("Baeckerdorf"), 15);
        assert_eq!(find(Some("Freiburg"), 21).population, Some(220_286), "a shorter short_name is stored");
        let ring = find(Some("Ringdorf"), 23);
        assert_eq!((ring.lat_udeg, ring.lon_udeg, ring.from_node), (47_950_200, 7_900_200, false));
        find(Some("Mirnyy"), 23);
        assert_eq!(obc_places::metadata::format_counts(&ing.source.pois, 0).matches("settlement 8").count(), 1);
    }

    #[test]
    fn bbox_parse_is_strict_about_the_box() {
        let ok = Bbox::parse("7.39,43.71,7.47,43.77").expect("valid box");
        assert_eq!(ok.to_degrees(), (7.39, 43.71, 7.47, 43.77), "degrees survive the decimicro round trip");
        assert_eq!(Bbox::parse(" 7.39 , 43.71 , 7.47 , 43.77 ").expect("whitespace"), ok, "fields are trimmed");
        assert_eq!(
            Bbox::parse("-8.0000011,-1.0000001,8.000001,1.0000001").unwrap().microdegree_bounds(),
            (-8_000_001, -1_000_000, 8_000_001, 1_000_000)
        );
        // The edges land on osmium's grid: round-half-away-from-zero at 1e-7.
        assert_eq!(Bbox::parse("7.39,0,8,1").unwrap().microdegree_bounds().0, 7_390_000);
        assert_eq!(Bbox::parse("-7.39,0,8,1").unwrap().microdegree_bounds().0, -7_390_000);

        for bad in [
            "7.39,43.71,7.47",         // three fields
            "7.39,43.71,7.47,43.77,1", // five
            "west,43.71,7.47,43.77",   // not a number
            "nan,43.71,7.47,43.77",    // not finite
            "-181,43.71,7.47,43.77",   // lon out of range
            "7.39,-91,7.47,43.77",     // lat out of range
            "7.47,43.71,7.39,43.77",   // east of west (the antimeridian wrap)
            "7.39,43.71,7.39,43.77",   // zero width
            "7.39,43.77,7.47,43.71",   // north below south
        ] {
            assert!(Bbox::parse(bad).is_err(), "{bad:?} must be rejected");
        }
        // A wrapping box names the reason, not just "invalid".
        let msg = Bbox::parse("179,-1,-179,1").unwrap_err();
        assert!(msg.contains("antimeridian"), "wrap error should explain itself: {msg}");
    }

    #[test]
    fn box_area_shrinks_with_latitude() {
        let one_degree_at_equator = obc_pbf::bbox::box_area_km2((0.0, 0.0, 1.0, 1.0));
        assert!((one_degree_at_equator - 12_363.0).abs() < 10.0, "{one_degree_at_equator} km²");
        let one_degree_at_sixty = obc_pbf::bbox::box_area_km2((0.0, 59.5, 1.0, 60.5));
        assert!((one_degree_at_sixty - 6_182.0).abs() < 10.0, "{one_degree_at_sixty} km²");

        // The Grimsel fixture box: a region a look is meant to reach.
        let grimsel = obc_pbf::bbox::box_area_km2((8.15034, 46.48261, 8.46007, 46.72070));
        assert!((600.0..700.0).contains(&grimsel), "{grimsel} km²");
    }

    #[test]
    fn a_source_without_a_declared_box_reads_as_unknown() {
        assert_eq!(obc_pbf::bbox::declared_bbox(TINY_PBF), Ok(None));
    }

    /// The relation-complete crop, over the `tiny.osm` truth table. The box covers R1 whole, takes
    /// only one of R2's two outer rings, and clips the middle of both open highways. Ways stay
    /// whole: W7b reaches far outside the box because one of its nodes is inside. Relations stay
    /// whole: R2's in-box W3 pulls in its outside W4 member, so both forest outers assemble.
    #[test]
    fn bbox_crop_keeps_ways_whole_and_completes_area_relations() {
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        // lon 7.798..7.809, lat 47.979..47.995 — see tiny.osm's node grid.
        let bbox = Bbox::parse("7.798,47.979,7.809,47.995").expect("box");
        let ing = ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");

        let mut counts: HashMap<(u8, bool), usize> = HashMap::new();
        for f in &ing.source.features {
            *counts.entry((f.style_id, is_polygon(&f.geom))).or_insert(0) += 1;
        }
        let n = |id: u8, poly: bool| counts.get(&(id, poly)).copied().unwrap_or(0);

        // R1 (both member ways inside) still assembles, hole and all.
        assert_eq!(n(36, true), 1, "R1 lake survives whole");
        let lake = ing.source.features.iter().find(|f| f.style_id == 36 && is_polygon(&f.geom)).expect("water polygon");
        match &lake.geom {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "island hole kept"),
            _ => unreachable!(),
        }
        // R2 touches the box through W3, so W4 is pulled in and both disjoint
        // outer rings survive. W5 is an unrelated closed forest outside the box.
        assert_eq!(n(50, true), 2, "R2's complete two-outer forest survives");
        // Out of the box entirely: W5/W6/W11 (lat ≥ 47.996), W9 (48.000), W8 coast.
        assert_eq!(n(15, true), 0, "W11 pedestrian area is north of the box");
        assert_eq!(n(12, false), 0, "W6 residential loop is north of the box");
        assert_eq!(n(63, false), 0, "W9 admin line is north of the box");
        assert!(ing.source.coastlines.is_empty(), "W8 coastline sits east of the box");
        // Kept: W7 primary, W7b trunk, W12 water line, R1's polygon, and both
        // of R2's forest polygons.
        assert_eq!(n(5, false), 1, "W7 primary crosses the east edge and is kept");
        assert_eq!(n(3, false), 1, "W7b trunk crosses the east edge and is kept");
        assert_eq!(n(36, false), 1, "W12 water line is inside");
        assert_eq!(ing.source.features.len(), 6, "1 lake + 2 forest polygons + 3 lines");

        // The headline: the trunk is not trimmed at the box edge (lon 7.809) — it keeps its far
        // node at 7.855, exactly as `osmium extract` would emit it.
        let trunk = ing.source.features.iter().find(|f| f.style_id == 3).expect("trunk line");
        let (_, _, maxx, _) = trunk.geom.bounds();
        assert!((maxx - 7.855).abs() < 1e-9, "trunk must reach its real end at 7.855, got {maxx}");

        let outside_forest = ing
            .source
            .features
            .iter()
            .filter(|f| f.style_id == 50 && is_polygon(&f.geom))
            .map(|f| f.geom.bounds().2)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            outside_forest > 7.811,
            "R2's member outside the 7.809 crop edge must be present, got max lon {outside_forest}"
        );
    }

    /// A box that swallows the whole file must change nothing — the crop path is
    #[test]
    fn bbox_covering_everything_is_a_no_op() {
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let plain = ingest_osm(&sources(&[TINY_PBF]), &cfg, None, &quiet()).expect("ingest");
        let boxed =
            ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(Bbox::parse("-180,-90,180,90").expect("world")), &quiet())
                .expect("ingest");
        assert_eq!(plain.source.features.len(), boxed.source.features.len());
        assert_eq!(plain.source.coastlines, boxed.source.coastlines);
        assert_eq!(plain.source.pois.len(), boxed.source.pois.len());
        for (a, b) in plain.source.features.iter().zip(&boxed.source.features) {
            assert_eq!((a.style_id, a.min_lod, a.geom.bounds()), (b.style_id, b.min_lod, b.geom.bounds()));
        }
    }

    #[test]
    fn bbox_missing_the_data_is_an_error() {
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let Err(err) =
            ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(Bbox::parse("10,10,11,11").expect("box")), &quiet())
        else {
            panic!("a box off in the Mediterranean must not ingest");
        };
        assert!(err.contains("does not overlap"), "unexpected message: {err}");
    }

    /// Pass 0 is the one place that needs the PBF type-sorted, and a file that is not would
    /// otherwise select nothing at all and pack a silently empty map. The committed
    /// `unsorted.osm.pbf` writes its way before its nodes.
    #[test]
    fn bbox_refuses_an_unsorted_pbf() {
        const UNSORTED_PBF: &str =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/unsorted.osm.pbf");
        assert!(
            std::path::Path::new(UNSORTED_PBF).exists(),
            "corpus fixture missing: {UNSORTED_PBF}. It is committed; rebuild from unsorted/unsorted.osm via \
             builder/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        // The box covers both nodes, so a sorted file would have kept the way.
        let bbox = Bbox::parse("7.79,47.98,7.81,48.0").expect("box");
        let Err(err) = ingest_osm(&sources(&[UNSORTED_PBF]), &cfg, Some(bbox), &quiet()) else {
            panic!("an unsorted .pbf must not be cropped silently");
        };
        assert!(err.contains("not sorted"), "unexpected message: {err}");
        // Without a box the ingest is order-agnostic, so the same file still packs.
        let ing =
            ingest_osm(&sources(&[UNSORTED_PBF]), &cfg, None, &quiet()).expect("uncropped ingest is order-agnostic");
        assert_eq!(ing.source.features.len(), 1, "the primary way survives without a box");
    }

    /// The same file listed twice is the sharpest duplicate case there is: every single object is a
    /// duplicate, so a right merge gives exactly the one-source ingest and a wrong one doubles it.
    #[test]
    fn merging_a_source_with_itself_changes_nothing() {
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let once = ingest_osm(&sources(&[TINY_PBF]), &cfg, None, &quiet()).expect("ingest");
        let twice = ingest_osm(&sources(&[TINY_PBF, TINY_PBF]), &cfg, None, &quiet()).expect("ingest");
        assert_eq!(shape(&once), shape(&twice), "a source merged with itself must be that source");

        // And the same with a box, which adds pass 0's id sets to the mix.
        let bbox = Bbox::parse("7.798,47.979,7.809,47.995").expect("box");
        let once = ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");
        let twice = ingest_osm(&sources(&[TINY_PBF, TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");
        assert_eq!(shape(&once), shape(&twice), "cropped, too");
    }

    /// Two halves of `tiny.osm` that overlap in the middle must ingest to exactly what the whole
    /// file does. The split is awkward on purpose: `tiny_west` holds R1 and the long ways,
    /// `tiny_east` holds R2 and repeats three shared objects, so the merge has to interleave two id
    /// runs and drop duplicates, not just concatenate.
    #[test]
    fn merging_two_overlapping_halves_rebuilds_the_whole() {
        const WEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny_west.osm.pbf");
        const EAST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny_east.osm.pbf");
        for f in [WEST, EAST] {
            assert!(
                std::path::Path::new(f).exists(),
                "corpus fixture missing: {f}. It is committed; rebuild via builder/tests/corpus/build_corpus.sh"
            );
        }
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let whole = ingest_osm(&sources(&[TINY_PBF]), &cfg, None, &quiet()).expect("ingest");
        let halves = ingest_osm(&sources(&[WEST, EAST]), &cfg, None, &quiet()).expect("ingest");
        assert_metadata(&sources(&[WEST, EAST]), &cfg, &halves);
        assert_eq!(shape(&whole), shape(&halves), "west + east must rebuild tiny.osm exactly");

        // Cropped: pass 0's node phase has to finish across BOTH files before either one's ways
        // can be judged. W7/W7b start west and run east, so a per-file selection would differ.
        let bbox = Bbox::parse("7.798,47.979,7.809,47.995").expect("box");
        let whole = ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");
        let halves = ingest_osm(&sources(&[WEST, EAST]), &cfg, Some(bbox), &quiet()).expect("ingest");
        assert_eq!(shape(&whole), shape(&halves), "west + east must rebuild the cropped tiny.osm exactly");
    }

    /// The tie-break: the first source that carries an id wins the whole object. `tiny_east`
    /// re-states way 107 with a different style, so listing it first changes the style and listing
    /// it second changes nothing.
    #[test]
    fn the_first_source_carrying_an_id_wins_it() {
        const WEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny_west.osm.pbf");
        const EAST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny_east.osm.pbf");
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let style_of_107 = |paths: &[&str]| {
            let ing = ingest_osm(&sources(paths), &cfg, None, &quiet()).expect("ingest");
            assert_metadata(&sources(paths), &cfg, &ing);
            // Way 107 is the only feature spanning lon 7.800..7.812 at lat 47.988.
            ing.source
                .features
                .iter()
                .find(|f| {
                    let (minx, miny, maxx, _) = f.geom.bounds();
                    (miny - 47.988).abs() < 1e-9 && (minx - 7.800).abs() < 1e-9 && (maxx - 7.812).abs() < 1e-9
                })
                .map(|f| f.style_id)
        };
        let west_first = style_of_107(&[WEST, EAST]).expect("way 107 kept");
        let east_first = style_of_107(&[EAST, WEST]).expect("way 107 kept");
        assert_ne!(west_first, east_first, "the two copies of way 107 must be distinguishable");
        assert_eq!(west_first, style_of_107(&[WEST]).expect("way 107"), "west first ⇒ west's copy");
        assert_eq!(east_first, style_of_107(&[EAST]).expect("way 107"), "east first ⇒ east's copy");

        // And on a node, where the loser is the copy carrying the tags: east's node 25 is a
        // drinking-water POI and west's is bare, so with west first that POI does not exist.
        let pois =
            |paths: &[&str]| ingest_osm(&sources(paths), &cfg, None, &quiet()).expect("ingest").source.pois.len();
        assert_eq!(pois(&[EAST]), pois(&[WEST]) + 1, "only east's node 25 is a POI");
        assert_eq!(pois(&[WEST, EAST]), pois(&[WEST]), "west first ⇒ east's tagged copy contributes nothing");
        assert_eq!(pois(&[EAST, WEST]), pois(&[EAST]), "east first ⇒ its POI survives");
    }

    #[test]
    fn keyed_retains_in_order_and_sorts_by_id() {
        let mut k = Keyed::new(true);
        for (id, name) in [(9_i64, "a"), (3, "b"), (11, "c"), (3, "dup"), (1, "d")] {
            k.push(id, name);
        }
        k.retain_keys(|id| id != 3);
        assert_eq!(k.items(), ["a", "c", "d"], "retain preserves the surviving order");
        assert_eq!(k.keys(), [9, 11, 1]);
        k.sort();
        assert_eq!(k.items(), ["d", "a", "c"], "sort puts them in ascending id order");
        assert_eq!(k.keys(), [1, 9, 11]);

        // Untagged (single source) is a plain Vec — nothing is recorded to sort by.
        let mut plain = Keyed::new(false);
        plain.push(9, "a");
        plain.push(3, "b");
        assert!(plain.keys().is_empty(), "a single source records no tags");
        assert_eq!(plain.into_items(), ["a", "b"], "and keeps file order");
    }

    /// `freeze` must be safe to call twice, because pass 0 freezes the node set early.
    #[test]
    fn id_set_freezes_and_dedupes() {
        let mut s = IdSet::default();
        s.absorb(vec![9_i64, 3]);
        s.absorb(vec![9, -1, 3]);
        s.freeze();
        s.freeze();
        assert_eq!(s.len(), 3, "duplicates collapse");
        for id in [-1, 3, 9] {
            assert!(s.contains(id));
        }
        for id in [0, 4, 10] {
            assert!(!s.contains(id));
        }
    }
}
