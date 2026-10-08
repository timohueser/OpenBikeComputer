//! Read one or more `.osm.pbf` files into styled features: lines, closed-way polygons, and
//! multipolygon or `boundary` relation areas.
//!
//! Pass 1 builds the `node_id -> coord` store, matches tagged nodes against the POI table, and
//! collects qualifying area relations — relations sit last in a sorted PBF, so one read sees them
//! after the nodes. Pass 2 resolves ways into features and coastlines and captures the geometry of
//! any way a relation needs; [`assemble_multipolygon`] then builds the relation areas. Assembly is
//! additive: a tagged closed way that is also a relation member yields its own polygon and
//! contributes to the relation. A closed `highway=residential` loop is a line only, never a blob.
//!
//! Coordinates use `decimicro / 1e7`, never `* 1e-7`, so the f64 lon/lat match osmium's exactly.
//!
//! Given more than one source, every pass reads every source and the results are folded together.
//! On a duplicate the first source on the command line wins, decided on the `(type, id)` alone, so
//! the winner is a whole object and never a mix of two. The fold then restores ascending id order
//! per type, which is what a merged sorted file would have handed to the same pass: feature order
//! decides which quadtree chunk a feature lands in, and therefore the packed bytes. A single source
//! is left strictly alone, untagged and unsorted. Sources are read in parallel ([`par_sources`])
//! and the fold is sequential in command-line order, so the result never depends on which thread
//! finished first.
//!
//! A `--bbox` adds a pass 0 ([`select_crop`]) that reproduces `osmium extract --bbox` in-process,
//! as a renderer-aware variant of osmium's `smart` strategy. A way touching the box is kept whole,
//! with the outside nodes it needs, because [`resolve_coords`] drops a way with a missing node
//! rather than trimming it at the border, and because a nav edge must end where the way ends. The
//! member ways of a kept area relation are completed too, or a multipolygon would be dropped whole
//! as soon as one ring segment lay outside the box. Only area relations the active config can
//! render are completed, so a small crop never pulls in a continent-sized route relation.

use std::collections::{HashMap, HashSet};

use osmpbf::{ByteOffset, Element, RelMemberType};

use crate::geom::{assemble_multipolygon, Geom};
use obc_map_core::config::Config;
use obc_map_core::progress::{Phase, Progress};
use obc_pbf::area::polygon_is_valid;
use obc_pbf::bbox::Bbox;
use obc_pbf::scan::{par_sources, scan_blobs, Keyed, Scan};
use obc_pbf::selection::{Crop, IdSet};
use obc_places::metadata::{self as poi, Poi};
use obc_places::osm::{push_node_poi, push_routable_way, resolve_coords};
use obc_places::routing::RoutableWay;

pub struct IngestFeature {
    pub style_id: u8,
    pub min_lod: usize,
    pub geom: Geom,
}

/// Coastlines are captured separately and always: they feed the bbox and the land/sea split.
pub struct Ingested {
    pub features: Vec<IngestFeature>,
    pub coastlines: Vec<Vec<(f64, f64)>>,
    pub pois: Vec<Poi>,
    pub landmark_links: Vec<poi::LandmarkLink>,
}

/// A pass-1 area relation awaiting member geometry (pass 2) and assembly.
struct PendingRelation {
    style: Option<(u8, usize)>,
    poi: Option<Poi>,
    /// Member way ids in member order. Roles are dropped — `build_area` classifies outer and inner
    /// by geometry.
    member_ways: Vec<i64>,
}

/// The tags whose presence (with `area != no`) classifies a closed way as a polygon.
const AREA_TAGS: [&str; 6] = ["building", "landuse", "amenity", "leisure", "natural", "waterway"];

/// Pass 0 — select the crop across every source: nodes inside `bbox`, ways touching one of them,
/// complete members of renderable area relations reached by those ways, and the outside nodes all
/// selected ways still need. Also returns, per source, the offset of the first blob that holds a
/// way, so pass 2 resumes there instead of decoding the node section again.
///
/// The node phase has to finish across all sources before any file's ways can be judged: a way in
/// one file can have its only in-box node in another. The split costs nothing, because it falls
/// where the file is already split — phase A walks the node section and stops at the first
/// way-bearing blob, phase B resumes exactly there, phase C completes the area relations.
///
/// This is the one pass that needs the source type-sorted, because phase A stops at the first way.
/// An element out of that order is reported as an error rather than silently skipped.
fn select_crop(
    paths: &[String],
    bbox: Bbox,
    config: &Config,
    progress: &Progress,
) -> Result<(Crop, Vec<Option<ByteOffset>>), String> {
    progress.stage(Phase::Ingest, "Pass 0: selecting bbox...");
    // Phase A: the in-box node ids, from every source.
    let scans = par_sources(paths, |_, path| {
        let mut ids: Vec<i64> = Vec::new();
        let mut saw_relation = false;
        let mut out_of_order = false;
        let mut in_box = |lon: i32, lat: i32, id: i64| {
            if bbox.contains(lon, lat) {
                ids.push(id);
            }
        };
        let ways_at = scan_blobs(path, None, progress, |el| match el {
            Element::Node(n) => {
                in_box(n.decimicro_lon(), n.decimicro_lat(), n.id());
                Scan::Continue
            }
            Element::DenseNode(n) => {
                in_box(n.decimicro_lon(), n.decimicro_lat(), n.id());
                Scan::Continue
            }
            // The first way: the node section is behind us and phase B takes over from this blob.
            // A file with no ways at all just runs to the end.
            Element::Way(_) => {
                out_of_order |= saw_relation;
                Scan::StopAtThisBlob
            }
            Element::Relation(_) => {
                saw_relation = true;
                Scan::Continue
            }
        })?;
        if out_of_order {
            return Err(format!(
                "{path} is not sorted (a way follows a relation), so --bbox cannot select its areas — sort it \
                 first (e.g. `osmium sort`)"
            ));
        }
        Ok((ids, ways_at))
    })?;
    let mut inside = IdSet::default();
    let mut ways_at = Vec::with_capacity(paths.len());
    for (ids, at) in scans {
        inside.absorb(ids);
        ways_at.push(at);
    }
    inside.freeze();

    // Phase B: ways touching the box and their halo. Stop at the first relation-bearing blob, so
    // relation member lists never accumulate for the whole source.
    let scans = par_sources(paths, |i, path| {
        let (mut ways, mut halo) = (Vec::new(), Vec::new());
        let (mut saw_way, mut out_of_order) = (false, false);
        let relations_at = scan_blobs(path, ways_at[i], progress, |el| {
            match el {
                Element::Way(w) => {
                    saw_way = true;
                    if w.refs().any(|r| inside.contains(r)) {
                        ways.push(w.id());
                        // The halo: every other node this way needs. Ids already `inside` are
                        // skipped — `keeps_node` checks both sets, and a dense urban box would
                        // otherwise store most of its nodes twice.
                        for r in w.refs() {
                            if !inside.contains(r) {
                                halo.push(r);
                            }
                        }
                    }
                }
                // Nodes before the first way of the resume blob are ones phase A already saw;
                // anything after a way means the file is not sorted.
                Element::Node(_) | Element::DenseNode(_) => out_of_order |= saw_way,
                Element::Relation(_) => return Scan::StopAtThisBlob,
            }
            Scan::Continue
        })?;
        if out_of_order {
            return Err(format!(
                "{path} is not sorted (a node follows a way), so --bbox cannot select its ways — sort it first \
                 (e.g. `osmium sort`)"
            ));
        }
        Ok((ways, halo, relations_at))
    })?;
    let (mut ways, mut halo) = (IdSet::default(), IdSet::default());
    let mut relations_at = Vec::with_capacity(scans.len());
    for (w, h, at) in scans {
        ways.absorb(w);
        halo.absorb(h);
        relations_at.push(at);
    }
    ways.freeze();

    // Phase C: complete every renderable area relation reached by a touching way. Relations are
    // streamed in source order to match the ingest merge rule: on a duplicate relation id the first
    // renderable copy wins.
    let touching_ways = ways.len();
    let mut seen_relations = HashSet::new();
    let mut relation_ways = IdSet::default();
    let mut relation_ids = IdSet::default();
    for (i, path) in paths.iter().enumerate() {
        let Some(at) = relations_at[i] else { continue };
        let mut saw_relation = false;
        let mut out_of_order = false;
        scan_blobs(path, Some(at), progress, |el| {
            match el {
                Element::Relation(r) => {
                    saw_relation = true;
                    if let Some(relation) = pending_relation(&r, config) {
                        if (paths.len() == 1 || seen_relations.insert(r.id()))
                            && relation.member_ways.iter().any(|&id| ways.contains(id))
                        {
                            relation_ids.push(r.id());
                            relation_ways.absorb(relation.member_ways);
                        }
                    }
                }
                // The resume blob can hold its final ways before its first relation. An object of
                // an earlier type after that relation is genuinely out of order.
                Element::Node(_) | Element::DenseNode(_) | Element::Way(_) => out_of_order |= saw_relation,
            }
            Scan::Continue
        })?;
        if out_of_order {
            return Err(format!(
                "{path} is not sorted (a node or way follows a relation), so --bbox cannot complete its areas — \
                 sort it first (e.g. `osmium sort`)"
            ));
        }
    }
    relation_ids.freeze();
    relation_ways.freeze();

    // Relation completion can introduce ways that never touch an in-box node. Read only the way
    // section again to collect every node those ways require.
    if !relation_ways.is_empty() {
        let scans = par_sources(paths, |i, path| {
            let mut relation_halo = Vec::new();
            scan_blobs(path, ways_at[i], progress, |el| {
                if let Element::Way(w) = el {
                    if relation_ways.contains(w.id()) {
                        for id in w.refs() {
                            if !inside.contains(id) {
                                relation_halo.push(id);
                            }
                        }
                    }
                }
                Scan::Continue
            })?;
            Ok(relation_halo)
        })?;
        for ids in scans {
            halo.absorb(ids);
        }
    }

    ways.absorb(relation_ways.into_ids());
    ways.freeze();
    halo.freeze();
    progress.log(format!(
        "  {} node(s) in box, {} touching way(s) + {} relation member(s) from {} area relation(s) kept (+{} boundary node(s))",
        inside.len(),
        touching_ways,
        ways.len().saturating_sub(touching_ways),
        relation_ids.len(),
        halo.len()
    ));
    Ok((Crop::new(inside, halo, ways, relation_ids), ways_at))
}

/// One source's pass-1 harvest.
struct NodeScan {
    links: Keyed<poi::LandmarkLink>,
    nodes: HashMap<i64, (i32, i32)>,
    pois: Keyed<Poi>,
    rels: Keyed<PendingRelation>,
}

/// One source's pass-2 harvest, plus the way ids it claims.
struct WayScan {
    features: Keyed<IngestFeature>,
    coastlines: Keyed<Vec<(f64, f64)>>,
    pois: Keyed<Poi>,
    routable: Keyed<RoutableWay>,
    member_geom: HashMap<i64, Vec<(f64, f64)>>,
    /// Every way id this source processed, including the ones that produced nothing at all.
    /// Ownership is decided on the id alone, so an untagged or unresolvable copy here still has to
    /// shadow a later source's copy. Left empty for a single source, which claims everything.
    claimed: Vec<i64>,
}

/// Read drawing features and source metadata without constructing a graph.
/// Routable ways retain their source topology for the separate network producer.
pub fn ingest_osm_ways(
    paths: &[String],
    config: &Config,
    bbox: Option<Bbox>,
    progress: &Progress,
) -> Result<(Ingested, Vec<RoutableWay>), String> {
    ingest_inner(paths, config, bbox, progress)
}

fn ingest_inner(
    paths: &[String],
    config: &Config,
    bbox: Option<Bbox>,
    progress: &Progress,
) -> Result<(Ingested, Vec<RoutableWay>), String> {
    if paths.is_empty() {
        return Err("no .osm.pbf input given".into());
    }
    // More than one source means every output is tagged with the id that produced it, which is
    // what the fold needs to drop duplicates and restore a single merged file's order.
    let merging = paths.len() > 1;
    if merging {
        progress.stage(
            Phase::Merging,
            format!("Merging {} sources (on a duplicate id, the first source wins)...", paths.len()),
        );
    }

    // Pass 0 (only with --bbox): the id selection, plus the per-source offset where the ways begin.
    let (crop, ways_at) = match bbox {
        Some(bb) => {
            let (crop, ways_at) = select_crop(paths, bb, config, progress)?;
            if crop.is_empty() {
                let (w, s, e, n) = bb.to_degrees();
                return Err(format!("--bbox {w},{s},{e},{n} does not overlap any data in {}", paths.join(", ")));
            }
            (Some(crop), ways_at)
        }
        None => (None, vec![None; paths.len()]),
    };

    // Pass 1: node-location store and relation collection, per source. The stage strings reach the
    // build UI, so each is reported when its pass actually starts, not both up front.
    progress.stage(Phase::Ingest, "Pass 1: reading nodes...");
    let scans = par_sources(paths, |_, path| read_nodes(path, config, crop.as_ref(), merging, progress))?;
    let NodeScan { nodes, pois: node_pois, rels, links } = fold_node_scans(scans, merging);
    let pending = rels.into_items();
    let needed_ways: HashSet<i64> = pending.iter().flat_map(|r| r.member_ways.iter().copied()).collect();

    // Pass 2: ways into features and coastlines, plus member-way geometry capture.
    progress.stage(Phase::Ingest, "Pass 2: processing ways...");
    let scans = par_sources(paths, |i, path| {
        read_ways(path, ways_at[i], config, crop.as_ref(), &nodes, &needed_ways, merging, progress)
    })?;
    let WayScan { features, coastlines, pois: way_pois, routable, member_geom, .. } = fold_way_scans(scans, merging);
    let mut features = features.into_items();
    let coastlines = coastlines.into_items();
    let routable_ways = routable.into_items();
    // POI candidates from both passes, deduped after assembly — node candidates first, then way
    // centroids, the order a single sorted file produces them in.
    let mut poi_cands = node_pois.into_items();
    poi_cands.extend(way_pois.into_items());

    // Assemble relation areas from the captured member geometry: each outer ring plus its nested
    // holes becomes one polygon, styled by the relation. Like osmium, assemble only when ALL member
    // ways are present; an incomplete relation is dropped rather than assembled from survivors,
    // which would emit a phantom boundary-crossing polygon.
    for pr in &pending {
        let mut members = Vec::with_capacity(pr.member_ways.len());
        let mut complete = true;
        for wid in &pr.member_ways {
            match member_geom.get(wid) {
                Some(g) => members.push(g.clone()),
                None => {
                    complete = false;
                    break;
                }
            }
        }
        if !complete {
            continue;
        }
        let polygons = assemble_multipolygon(&members);
        if let Some(mut poi) = pr.poi.clone() {
            if let Some((x, y)) = obc_places::area_center(polygons.iter().filter_map(|p| match p {
                Geom::Polygon { exterior, .. } => Some(exterior.as_slice()),
                _ => None,
            })) {
                poi.lon_udeg = obc_places::to_udeg(x);
                poi.lat_udeg = obc_places::to_udeg(y);
                poi_cands.push(poi);
            }
        }
        if let Some((style_id, min_lod)) = pr.style {
            for poly in polygons {
                features.push(IngestFeature { style_id, min_lod, geom: poly });
            }
        }
    }

    // Deduplicate source identities before resolving approaches.
    let (mut pois, poi_dropped) = poi::dedupe(poi_cands);
    poi::resolve_approaches(&mut pois, &routable_ways, &config.routing.profiles);
    let mut landmark_links: Vec<_> =
        pois.iter().filter(|p| p.wikidata.is_some() || p.wikipedia.is_some()).map(poi::LandmarkLink::from).collect();
    landmark_links.extend(links.into_items());
    pois.retain(|p| p.subtype != 0);
    progress.log(poi::format_counts(&pois, poi_dropped));

    progress.log(format!("routable ways: {} (graphs are built separately)", routable_ways.len()));
    Ok((Ingested { features, coastlines, pois, landmark_links }, routable_ways))
}

/// Pass 1, one source: node-location store, node POIs and area relations.
///
/// Cropped, this keeps only the nodes the extract would contain, halo included, so a tagged node
/// just outside the box that a kept way needs becomes a POI here exactly as it would in an `osmium
/// extract` output. Cropped runs collect only the relations pass 0 selected; the
/// all-members-present rule below still rejects relations already incomplete at the source
/// extract's own edge.
fn read_nodes(
    path: &str,
    config: &Config,
    crop: Option<&Crop>,
    tagged: bool,
    progress: &Progress,
) -> Result<NodeScan, String> {
    let mut nodes: HashMap<i64, (i32, i32)> = HashMap::new();
    let mut pois = Keyed::new(tagged);
    let mut rels = Keyed::new(tagged);
    let mut links = Keyed::new(tagged);
    let keeps_node = |id: i64| crop.is_none_or(|c| c.keeps_node(id));
    let keeps_relation = |id: i64| crop.is_none_or(|c| c.keeps_relation(id));
    scan_blobs(path, None, progress, |el| {
        match el {
            Element::Node(n) if keeps_node(n.id()) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                push_node_poi(n.id(), n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut pois);
            }
            Element::DenseNode(n) if keeps_node(n.id()) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                push_node_poi(n.id(), n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut pois);
            }
            Element::Relation(r) => {
                if keeps_relation(r.id()) {
                    collect_relation(&r, config, &mut rels);
                }
                let tags: HashMap<_, _> = r.tags().collect();
                if tags.contains_key("wikidata") || tags.contains_key("wikipedia") {
                    links.push(r.id(), obc_places::osm::relation_link(r.id(), &tags));
                }
            }
            _ => {}
        }
        Scan::Continue
    })
    .map_err(|e| format!("pass 1: {e}"))?;
    Ok(NodeScan { nodes, pois, rels, links })
}

/// Combine the sources' pass-1 harvests, in command-line order.
fn fold_node_scans(scans: Vec<NodeScan>, merging: bool) -> NodeScan {
    let mut it = scans.into_iter();
    let mut acc = it.next().expect("at least one source");
    let mut seen_rels: HashSet<i64> = acc.rels.keys().iter().copied().collect();
    let mut seen_links: HashSet<i64> = acc.links.keys().iter().copied().collect();
    for mut next in it {
        // Ownership is tested BEFORE this source's nodes land in `acc`, so the question is whether
        // an earlier source already had this node — and the whole object loses, tags and all.
        next.pois.retain_keys(|id| !acc.nodes.contains_key(&id));
        next.rels.retain_keys(|id| seen_rels.insert(id));
        next.links.retain_keys(|id| seen_links.insert(id));
        acc.links.append(next.links);
        acc.pois.append(next.pois);
        acc.rels.append(next.rels);
        for (id, coord) in next.nodes {
            acc.nodes.entry(id).or_insert(coord);
        }
    }
    if merging {
        acc.pois.sort();
        acc.rels.sort();
        acc.links.sort();
    }
    acc
}

/// Pass 2, one source: ways into features, coastlines, POIs and routable topology, plus the geometry
/// of any way a relation needs.
///
/// `ways_at` is where pass 0 found this file's first way; starting there skips re-decoding the node
/// section. Without a `--bbox` there is no offset and the scan starts at the beginning, which is
/// also what keeps an uncropped ingest order-agnostic.
#[allow(clippy::too_many_arguments)]
fn read_ways(
    path: &str,
    ways_at: Option<ByteOffset>,
    config: &Config,
    crop: Option<&Crop>,
    nodes: &HashMap<i64, (i32, i32)>,
    needed_ways: &HashSet<i64>,
    tagged: bool,
    progress: &Progress,
) -> Result<WayScan, String> {
    let mut features = Keyed::new(tagged);
    let mut coastlines = Keyed::new(tagged);
    let mut pois = Keyed::new(tagged);
    // Routable-way topology for the nav graph. The OSM node ids are kept here (the render path
    // drops them) so shared nodes can be recovered as junctions after the pass.
    let mut routable = Keyed::new(tagged);
    let mut member_geom: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
    let mut claimed: Vec<i64> = Vec::new();
    let keeps_way = |id: i64| crop.is_none_or(|c| c.keeps_way(id));
    scan_blobs(path, ways_at, progress, |el| {
        if let Element::Way(w) = el {
            if keeps_way(w.id()) {
                // Claimed on sight, before anything can go wrong with it: a way this source could
                // not resolve still shadows a later copy.
                if tagged {
                    claimed.push(w.id());
                }
                let refs: Vec<i64> = w.refs().collect();
                // A missing node aborts the whole way, as osmium's `InvalidLocationError` would.
                if let Some(coords) = resolve_coords(&refs, nodes) {
                    push_routable_way(w.id(), &w, &refs, &coords, &mut routable);
                    process_way(&w, &refs, &coords, config, &mut features, &mut coastlines, &mut pois);
                    if needed_ways.contains(&w.id()) {
                        member_geom.insert(w.id(), coords);
                    }
                }
            }
        }
        Scan::Continue
    })
    .map_err(|e| format!("pass 2: {e}"))?;
    Ok(WayScan { features, coastlines, pois, routable, member_geom, claimed })
}

/// Combine the sources' pass-2 harvests, in command-line order: a later source contributes only the
/// ways no earlier source claimed, and the survivors go back in way-id order.
fn fold_way_scans(scans: Vec<WayScan>, merging: bool) -> WayScan {
    let mut it = scans.into_iter();
    let mut acc = it.next().expect("at least one source");
    let mut claimed = IdSet::default();
    claimed.absorb(std::mem::take(&mut acc.claimed));
    claimed.freeze();
    for mut next in it {
        let owned = |id: i64| !claimed.contains(id);
        next.features.retain_keys(owned);
        next.coastlines.retain_keys(owned);
        next.pois.retain_keys(owned);
        next.routable.retain_keys(owned);
        acc.features.append(next.features);
        acc.coastlines.append(next.coastlines);
        acc.pois.append(next.pois);
        acc.routable.append(next.routable);
        for (id, geom) in next.member_geom {
            if owned(id) {
                acc.member_geom.insert(id, geom);
            }
        }
        claimed.absorb(next.claimed);
        claimed.freeze();
    }
    if merging {
        acc.features.sort();
        acc.coastlines.sort();
        acc.pois.sort();
        acc.routable.sort();
    }
    acc
}

/// Classify a `type=multipolygon` or `type=boundary` relation (skipping `admin_level`) for area
/// assembly. Shared by crop selection and pass 1, so the crop completes exactly the relations the
/// renderer can consume.
fn pending_relation(r: &osmpbf::Relation, config: &Config) -> Option<PendingRelation> {
    let tags: HashMap<&str, &str> = r.tags().collect();
    match tags.get("type").copied() {
        Some("multipolygon") | Some("boundary") => {}
        _ => return None,
    }
    let style = (!tags.contains_key("admin_level"))
        .then(|| config.get_style(&tags))
        .flatten()
        .map(|style| (style.id, style.min_lod));
    let poi = obc_places::osm::relation_poi(r.id(), &tags);
    if style.is_none() && poi.is_none() {
        return None;
    }
    let member_ways: Vec<i64> =
        r.members().filter(|m| m.member_type == RelMemberType::Way).map(|m| m.member_id).collect();
    if member_ways.is_empty() {
        return None;
    }
    Some(PendingRelation { style, poi, member_ways })
}

/// Collect a renderable area relation for pass-2 assembly. Roles are ignored; non-way members are
/// skipped.
fn collect_relation(r: &osmpbf::Relation, config: &Config, pending: &mut Keyed<PendingRelation>) {
    if let Some(relation) = pending_relation(r, config) {
        pending.push(r.id(), relation);
    }
}

/// One way: capture the coastline always, then style and classify into a single polygon-or-line
/// emission. `refs` and `coords` are pre-resolved.
fn process_way(
    w: &osmpbf::Way,
    refs: &[i64],
    coords: &[(f64, f64)],
    config: &Config,
    features: &mut Keyed<IngestFeature>,
    coastlines: &mut Keyed<Vec<(f64, f64)>>,
    pois: &mut Keyed<Poi>,
) {
    let tags: HashMap<&str, &str> = w.tags().collect();
    let is_closed = refs.len() >= 2 && refs.first() == refs.last();

    // Coastlines are captured ALWAYS — even if the way is also closed and styled — and as lines.
    if tags.get("natural") == Some(&"coastline") && coords.len() >= 2 {
        coastlines.push(w.id(), coords.to_vec());
    }

    obc_places::osm::push_way_poi(w, refs, coords, pois);

    let Some(style) = config.get_style(&tags) else { return };

    // A closed area emits a polygon; a closed road loop emits a line, never both.
    if is_closed && is_area(&tags) {
        // admin_level + area ⇒ drop entirely (no line, no polygon).
        if tags.contains_key("admin_level") {
            return;
        }
        // Skip rings osmium's assembler would reject as invalid, such as a self-intersecting
        // building: no polygon and no line.
        if coords.len() >= 3 && polygon_is_valid(coords, &[]) {
            features.push(
                w.id(),
                IngestFeature {
                    style_id: style.id,
                    min_lod: style.min_lod,
                    geom: Geom::Polygon { exterior: coords.to_vec(), interiors: Vec::new() },
                },
            );
        }
        return;
    }

    // Line: open ways, and closed-but-not-area circular roads.
    if coords.len() >= 2 {
        features.push(
            w.id(),
            IngestFeature { style_id: style.id, min_lod: style.min_lod, geom: Geom::Line(coords.to_vec()) },
        );
    }
}

/// Closed-way area heuristic: `area=yes` is an area, `area=no` never is, otherwise it is an area iff
/// it carries any [`AREA_TAGS`] key.
pub fn is_area(tags: &HashMap<&str, &str>) -> bool {
    // A cliff can form a closed rim, but it still marks an edge, not a filled area.
    if tags.get("natural") == Some(&"cliff") {
        return false;
    }
    match tags.get("area") {
        Some(&"yes") => true,
        Some(&"no") => false,
        _ => AREA_TAGS.iter().any(|k| tags.contains_key(k)),
    }
}

/// Total bounds over geometry and point records. Geometry truncates `v * 1e6` toward zero, and the
/// coords are the exact osmium f64s, so the bbox is stable across runs. Truncation pulls the max
/// edges, and for negative coordinates the min edges, inward by under 1 µdeg; vertices past the
/// shrunken edge are clipped at the root.
pub fn compute_bbox(ing: &Ingested) -> (i64, i64, i64, i64) {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut widen = |x: f64, y: f64| {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    };
    for f in &ing.features {
        let (a, b, c, d) = f.geom.bounds();
        widen(a, b);
        widen(c, d);
    }
    for cl in &ing.coastlines {
        for &(x, y) in cl {
            widen(x, y);
        }
    }
    // `as i64` truncates toward zero — NOT a floor for negatives; see the doc above.
    let mut bounds = ((minx * 1e6) as i64, (miny * 1e6) as i64, (maxx * 1e6) as i64, (maxy * 1e6) as i64);
    for poi in &ing.pois {
        bounds.0 = bounds.0.min(i64::from(poi.lon_udeg));
        bounds.1 = bounds.1.min(i64::from(poi.lat_udeg));
        bounds.2 = bounds.2.max(i64::from(poi.lon_udeg));
        bounds.3 = bounds.3.max(i64::from(poi.lat_udeg));
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tags(pairs: &[(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
        pairs.iter().copied().collect()
    }

    /// The closed-way polygon/line gate: `area=yes` forces an area even with no AREA_TAGS key,
    /// `area=no` forces a line even with one present, and an absent `area` falls back to the keys.
    #[test]
    fn is_area_overrides_and_tag_fallback() {
        assert!(is_area(&tags(&[("area", "yes")])), "area=yes ⇒ area regardless of other tags");
        assert!(!is_area(&tags(&[("area", "no"), ("natural", "water")])), "area=no ⇒ never an area");
        assert!(!is_area(&tags(&[("natural", "cliff")])), "a closed cliff remains a line");
        for key in AREA_TAGS {
            assert!(is_area(&tags(&[(key, "whatever")])), "AREA_TAGS key {key} ⇒ area");
        }
        assert!(!is_area(&tags(&[("highway", "residential")])), "no area tag, no AREA_TAGS key ⇒ line");
        // An unrecognized `area` value falls through to the tag fallback (not yes/no).
        assert!(!is_area(&tags(&[("area", "maybe")])), "unknown area value, no AREA_TAGS key ⇒ line");
        assert!(is_area(&tags(&[("area", "maybe"), ("building", "yes")])), "unknown area value falls back to tags");
    }
}
