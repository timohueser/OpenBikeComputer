---
title: Packer & routing
description: How OpenStreetMap data becomes a device-ready OBCM map (the obc-pack pipeline), and how your uploaded route is followed — converted to OBCR and map-matched to your live position as you ride.
---

# Packer & routing

Two jobs bracket the device's own work. **Packing** turns raw OpenStreetMap data into a styled `.obcm` map — a heavy job, run once on a computer, and (as of v8) it also bakes a [routable navigation graph](#building-the-navigation-graph) into the map — one that v9 makes [bike-type-aware](#weighting-the-graph-bike-profiles). **Routing** is the lighter on-device work: turning a GPX you upload into a navigable `.obcr`, **map-matching** your live position onto it as you ride, and — new this iteration — **computing** a route to a POI on the device itself over that baked graph. So "routing" here is mostly *following* a line you brought, with a memory-bounded bit of *pathfinding* when you ask the device to reach a POI on its own.

The packer ([`obc-pack`](src:firmware/obc-pack)) lives in the same Rust workspace as the device firmware and depends on the same [`obc-reader`](src:firmware/obc-reader), so the program that *writes* the format and the program that *reads* it can never disagree about a byte.

## Packing a map

The pipeline is a straight line from an `.osm.pbf` extract to a finished `.obcm`. Two stages carry the weight — ingest and the per-LOD build — and the rest are quick.

<figure class="fig">
<svg viewBox="0 0 820 240" role="img" aria-label="The packer pipeline as a trail: starting from an OSM .pbf plus a config, the stages are merge, ingest, compute bounding box, generate land, build the per-LOD pyramid (simplify then quadtree), and serialize, ending at a .obcm file. Ingest and the per-LOD build are marked as the expensive stages.">
  <text class="d-tag" x="20" y="24">From OpenStreetMap to a device map</text>

  <!-- trail -->
  <line x1="96" y1="120" x2="742" y2="120" stroke="#5f7d3d" stroke-width="2.5" stroke-dasharray="2 7" stroke-linecap="round" />

  <!-- start -->
  <circle cx="58" cy="120" r="7" class="d-forest" />
  <text class="d-sub" x="58" y="150" text-anchor="middle">.pbf +</text>
  <text class="d-sub" x="58" y="162" text-anchor="middle">config</text>

  <!-- 1 merge (above) -->
  <circle cx="128" cy="120" r="15" class="d-forest" /><text class="d-num" x="128" y="124" text-anchor="middle">1</text>
  <text class="d-label" x="128" y="74" text-anchor="middle">Merge</text>
  <text class="d-sub" x="128" y="88" text-anchor="middle">if &gt; 1 input</text>
  <!-- 2 ingest (below, HOT) -->
  <circle cx="251" cy="120" r="16" class="d-hot-fill" /><text class="d-num" x="251" y="124" text-anchor="middle">2</text>
  <text class="d-label" x="251" y="160" text-anchor="middle" style="fill:#a9501c">Ingest</text>
  <text class="d-sub" x="251" y="174" text-anchor="middle">ways · relations</text>
  <!-- 3 bbox (above) -->
  <circle cx="374" cy="120" r="15" class="d-forest" /><text class="d-num" x="374" y="124" text-anchor="middle">3</text>
  <text class="d-label" x="374" y="74" text-anchor="middle">BBox</text>
  <text class="d-sub" x="374" y="88" text-anchor="middle">truncate µdeg</text>
  <!-- 4 land (below) -->
  <circle cx="497" cy="120" r="15" class="d-forest" /><text class="d-num" x="497" y="124" text-anchor="middle">4</text>
  <text class="d-label" x="497" y="160" text-anchor="middle">Land</text>
  <text class="d-sub" x="497" y="174" text-anchor="middle">clip to bbox</text>
  <!-- 5 per-LOD (above, HOT) -->
  <circle cx="620" cy="120" r="16" class="d-hot-fill" /><text class="d-num" x="620" y="124" text-anchor="middle">5</text>
  <text class="d-label" x="620" y="74" text-anchor="middle" style="fill:#a9501c">Per-LOD</text>
  <text class="d-sub" x="620" y="88" text-anchor="middle">simplify → quadtree</text>
  <!-- 6 serialize (below) -->
  <circle cx="720" cy="120" r="15" class="d-forest" /><text class="d-num" x="720" y="124" text-anchor="middle">6</text>
  <text class="d-label" x="720" y="160" text-anchor="middle">Serialize</text>
  <text class="d-sub" x="720" y="174" text-anchor="middle">stream out</text>

  <!-- end -->
  <rect class="d-panel" x="772" y="104" width="40" height="32" rx="5" style="fill:#e7ead8" />
  <text class="d-sub" x="792" y="124" text-anchor="middle" style="font-size:9px">.obcm</text>
</svg>
<figcaption>Inputs are merged (via <code>osmium</code>) only when there's more than one. Each LOD tier is built and streamed to disk before the next begins, so peak memory is roughly <i>one</i> tier's quadtree rather than the whole pyramid plus the output — the same "never resident if it doesn't have to be" instinct the device's reader uses.</figcaption>
</figure>

### Styling: first match wins

What a feature *is* — and whether it's kept at all — comes from a `config.json`. It's an **ordered** map of `tag_key → value → style`, and a way is styled by the **first** rule (in document order) whose tag it carries. Style IDs are assigned 1-based in that same document order; the config never names them.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="A way's tags, highway=primary and building=yes, are matched against the config rules in document order: highway comes before building, so the highway=primary rule wins and produces a style with id 5, a colour, a z-index, a priority, and a min-LOD.">
  <defs>
    <marker id="aP2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">First matching rule, in document order</text>

  <!-- tags -->
  <rect class="d-panel-2" x="32" y="78" width="150" height="86" rx="10" />
  <text class="d-label" x="48" y="100">a way's tags</text>
  <text class="d-sub" x="48" y="124" font-family="var(--mono)">highway = primary</text>
  <text class="d-sub" x="48" y="144" font-family="var(--mono)">building = yes</text>

  <line class="d-flow" x1="186" y1="120" x2="232" y2="120" marker-end="url(#aP2)" />

  <!-- config rules -->
  <rect class="d-panel" x="240" y="48" width="244" height="160" rx="10" />
  <text class="d-tag" x="256" y="68">config.json · in order</text>
  <g font-family="var(--mono)">
    <rect x="252" y="78" width="220" height="24" rx="5" class="d-hot-fill" />
    <text class="d-sub" x="262" y="94" style="fill:#fff">highway → { primary ★ … }</text>
    <text class="d-sub" x="262" y="120">railway → { … }</text>
    <text class="d-sub" x="262" y="142">natural → { water, land … }</text>
    <text class="d-sub" x="262" y="164">building → { yes … }</text>
    <text class="d-sub" x="262" y="186">admin_level → { 2 … }</text>
  </g>
  <text class="d-sub" x="362" y="226" text-anchor="middle" style="font-size:9px">first key the way carries wins — building is never reached</text>

  <line class="d-flow" x1="488" y1="92" x2="540" y2="92" marker-end="url(#aP2)" />

  <!-- style out -->
  <rect class="d-hot" x="548" y="52" width="150" height="140" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="564" y="72" style="fill:#a9501c">style #5</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="564" y="94">color (RGB565)</text>
    <text class="d-sub" x="564" y="112">z_index · weight</text>
    <text class="d-sub" x="564" y="130">priority 1–4</text>
    <text class="d-sub" x="564" y="148">min_lod</text>
    <text class="d-sub" x="564" y="166">line_style · color2</text>
  </g>
</svg>
<figcaption>A style carries everything the renderer later needs: a colour, a paint order (<code>z_index</code>), a line weight, a drop-priority, a <code>min_lod</code> (the zoom tier below which the feature isn't included), and — since <b>v10</b> — a <code>line_style</code> and an optional secondary colour <code>color2</code> for dashes, casings, stripes and outlines. These become the <a href="../formats/#the-header">style table</a> in the file, and the colours resolve through the very same <a href="../architecture/#two-hosts-one-core-and-the-seams-between-them"><code>color_fn</code></a> the UI uses.</figcaption>
</figure>

```rust
pub fn get_style(&self, tags: &HashMap<&str, &str>) -> Option<&FeatureStyle> {
    for (tag_key, by_value) in &self.features {   // walked in document order
        if let Some(val) = tags.get(tag_key.as_str()) {
            // exact value first, then the category's "*" catch-all
            if let Some(style) = by_value.get(*val).or_else(|| by_value.get("*")) {
                return Some(style);               // first match wins
            }
        }
    }
    None                                          // unstyled → dropped
}
```

Within a `tag_key`, the value `"*"` is a **catch-all**: an exact value match still wins, but any other value that key carries falls back to the `"*"` rule. So `building → { warehouse: …, "*": … }` gives warehouses their own style and paints every other `building=*` with the catch-all — without enumerating OSM's ~50 building values by hand. The catch-all is an ordinary rule (it takes one style ID like any other), so it's purely a packer-side convenience; the file format and the device never know it existed.

### Ingest: two passes, then assemble

OSM is nodes, ways and relations, stored in that order. The ingester reads the `.pbf` twice. **Pass 1** builds a `node id → coordinate` store and notes which *area relations* exist (lakes-with-islands, multi-part forests). **Pass 2** turns ways into lines and polygons — and captures the geometry of any way a relation needs. Then each relation's member ways are assembled into a polygon-with-holes.

<figure class="fig">
<svg viewBox="0 0 720 280" role="img" aria-label="Ingest in two passes. Pass 1 reads the pbf into a node store and collects area relations. Pass 2 turns ways into lines and closed-way polygons and coastlines, capturing member geometry. Then relation member ways are assembled via build_area into a polygon with a hole — a lake with an island.">
  <defs>
    <marker id="aP3" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Two passes over the .pbf, then relation assembly</text>

  <!-- pbf -->
  <rect class="d-panel" x="30" y="96" width="70" height="60" rx="9" />
  <text class="d-sub" x="65" y="130" text-anchor="middle">.pbf</text>

  <!-- pass 1 -->
  <line class="d-flow" x1="104" y1="112" x2="150" y2="92" marker-end="url(#aP3)" />
  <rect class="d-panel-2" x="156" y="56" width="200" height="60" rx="9" />
  <text class="d-label" x="172" y="76">Pass 1</text>
  <text class="d-sub" x="172" y="94">node store · id → coord</text>
  <text class="d-sub" x="172" y="108">+ collect area relations</text>

  <!-- pass 2 -->
  <line class="d-flow" x1="104" y1="140" x2="150" y2="160" marker-end="url(#aP3)" />
  <rect class="d-panel-2" x="156" y="140" width="200" height="76" rx="9" />
  <text class="d-label" x="172" y="160">Pass 2</text>
  <text class="d-sub" x="172" y="178">ways → lines · polygons</text>
  <text class="d-sub" x="172" y="194">coastlines (always)</text>
  <text class="d-sub" x="172" y="210">capture member geometry</text>

  <!-- assemble -->
  <line class="d-flow" x1="360" y1="135" x2="412" y2="135" marker-end="url(#aP3)" />
  <rect class="d-panel" x="420" y="80" width="120" height="110" rx="10" />
  <text class="d-tag" x="436" y="100">build_area</text>
  <!-- lake with island -->
  <path d="M436 110 L524 110 L524 178 L436 178 Z" fill="#bcd3da" stroke="#33575b" stroke-width="1.4" />
  <path d="M462 130 L498 130 L490 160 L468 158 Z" fill="#f3f0df" stroke="#33575b" stroke-width="1.2" />
  <text class="d-sub" x="480" y="200" text-anchor="middle" style="font-size:9px">lake + island = 1 hole</text>

  <!-- closed-way classification note -->
  <rect class="d-panel-2" x="560" y="80" width="140" height="110" rx="10" />
  <text class="d-tag" x="574" y="100">closed way?</text>
  <text class="d-sub" x="574" y="122" style="font-size:9.5px">area tag → polygon</text>
  <text class="d-sub" x="574" y="142" style="font-size:9.5px">else → line</text>
  <text class="d-sub" x="574" y="168" style="font-size:9px">never both — a closed</text>
  <text class="d-sub" x="574" y="180" style="font-size:9px">road loop is a line only</text>
</svg>
<figcaption>Relations are assembled with GEOS <code>build_area</code>, which sorts member rings into outers and holes by geometry — so a lake with an island comes out as a polygon with one interior ring, ready for the <a href="../formats/#features-an-anchor-then-deltas">holes encoding</a>. A closed way is a polygon only if its tags say it encloses an area; a closed <code>highway</code> loop stays a line, never also a filled blob.</figcaption>
</figure>

### Land and sea

OSM ways draw the *coast*, but not the sea or the land fill. Those come from a separate global dataset of land polygons, clipped to the map's bounding box and added as features styled `natural.land`. The sea needs no geometry at all: it's the **backdrop** the renderer clears to before drawing, and land is simply painted on top.

<figure class="fig">
<svg viewBox="0 0 720 230" role="img" aria-label="The global land-polygons dataset, a world map of land shapes, is clipped to the map's bounding box, producing land faces. On the device these are drawn over a sea-coloured backdrop, so land sits on top of sea.">
  <defs>
    <marker id="aP4" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Land is clipped in; sea is the backdrop</text>

  <!-- world dataset -->
  <rect x="36" y="50" width="200" height="150" rx="8" style="fill:#bcd3da;stroke:#33575b;stroke-width:1.4" />
  <text class="d-tag" x="48" y="68" style="fill:#2c5230">global land polygons</text>
  <path d="M60 110 C 90 80, 130 90, 150 110 C 175 135, 140 175, 100 170 C 70 166, 48 140, 60 110 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <path d="M170 150 C 190 135, 220 150, 214 175 C 208 192, 178 190, 170 175 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <!-- bbox window -->
  <rect x="96" y="110" width="80" height="64" fill="none" stroke="#cf6a2a" stroke-width="2.2" />
  <text class="d-sub" x="136" y="105" text-anchor="middle" style="fill:#a9501c;font-size:9px">bbox</text>

  <line class="d-flow" x1="244" y1="125" x2="300" y2="125" marker-end="url(#aP4)" />
  <text class="d-sub" x="272" y="115" text-anchor="middle" style="font-size:9px">clip</text>

  <!-- result: land over sea -->
  <rect x="312" y="50" width="200" height="150" rx="8" style="fill:#bcd3da;stroke:#33575b;stroke-width:1.4" />
  <text class="d-tag" x="324" y="68" style="fill:#2c5230">sea backdrop</text>
  <path d="M312 120 C 340 96, 380 104, 400 120 C 430 144, 470 120, 512 132 L512 200 L312 200 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.4" />
  <text class="d-sub" x="360" y="186" text-anchor="middle" style="font-size:9px">land faces, on top</text>

  <!-- note -->
  <rect class="d-panel-2" x="540" y="74" width="160" height="100" rx="10" />
  <text class="d-sub" x="556" y="98" style="font-size:10px">sea = the lowest-z</text>
  <text class="d-sub" x="556" y="114" style="font-size:10px">style; the screen is</text>
  <text class="d-sub" x="556" y="130" style="font-size:10px">cleared to it, then</text>
  <text class="d-sub" x="556" y="146" style="font-size:10px">land + roads paint</text>
  <text class="d-sub" x="556" y="162" style="font-size:10px">over it.</text>
</svg>
<figcaption>The packer reads the land shapefile directly and reprojects it from Web Mercator with closed-form math — no GIS stack — decoding only the records whose bounding box touches the query. The result flows through the same simplify-and-quadtree path as every other feature, so by the time the device sees it, land is just more geometry.</figcaption>
</figure>

### Extracting POIs

The same OSM extract carries more than geometry. Amenities a bikepacker actually looks for — water, campsites, lodging, resupply, pharmacies, bike shops — are tagged on nodes and areas the geometry pipeline would otherwise style-and-forget. A separate stage harvests them into the map's [POI section](../formats/#pois-a-nearest-list-not-a-map-layer), where the device browses them by category. It's config-free on purpose: the tag → category mapping is **hardcoded in the packer** (a locked decision), so packing the same extract always yields the same POIs.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="POI extraction. On the left, two OSM sources: a tagged node used as-is, and a closed way whose polygon centroid becomes a point. Both are classified against a fixed table of tag-equals-value rules mapping to a category and subtype. Names are folded to ASCII and capped at 24 bytes. Finally a dedup step collapses a node and a way-centroid of the same category within 50 metres into one POI, keeping the node.">
  <defs>
    <marker id="aP6" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Nodes + way-centroids → classify → fold names → dedup</text>

  <!-- sources -->
  <rect class="d-panel-2" x="24" y="44" width="150" height="40" rx="9" />
  <text class="d-sub" x="40" y="62" style="font-size:10px">a tagged node</text>
  <text class="d-sub" x="40" y="77" style="font-size:9px">amenity=drinking_water</text>
  <circle cx="158" cy="64" r="4" class="d-hot-fill" />

  <rect class="d-panel-2" x="24" y="94" width="150" height="52" rx="9" />
  <text class="d-sub" x="40" y="112" style="font-size:10px">a closed way</text>
  <text class="d-sub" x="40" y="127" style="font-size:9px">tourism=camp_site</text>
  <!-- small polygon with centroid dot -->
  <path d="M120 118 L150 116 L156 136 L126 140 Z" fill="none" stroke="#3c6b39" stroke-width="1.2" />
  <circle cx="138" cy="127" r="3" class="d-hot-fill" />
  <text class="d-sub" x="120" y="152" style="font-size:8.5px;fill:#a9501c">→ ring centroid</text>

  <!-- classify -->
  <line class="d-flow" x1="178" y1="64"  x2="224" y2="86" marker-end="url(#aP6)" />
  <line class="d-flow" x1="178" y1="120" x2="224" y2="98" marker-end="url(#aP6)" />
  <rect class="d-panel" x="230" y="56" width="204" height="92" rx="10" />
  <text class="d-tag" x="246" y="76">fixed table · first match</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="246" y="96"  style="font-size:9.5px">amenity=drinking_water → Water</text>
    <text class="d-sub" x="246" y="112" style="font-size:9.5px">natural=spring &nbsp;&nbsp;&nbsp;&nbsp;&nbsp;→ Water</text>
    <text class="d-sub" x="246" y="128" style="font-size:9.5px">tourism=camp_site &nbsp;&nbsp;→ Campsite</text>
    <text class="d-sub" x="246" y="142" style="font-size:9px;fill:#a9501c">… 18 subtypes, 6 categories</text>
  </g>

  <!-- fold names -->
  <line class="d-flow" x1="438" y1="102" x2="484" y2="102" marker-end="url(#aP6)" />
  <rect class="d-panel-2" x="490" y="72" width="206" height="60" rx="10" />
  <text class="d-tag" x="506" y="90">fold name → ASCII, ≤ 24 B</text>
  <text class="d-sub" x="506" y="110" font-family="var(--mono)" style="font-size:10px">"Bäckerei Müller"</text>
  <text class="d-sub" x="506" y="124" font-family="var(--mono)" style="font-size:10px;fill:#a9501c">→ "Baeckerei Mueller"</text>

  <!-- dedup -->
  <line class="d-flow" x1="360" y1="150" x2="360" y2="196" marker-end="url(#aP6)" />
  <rect class="d-hot" x="120" y="200" width="480" height="76" rx="12" style="fill:#f8efe4" />
  <text class="d-tag" x="138" y="220" style="fill:#a9501c">dedup — same category within 50 m = one POI</text>
  <!-- node + centroid merging -->
  <circle cx="168" cy="248" r="5" class="d-hot-fill" /><text class="d-sub" x="150" y="268" style="font-size:9px">node</text>
  <circle cx="210" cy="248" r="4" class="d-water" /><text class="d-sub" x="196" y="268" style="font-size:9px">centroid</text>
  <line x1="176" y1="248" x2="202" y2="248" stroke="#9aa884" stroke-width="1.2" stroke-dasharray="3 2" />
  <line class="d-flow" x1="250" y1="248" x2="300" y2="248" marker-end="url(#aP6)" />
  <circle cx="330" cy="248" r="5" class="d-hot-fill" /><text class="d-sub" x="346" y="252" style="font-size:9.5px">the node wins</text>
  <text class="d-sub" x="470" y="240" style="font-size:9px">priority: node beats centroid,</text>
  <text class="d-sub" x="470" y="254" style="font-size:9px">then named beats unnamed,</text>
  <text class="d-sub" x="470" y="268" style="font-size:9px">then first-seen.</text>
</svg>
<figcaption>Both an OSM <b>node</b> and a <b>closed way</b> can be a POI — a way is reduced to its shoelace-weighted <b>ring centroid</b>, so a campsite polygon becomes a single point. Each candidate is classified against a fixed <code>key=value</code> table (first match in table order wins, the same rule as the style config), and its name is <b>folded to printable ASCII</b> and capped at 24 bytes. The last step matters because OSM double-maps: a drinking-water node sitting inside a same-tagged area, an entrance node beside a campsite polygon. Two candidates of the <b>same category within ~50 m</b> collapse to one, and the winner is chosen by priority — a node (a real placed point) beats a derived centroid, a named POI beats an unnamed one.</figcaption>
</figure>

Why fold names at all? Because the device font is **[Terminus](../ui/#the-field-map-look), printable ASCII only** — it has no glyph for `ä` or `é`. Rather than ship mojibake or a heavier font, the packer transliterates at pack time: German umlauts get their proper digraphs (`ä → ae`, `ß → ss`), the rest of Latin strips to its base letter, and anything genuinely unmappable (CJK, Cyrillic, Greek) becomes a word break rather than gluing neighbours together. A name that folds away to nothing is stored as unnamed, and the device falls back to the subtype's label ("Spring", "Bakery"). The 24-byte cap is a device-row width, not a storage worry — POI bytes are noise next to geometry.

```rust
pub const POI_TABLE: [PoiKind; 18] = [
    kind(1, "amenity", "drinking_water"), // → Water / "Drinking water"
    kind(2, "natural", "spring"),         // → Water / "Spring"
    kind(5, "tourism", "camp_site"),      // → Campsite
    kind(13, "shop", "supermarket"),      // → Resupply / "Supermarket"
    // … 18 rows, ids append-only — the subtype id is normative (OBCM_Spec §7.4)
];
```

The subtype *ids* are normative and shared: the packer owns only the OSM `key=value` half of the table, while each subtype's category and fallback label live once in [`obc-reader`'s `poi_table`](src:firmware/obc-reader/src/poi_table.rs) — the same table the device reads — so the two crates can't drift, and a pinning test asserts every row agrees. The extracted, deduped, name-folded POIs are handed to the serializer, which builds the [per-category quadtrees](../formats/#pois-a-nearest-list-not-a-map-layer) of the POI section.

### Parsing opening hours

A POI carries one more thing worth harvesting: when it's *open*. OSM stores that in the [`opening_hours`](https://wiki.openstreetmap.org/wiki/Key:opening_hours) tag — a compact grammar like `Mo-Fr 08:00-18:00; Sa 09:00-13:00; PH off`. Parsing that grammar is a **host job that never runs on the device**: the microcontroller has no room for a date library and no reason to re-derive the same answer every frame. So the packer parses `opening_hours` **once, at pack time**, into the fixed [29-byte weekly schedule](../formats/#opening-hours-a-pooled-weekly-schedule) the device reads with a single array lookup.

The parser is a deliberate **subset**, not a full `opening_hours` engine — the real data (town shops, campsites) exercises a small corner of the grammar, and a full engine would drag a time model into a build tool that has no clock. It handles weekday ranges and lists (`Mo-Fr`, `Mo,We,Fr`), `HH:MM-HH:MM` intervals (including a split lunch, two per day), `24/7`, `off`/`closed`, bare time-only rules that apply every day, and overnight wrap. Times are rounded to the nearest quarter-hour with the same **round-half-to-even** convention the packer uses for coordinates. Anything it *can't* model it **drops and flags** rather than guessing — it never invents hours that aren't there.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The opening_hours stage. A raw OSM opening_hours string is parsed at pack time into a normalized weekly schedule of seven days. A seasonal date rule is flattened to a representative in-season week and flagged seasonal; a public-holiday or unmodellable rule is dropped and flagged truncated. All resulting schedules are then deduplicated into a small pool, and each POI stores only its pool index.">
  <defs>
    <marker id="aOH" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">opening_hours → normalize → flag → dedup pool</text>

  <!-- raw string -->
  <rect class="d-panel-2" x="24" y="44" width="238" height="52" rx="9" />
  <text class="d-sub" x="40" y="62" style="font-size:9.5px">raw OSM tag</text>
  <text class="d-sub" x="40" y="80" font-family="var(--mono)" style="font-size:9px">Mo-Fr 08:00-18:00; Sa 09-13</text>
  <text class="d-sub" x="40" y="92" font-family="var(--mono)" style="font-size:9px">Apr-Oct: …; PH off</text>

  <!-- parse -->
  <line class="d-flow" x1="266" y1="70" x2="312" y2="70" marker-end="url(#aOH)" />
  <rect class="d-panel" x="318" y="44" width="196" height="52" rx="10" />
  <text class="d-tag" x="334" y="64">parse (subset grammar)</text>
  <text class="d-sub" x="334" y="84" style="font-size:9px">→ 7 days × ≤2 intervals</text>

  <!-- flags -->
  <line class="d-flow" x1="416" y1="96" x2="416" y2="120" marker-end="url(#aOH)" />
  <rect class="d-panel-2" x="300" y="126" width="232" height="58" rx="10" />
  <text class="d-sub" x="316" y="146" style="font-size:9.5px;fill:#a9501c">seasonal — Apr-Oct flattened to</text>
  <text class="d-sub" x="316" y="159" style="font-size:9px">a representative in-season week</text>
  <text class="d-sub" x="316" y="176" style="font-size:9.5px;fill:#a9501c">truncated — PH / 3rd interval dropped</text>

  <!-- dedup pool -->
  <line class="d-flow" x1="300" y1="155" x2="230" y2="200" marker-end="url(#aOH)" />
  <rect class="d-hot" x="24" y="196" width="300" height="44" rx="12" style="fill:#f8efe4" />
  <text class="d-tag" x="40" y="216" style="fill:#a9501c">dedup — a region's shops share hours</text>
  <text class="d-sub" x="40" y="232" style="font-size:9px">identical schedules → one blob; POI stores its index</text>

  <!-- only-with-hours note -->
  <rect class="d-panel-2" x="344" y="196" width="352" height="44" rx="10" />
  <text class="d-sub" x="360" y="214" style="font-size:9.5px">a POI with no parseable hours stores <tspan font-family="var(--mono)">0xFFFF</tspan></text>
  <text class="d-sub" x="360" y="230" style="font-size:9px">— only POIs that actually have hours cost a pool slot</text>
</svg>
<figcaption>The stage runs per POI, after classification. Two cases need flattening. A <b>seasonal</b> rule (<code>Apr-Oct: …</code>) carries a date selector the weekly blob can't express, so the packer bakes a <b>representative in-season week</b> and sets the <i>seasonal</i> flag — the v1 device ignores it, but the bit is there for a future season-aware pass. A rule the subset genuinely can't model — a public-holiday <code>PH</code> clause, a <code>sunrise/sunset</code> time, or a third interval on a day — is <b>dropped</b> and the <i>truncated</i> flag set, so the device knows the schedule is partial rather than wrong. Finally every schedule is <b>deduplicated</b> into a shared pool: because a whole town's shops so often keep the same hours, the pool stays tiny, and a POI with no parseable hours (the common case) costs nothing but a <code>0xFFFF</code> sentinel in its record.</figcaption>
</figure>

The subset grammar, the flag semantics, and the quarter-hour encoding live in [`obc-pack/src/hours.rs`](src:firmware/obc-pack/src/hours.rs); the pooled bytes are described in [`OBCM_Spec.md` §7.5](src:OBCM_Spec.md). The device end — turning a pooled blob into *today's hours* and an *open-now* badge — is the [POI detail view](../ui/#the-poi-detail-view).

### Building the navigation graph

The map so far is geometry the device *draws*. To let the device *route* — compute its own way to a POI — the packer builds one more thing the raw data doesn't contain: a **navigation graph**. Highways in OSM are ways, and the geometry pipeline turns them into styled polylines the moment it resolves their coordinates — dropping the node ids as it goes. But those node ids are the topology: two roads that *share* an OSM node meet there. This stage keeps the node ids for routable ways and recovers the graph from the shared ones. It's serialized into the map's [navigation-graph section](../formats/#the-navigation-graph-a-routable-network), and it's **always built** — a config-free, always-present section like the POIs, so packing the same extract always yields the same graph (there's no toggle to forget).

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="Building the navigation graph in three steps. First, a bike-legality filter keeps most highway ways but excludes motorway and its links, excludes trunk unless it is tagged bicycle equals yes, and hard-excludes anything tagged access equals no or private, bicycle equals no, or motorroad equals yes. Second, junction detection: a node touched by two or more routable ways, or sitting at a way's endpoint, becomes a junction; interior shape points do not. Third, each way is split at its junctions into edges, duplicate and reversed parallel ways are deduplicated by an unordered endpoint pair plus geometry key, and each edge's great-circle length becomes its cost. The result is junction nodes joined by edges.">
  <defs>
    <marker id="aNG" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Filter routable ways → find junctions → split + dedup into edges</text>

  <!-- 1 class/access filter -->
  <rect class="d-panel" x="24" y="52" width="200" height="120" rx="10" />
  <text class="d-tag" x="40" y="72">① routable? highway + access</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="40" y="92"  style="font-size:9px;fill:#2c5230">residential · track · path · cycleway ✓</text>
    <text class="d-sub" x="40" y="107" style="font-size:9px;fill:#2c5230">footway · steps · service ✓ (walk a bike)</text>
    <text class="d-sub" x="40" y="125" style="font-size:9px;fill:#a9501c">motorway ✗ · trunk ✗ unless bicycle=yes</text>
    <text class="d-sub" x="40" y="140" style="font-size:9px;fill:#a9501c">access=no|private · bicycle=no ✗</text>
  </g>
  <text class="d-sub" x="40" y="162" style="font-size:8.5px">independent of render styling</text>

  <!-- 2 junction detection -->
  <line class="d-flow" x1="228" y1="112" x2="264" y2="112" marker-end="url(#aNG)" />
  <text class="d-sub" x="270" y="52" style="font-size:9px;fill:#6b7758">② a shared node = a junction</text>
  <!-- two ways crossing at a node -->
  <line x1="288" y1="140" x2="392" y2="96" stroke="#9aa884" stroke-width="2.4" />
  <line x1="300" y1="86"  x2="380" y2="150" stroke="#9aa884" stroke-width="2.4" />
  <!-- interior shape points (open) -->
  <circle cx="322" cy="128" r="2.6" fill="#ece8cf" stroke="#9aa884" stroke-width="1"/>
  <circle cx="360" cy="112" r="2.6" fill="#ece8cf" stroke="#9aa884" stroke-width="1"/>
  <!-- the shared junction -->
  <circle cx="340" cy="119" r="5" class="d-hot-fill" />
  <text class="d-sub" x="340" y="172" text-anchor="middle" style="font-size:8.5px;fill:#a9501c">touched ≥2× → junction</text>
  <text class="d-sub" x="270" y="192" style="font-size:8.5px">endpoints are always junctions;</text>
  <text class="d-sub" x="270" y="204" style="font-size:8.5px">interior shape points never are</text>

  <!-- 3 split + dedup -->
  <line class="d-flow" x1="404" y1="112" x2="440" y2="112" marker-end="url(#aNG)" />
  <rect class="d-hot" x="448" y="52" width="248" height="120" rx="10" style="fill:#f8efe4" />
  <text class="d-tag" x="464" y="72" style="fill:#a9501c">③ split into edges + dedup</text>
  <text class="d-sub" x="464" y="94"  style="font-size:9.5px">cut each way at every junction</text>
  <text class="d-sub" x="464" y="110" style="font-size:9.5px">→ edge interiors are junction-free</text>
  <text class="d-sub" x="464" y="130" style="font-size:9.5px">dedup key: unordered (a,b) + geometry</text>
  <text class="d-sub" x="464" y="146" style="font-size:9px;fill:#a9501c">a way + its reverse = one edge</text>
  <text class="d-sub" x="464" y="162" style="font-size:9px">cost = great-circle length, metres</text>

  <!-- result graph -->
  <line class="d-flow" x1="360" y1="216" x2="360" y2="242" marker-end="url(#aNG)" />
  <rect class="d-panel-2" x="180" y="246" width="360" height="46" rx="10" />
  <text class="d-sub" x="360" y="266" text-anchor="middle" style="font-size:10px">junction <b>nodes</b> (dense pack-run ids) joined by undirected <b>edges</b></text>
  <text class="d-sub" x="360" y="282" text-anchor="middle" style="font-size:9px">→ serialized as the map's §8 navigation graph</text>
</svg>
<figcaption>The <b>routable predicate</b> reads only a way's routing tags (<code>highway</code>, <code>access</code>, <code>bicycle</code>, <code>motorroad</code>) — never the style config, so a road can be drawn but not routable, or the reverse. Most classes are in; <b>motorway</b> (and its <code>_link</code> ramps) is always out — a bike router must never route onto one — while <b>trunk</b> is out <i>unless</i> the way is explicitly tagged <code>bicycle=yes</code>. <code>access=no</code>/<code>private</code>, <code>bicycle=no</code>/<code>use_sidepath</code>, and <code>motorroad=yes</code> are hard excludes on any class; <code>footway</code> and <code>steps</code> stay in, because it is legal to <i>walk</i> a bike there (preference, not legality, is the router's job — see [profiles](#weighting-the-graph-bike-profiles) below). <b>Junction detection</b> is pure counting: a node touched by two-or-more routable ways is a junction, as is any way's first or last node; a shape point touched once stays inside an edge. Each way is then <b>split</b> at its junctions into edges whose interiors carry no junction, and duplicate or reversed-parallel ways <b>collapse</b> — the dedup key is the unordered endpoint pair <i>plus</i> the geometry <i>plus</i> the way-kind, so two genuinely different roads between the same pair (even a cycleway drawn over a road) both survive. Each edge's <b>cost</b> is its great-circle length in metres, summed with the very same helper the route format uses, so on-device costs can't drift from measured distance. A final hygiene pass <b>prunes islands</b> — tiny disconnected components (fewer than <code>min_component_edges</code>, default 50, edges) are dropped so the device can't snap a rider onto an unroutable islet — and long edges are split at synthetic junctions so every neighbour delta and cost fits the §8 record's <code>int16</code>/<code>uint16</code> fields.</figcaption>
</figure>

A way is routable exactly when it can be *classified* — the same pass that decides legality also computes the edge's **way-kind** byte (its highway + surface class, [below](#weighting-the-graph-bike-profiles)), so `is_routable` is just `classify(tags).is_some()`:

```rust
pub fn is_routable<'a, I: IntoIterator<Item = (&'a str, &'a str)>>(tags: I) -> bool {
    classify(tags).is_some()
}

/// The packed way-kind byte, or None when the way is bike-illegal.
pub fn classify<'a, I: IntoIterator<Item = (&'a str, &'a str)>>(tags: I) -> Option<u8> {
    // ... read highway / surface / bicycle / access / motorroad from tags ...
    if matches!(access, Some("no" | "private")) { return None; }            // hard excludes
    if motorroad == Some("yes") { return None; }
    if matches!(bicycle, Some("no" | "use_sidepath")) { return None; }
    let hclass = match highway? {
        "trunk" | "trunk_link" if bicycle == Some("yes") => 13,             // else excluded, like motorway
        other => highway_class(other)?,                                     // None ⇒ not routable (motorway, …)
    };
    Some((surface_class(surface) << 5) | hclass)                            // way_kind = surface<<5 | highway
}
```

A real pack run logs the graph next to the POI counts, so a glance at the build output confirms it's there and plausibly sized:

```
nav graph: 12874 nodes, 15903 edges, 8421.6 km
```

The in-memory build — the way-kind classification, the bike-legality filter, junction detection, edge split and dedup, island pruning, and great-circle lengths — lives in [`obc-pack/src/nav.rs`](src:firmware/obc-pack/src/nav.rs); turning that graph into the tiled, chunked [§8 section](../formats/#the-navigation-graph-a-routable-network) (the node quadtree, the inline-adjacency records, the byte-addressed edge pool, and the densify + long-edge split that keep every record inside one chunk) is the serializer's job, described in [`OBCM_Spec.md` §8](src:OBCM_Spec.md). What the device *does* with it — snap, profile-weighted A\*, emit — is [the router seam](../architecture/#on-device-routing-the-router-seam).

### Weighting the graph: bike profiles

The graph so far is bike-*legal* but undifferentiated: every edge costs its metres, so the device would route a road bike down a muddy singletrack if it were a few metres shorter. What makes an MTB route differ from a road route — *why your MTB route differs* — is two more things the packer bakes in. Each edge carries a **way-kind** byte, and the section opens with a small table of **bike profiles**; on the device, A\* multiplies each edge's raw metres by the chosen profile's weight for that edge's way-kind, so "shortest" becomes "cheapest *for this bike*."

**Way-kind** is one byte per edge, `way_kind = (surface_class << 5) | highway_class` — a 5-bit **highway class** (0 `cycleway`, 1 `path`, 2 `track`, 3 `footway`, … 10 `tertiary`, 11 `secondary`, 12 `primary`, and 13 `trunk_cycl` for a bike-legal trunk) and a 3-bit **surface class** (`paved`, `compacted`, `gravel`, `dirt`, `rough`, `cobbles`, `grass`, plus `unknown`). Both tables are **locked** and config-free — the same OSM extract always yields the same bytes — and they are the *single vocabulary* profiles are written against. The full canonical table is [`OBCM_Spec.md` §8.6](src:OBCM_Spec.md) (mirrored from the one source of truth, `nav.rs`); the device never sees a raw OSM tag, only this byte.

A **profile** is a display name plus a multiplier for every highway class and every surface class, stored in `1/16` fixed-point (so `16` = 1.0×, and `0` means **forbidden** — that class is dropped from the profile's graph entirely). The map carries 1–8 of them (the default pack ships four); the device's effective weight for an edge is `(highway_mult × surface_mult) >> 4`. Here are the four default profiles' highway weights for a handful of classes (the [preset](src:packer/presets/default.json) has the rest, plus the surface axis):

| highway class | Road | Gravel | MTB | Touring |
|---|---|---|---|---|
| cycleway | 1.0 | 1.1 | 1.3 | 1.0 |
| primary  | 1.8 | 2.2 | 3.5 | 2.6 |
| track    | 6.0 | 1.2 | 1.0 | 1.6 |
| path     | 7.0 | 1.5 | 1.0 | 2.0 |
| steps    | forbidden | forbidden | 3.0 | 6.0 |

Read a column and you can predict the routing. **Worked example:** suppose a rider can reach the same destination two ways — a **1 km stretch of `primary`** road, or a **2 km `cycleway`** that loops around it (both `paved`, so each profile's surface weight scales both sides equally and drops out of the comparison). Multiply length by the highway weight:

- **Road** — primary `1000 × 1.8 = 1800`, cycleway `2000 × 1.0 = 2000`. Road takes the **primary** (1800 < 2000): a road cyclist would rather ride 1 km of quiet main road than 2 km out of the way.
- **MTB** — primary `1000 × 3.5 = 3500`, cycleway `2000 × 1.3 = 2600`. MTB takes the **cycleway** (2600 < 3500): to the MTB profile the primary is so heavily penalised that the 2× detour is still cheaper.

Same two roads, same start and finish, opposite choice — that difference is entirely the profile, and you could have called it from the table above.

**One rule constrains every profile: no weight below 1.0×** (a non-zero multiplier is always ≥ `16`). The on-device A\* uses a great-circle heuristic, which is only admissible — only *safe to trust* — if no edge can cost less than its straight-line distance. A weight under 1.0× would make some edge cheaper than the crow flies and quietly break the ε bound. So the packer **rejects** a config with a non-zero weight below 1.0 (naming the A\* bound in the error), and the reader **clamps** one up to 1.0× defensively. Which is what keeps **ε** meaningful: the router's `f = g + ε·h` with ε = 1.3 returns a path at most 1.3× the *cheapest route under the profile* — not the geometrically shortest, the cheapest once your bike's weights are applied. (When even the tight 1.3× bound exhausts the device's fixed search table, ε **escalates** — 1.3 → 2.0 → 3.0 — to reach farther in the same memory; the bound is then the successful rung's ε. That range mechanism lives with [the router seam](../architecture/#on-device-routing-the-router-seam).)

Which profile the device uses is a single **Bike-type** setting — a bare index into the loaded map's profile table, persisted across reboots. Pick "MTB" and every plan re-weights accordingly; the created-route overview shows the profile it used. If the setting points past a particular map's profile count (a smaller map, a stale setting), the router falls back to profile 0 and the UI honestly shows *profile 0's* name rather than a profile the map doesn't have.

Profiles are the one part of the routing graph that **is** configurable (the topology is not). The web builder's advanced editor has a **Bike-profiles panel**: one row per way-kind class, a multiplier cell per profile, and a **forbidden** toggle for the `0` case — schema-driven from the same class vocabulary above, and it enforces the ≥ 1.0× floor in the editor so a config that the packer would reject can't be exported in the first place. Like every other field, it round-trips to a plain CLI config.

### Building the LOD pyramid

Now the heart of it. The file is a [pyramid of detail levels](../formats/#the-file-front-to-back), and the packer builds each one independently. Two knobs from the config drive it: every feature's **`min_lod`** (the coarsest tier it's allowed into) and each tier's **simplify tolerance**. So the country tier holds a handful of feature types, heavily simplified; the street tier holds everything, at full detail. The presets pick each tolerance pixel-accurately: one pixel at the finest scale the tier is drawn at, which is the next finer tier's `max_mpp` ceiling.

An optional third knob, **`min_area_px`**, declutters the coarse tiers: after simplify, a **polygon** whose projected area falls below that many square pixels — measured at the tier's finest on-screen scale, again the next finer tier's `max_mpp` — is dropped, so a whole region's worth of sub-pixel forest and landuse slivers stop crowding the render's [point budget](../rendering/#4-decode-by-priority-the-clever-bit). It's off by default and never touches the finest tier (nothing coarser to fall back to). Lines are left alone: an OSM way is stored as many short segments, so an area test would drop a road's shortest links and leave it holed — zoomed-out line density stays purely a `min_lod` choice.

<figure class="fig">
<svg viewBox="0 0 720 270" role="img" aria-label="A pool of features each tagged with a min-LOD flows into three tiers. The country tier takes only features with min-LOD 0 and simplifies them at 120 metres. The region tier adds min-LOD 1 features at 18 metres. The street tier adds everything at full detail. Each tier becomes its own quadtree.">
  <defs>
    <marker id="aP5" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Each tier: filter by min_lod, simplify, cull tiny areas, then quadtree</text>

  <!-- feature pool -->
  <rect class="d-panel-2" x="30" y="70" width="140" height="130" rx="10" />
  <text class="d-tag" x="44" y="90">features</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="44" y="112" style="font-size:9.5px">coast · sea  min 0</text>
    <text class="d-sub" x="44" y="132" style="font-size:9.5px">motorway   min 0</text>
    <text class="d-sub" x="44" y="152" style="font-size:9.5px">forest     min 1</text>
    <text class="d-sub" x="44" y="172" style="font-size:9.5px">footway    min 2</text>
    <text class="d-sub" x="44" y="192" style="font-size:9.5px">building   min 2</text>
  </g>

  <!-- tiers -->
  <g>
    <line class="d-flow" x1="174" y1="100" x2="214" y2="92" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="62" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="80">LOD 0 · country</text>
    <text class="d-sub" x="234" y="96" style="font-size:9px">min_lod ≤ 0 · simplify 120 m</text>

    <line class="d-flow" x1="174" y1="135" x2="214" y2="135" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="114" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="132">LOD 1 · region</text>
    <text class="d-sub" x="234" y="148" style="font-size:9px">+ min_lod ≤ 1 · simplify 18 m</text>

    <line class="d-flow" x1="174" y1="170" x2="214" y2="178" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="166" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="184">LOD 2 · street</text>
    <text class="d-sub" x="234" y="200" style="font-size:9px">+ everything · full detail</text>
  </g>

  <!-- each → quadtree -->
  <line class="d-flow" x1="444" y1="83"  x2="492" y2="83"  marker-end="url(#aP5)" />
  <line class="d-flow" x1="444" y1="135" x2="492" y2="135" marker-end="url(#aP5)" />
  <line class="d-flow" x1="444" y1="187" x2="492" y2="187" marker-end="url(#aP5)" />
  <g>
    <rect x="500" y="64" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="87" text-anchor="middle" style="font-size:9px">quadtree</text>
    <rect x="500" y="116" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="139" text-anchor="middle" style="font-size:9px">quadtree</text>
    <rect x="500" y="168" width="56" height="38" rx="5" class="d-muted" /><text class="d-sub" x="528" y="191" text-anchor="middle" style="font-size:9px">quadtree</text>
  </g>
  <text class="d-sub" x="600" y="120" style="font-size:10px">→ the LOD</text>
  <text class="d-sub" x="600" y="136" style="font-size:10px">pyramid in</text>
  <text class="d-sub" x="600" y="152" style="font-size:10px">the .obcm</text>
</svg>
<figcaption>The per-feature simplify runs in parallel across CPU cores (each worker with its own GEOS context, so no geometry crosses threads) while feature order is preserved. This is exactly the structure the device exploits at the other end: zooming out reads one small coarse tier instead of decimating fine geometry — see <a href="../rendering/#2-level-of-detail-pick-the-right-layer">LOD selection</a>.</figcaption>
</figure>

```rust
for i in 0..lods.len() {                              // coarse (0) → fine
    let tol = lods[i].simplify_m / M_PER_DEG;
    let cull_mpp = lods.get(i + 1).and_then(|l| l.max_mpp); // finest tier: None → never cull
    let level: Vec<(u8, Geom)> = features
        .par_iter()                                   // rayon — one GEOS context per thread
        .filter(|f| f.min_lod <= i)                   // the LOD gate
        .filter_map(|f| {
            let g = simplify(&f.geom, tol);
            let too_small = cull_mpp                  // drop sub-min_area_px polygons (lines: never)
                .is_some_and(|mpp| footprint_below(&g, mpp, lods[i].min_area_px));
            (!too_small).then(|| (f.style_id, g))
        })
        .collect();                                   // order preserved
    let tree = build_lod(level, global_bbox, chunk_size); // this tier's quadtree
    serialize_and_stream(tree);                       // write to disk, then drop
}
```

### The quadtree: packing geometry into chunks

Within a tier, features are inserted into a quadtree over the global bounding box. A leaf simply accumulates features until their packed size — `12 + point_count·4` bytes each — would exceed the chunk size; then it **splits** into four (NW · NE · SW · SE) and re-distributes them. A feature that straddles a child boundary is **clipped** to each child's box.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="A region with features being bucketed into a quadtree. A dense corner has been subdivided into four smaller cells, one of them subdivided again. A line feature crossing a cell boundary is clipped into two pieces, one per cell.">
  <text class="d-tag" x="20" y="24">A leaf splits when its chunk fills; straddlers are clipped</text>

  <!-- region -->
  <rect x="40" y="48" width="256" height="256" fill="none" stroke="#3c6b39" stroke-width="1.6" />
  <!-- root split -->
  <line x1="168" y1="48" x2="168" y2="304" stroke="#3c6b39" stroke-width="1.3" />
  <line x1="40" y1="176" x2="296" y2="176" stroke="#3c6b39" stroke-width="1.3" />
  <!-- NE subdivided (dense) -->
  <line x1="232" y1="48" x2="232" y2="176" stroke="#7c9a63" stroke-width="1" />
  <line x1="168" y1="112" x2="296" y2="112" stroke="#7c9a63" stroke-width="1" />
  <!-- one sub-cell subdivided again -->
  <line x1="264" y1="112" x2="264" y2="176" stroke="#9aa884" stroke-width="0.8" />
  <line x1="232" y1="144" x2="296" y2="144" stroke="#9aa884" stroke-width="0.8" />
  <!-- dense dots in NE -->
  <g fill="#3c6b39"><circle cx="248" cy="128" r="1.6"/><circle cx="256" cy="158" r="1.6"/><circle cx="276" cy="124" r="1.6"/><circle cx="284" cy="160" r="1.6"/><circle cx="240" cy="160" r="1.6"/><circle cx="272" cy="150" r="1.6"/></g>
  <text class="d-sub" x="232" y="70" text-anchor="middle" style="font-size:9px">dense → deeper</text>
  <!-- a polygon in SW -->
  <path d="M70 210 L130 205 L140 260 L80 270 Z" fill="#cfe0c2" stroke="#3c6b39" stroke-width="1.2" />
  <!-- a line straddling the vertical boundary -->
  <line x1="120" y1="120" x2="220" y2="150" stroke="#cf6a2a" stroke-width="2.5" />
  <circle cx="168" cy="134" r="3.5" class="d-hot-fill" />
  <text class="d-sub" x="120" y="110" style="fill:#a9501c;font-size:9px">clipped at the boundary →</text>

  <!-- right notes -->
  <text class="d-label" x="330" y="84">leaf fills:</text>
  <text class="d-sub" x="330" y="104" font-family="var(--mono)" style="font-size:11px">size += 12 + pts·4</text>
  <text class="d-sub" x="330" y="124" style="font-size:11px">&gt; chunk_size → split 4</text>
  <line class="d-stroke" x1="330" y1="140" x2="690" y2="140" style="stroke:#9aa884" />
  <text class="d-sub" x="330" y="164" style="font-size:11px">a straddling feature is clipped to</text>
  <text class="d-sub" x="330" y="180" style="font-size:11px">each child's box, so every leaf's</text>
  <text class="d-sub" x="330" y="196" style="font-size:11px">geometry is self-contained — which</text>
  <text class="d-sub" x="330" y="212" style="font-size:11px">is what lets the device decode one</text>
  <text class="d-sub" x="330" y="228" style="font-size:11px">chunk without touching its neighbours.</text>
  <text class="d-sub" x="330" y="258" style="font-size:10px;fill:#a9501c">this is the same tree the device walks to cull</text>
</svg>
<figcaption>Splitting at floor-division midpoints — and clipping straddlers per child — is what makes a chunk independently decodable: each holds whole geometry anchored to its own corner. That's the exact tree the reader walks at render time, so the packer's subdivision math and the renderer's must agree byte for byte. (A recursion guard stops splitting below 10 µdeg, so degenerate density can't recurse forever.)</figcaption>
</figure>

```rust
let delta = 12 + pt_count(&f.geom) * 4;   // this feature's packed size
self.features.push(f);
self.current_size += delta;
if self.current_size > self.chunk_size {  // leaf full → subdivide NW/NE/SW/SE
    self.split();                         // and re-insert the accumulated features
}
```

That this is the *same* quadtree the device walks closes the loop with the other pages: the packer writes it, the [format](../formats/#the-quadtree-index) stores it as a flat `u32` array, and the [renderer](../rendering/#3-the-quadtree-cull-only-the-chunks-you-can-see) walks it to cull.

### The web builder

Everything above hides behind one command: `python -m packer.web_builder` serves a small local web app that turns *"I want a map of the Black Forest"* into an `.obcm` — pick an area on a map (whole [Geofabrik](https://download.geofabrik.de/) regions, or a drawn box the sources are cropped to), pick a style, build, download. Three ideas shape it:

- **Presets over knobs.** The main page offers complete style presets — Bikepacking, Minimal, High detail — each a full packer config shipped in [`packer/presets/`](src:packer/presets) and directly usable with the CLI. An advanced editor still exposes every field the packer accepts (per-feature styling, LOD tiers, the bike-type routing profiles baked into the map, output settings), so nothing is lost for fine-grained work; exports are, again, plain CLI configs.
- **The binary is the schema authority.** `obc-pack schema` prints a JSON Schema describing exactly the config the installed binary parses, and the editor derives its capability from it. When the format grows — as v10's line styles (`line_style`, `color2`) did — the new fields appear in the editor because the *schema* says so, not because the frontend shipped in lockstep.
- **A stateless server.** The working config lives in the browser ("Custom — based on Bikepacking"), never on the server; builds run through a bounded queue into per-job directories and stream progress live. That shape runs locally today and would survive a shared deployment unchanged.

## Following a route

You plan a route elsewhere and upload a GPX. Converting it to an `.obcr` — decimating the geometry for drawing while keeping the stats exact, then chunking it with shared seams — is covered on the [data formats](../formats/#obcr-the-route) page. The converter is one portable `no_std` routine, so it runs on the device or in the simulator.

<figure class="fig">
<svg viewBox="0 0 760 322" role="img" aria-label="Converting a GPX track to an OBCR route in one streaming pass. Left panel, the shape: the stored line keeps only the corners (and one vertex at least every 1.2 km) — vertices within 1 metre of the line between their neighbours are dropped — yet distance and climb are summed over every original point, so the stats stay exact even though the stored geometry is sparse. Right panel, the climb: a raw elevation trace is integrated through a 3-metre dead-band; small wiggles inside the band book no ascent, and only once the trace leaves the band is the climb booked and the reference re-anchored. The same dead-band is shared by the elevation profile and the live barometric climb on the device.">
  <text class="d-tag" x="20" y="22">GPX → OBCR · one streaming pass</text>
  <text class="d-sub" x="20" y="38" style="font-size:9.5px">decimate the geometry, but measure distance + climb from every raw point</text>
  <line x1="384" y1="58" x2="384" y2="300" stroke="#9aa884" stroke-opacity="0.45" stroke-width="1" />
  <text class="d-sub" x="26" y="62" style="font-size:9px;fill:#6b7758">① the shape — decimate, keep the corners</text>
  <polyline points="55,206 130,131 225,166 330,116" fill="none" stroke="#cf6a2a" stroke-width="2" />
  <circle cx="75" cy="187" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="92" cy="169" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="110" cy="150" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="155" cy="142" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="180" cy="151" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="200" cy="160" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="255" cy="156" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="285" cy="140" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="305" cy="129" r="3" fill="#ece8cf" stroke="#9aa884" stroke-width="1.2" />
  <circle cx="55" cy="206" r="3.6" fill="#cf6a2a" />
  <circle cx="130" cy="131" r="3.6" fill="#cf6a2a" />
  <circle cx="225" cy="166" r="3.6" fill="#cf6a2a" />
  <circle cx="330" cy="116" r="3.6" fill="#cf6a2a" />
  <line class="d-stroke" x1="92" y1="169" x2="92" y2="205" style="stroke-width:0.9;stroke:#9aa884;stroke-dasharray:2 2" />
  <text class="d-sub" x="86" y="220" style="font-size:8.5px">dropped — within 1 m of the line</text>
  <text x="130" y="121" text-anchor="middle" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">kept — a corner</text>
  <text class="d-sub" x="330" y="107" text-anchor="middle" style="font-size:8.5px;fill:#6b7758">+ force-keep every ~1.2 km</text>
  <rect x="34" y="246" width="320" height="42" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="194" y="263" text-anchor="middle" style="font-family:var(--mono);font-size:9px;fill:#3c6b39">distance &amp; climb are summed over EVERY raw point</text>
  <text x="194" y="277" text-anchor="middle" style="font-family:var(--mono);font-size:9px;fill:#3c6b39">— exact, though the stored line is decimated</text>
  <text class="d-sub" x="402" y="62" style="font-size:9px;fill:#6b7758">② the climb — a ±3 m dead-band</text>
  <polyline points="430.0,232.0 444.5,237.0 459.0,227.0 473.5,229.5 488.0,222.0 502.5,212.0 517.0,202.0 531.5,192.0 546.0,187.0 560.5,189.5 575.0,197.0 589.5,187.0 604.0,172.0 618.5,157.0 633.0,147.0 647.5,152.0 662.0,137.0 676.5,122.0 691.0,112.0 705.5,117.0 720.0,107.0" fill="none" stroke="#9aa884" stroke-width="1.3" stroke-opacity="0.65" />
  <rect x="531.5" y="177.0" width="72.5" height="30.0" fill="#cf6a2a" fill-opacity="0.08" stroke="#cf6a2a" stroke-opacity="0.35" stroke-width="0.8" stroke-dasharray="3 3" />
  <text x="608.0" y="195.0" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">±3 m</text>
  <polyline points="430.0,232.0 502.5,232.0 502.5,212.0 531.5,212.0 531.5,192.0 604.0,192.0 604.0,172.0 618.5,172.0 618.5,157.0 662.0,157.0 662.0,137.0 676.5,137.0 676.5,122.0 720.0,122.0 720.0,107.0 720.0,107.0" fill="none" stroke="#cf6a2a" stroke-width="2" />
  <text class="d-sub" x="430" y="252" style="font-size:8.5px;fill:#6b7758">along the route →</text>
  <text class="d-sub" x="402" y="100" style="font-size:8.5px;fill:#6b7758">elev</text>
  <text class="d-sub" x="455" y="210" style="font-size:8.5px;fill:#6b7758">wiggle &lt; 3 m → ignored</text>
  <text x="592" y="120" text-anchor="middle" style="font-family:var(--mono);font-size:8.5px;fill:#a9501c">past ±3 m → book + re-anchor</text>
  <rect x="430" y="266" width="296" height="22" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="578" y="281" text-anchor="middle" style="font-family:var(--mono);font-size:9px;fill:#3c6b39">one dead-band, shared: converter · profile · live baro climb</text>
</svg>
<figcaption>The GPX → OBCR conversion is one streaming pass. It <b>decimates</b> the geometry for storage — dropping any vertex within a metre of the line between its neighbours, keeping the corners (and one vertex at least every ~1.2 km, so a long straight still holds its shape and the deltas stay in <code>int16</code>) — but accumulates <b>distance and climb over every original point</b>, so the route's stats are exact even though the stored line is sparse. Climb runs through a <b>±3 m dead-band</b>: a move smaller than that books nothing and doesn't move the reference, so sampling jitter can't inflate the ascent. That one dead-band is shared by the converter, the elevation profile, and the device's live barometric climb — so the three numbers can't drift apart.</figcaption>
</figure>

What's left is the interesting part of *following*: snapping each GPS fix onto that route.

### Map-matching: a forward-biased cursor

The matcher keeps a cursor — *which segment of the route you're on* — and for each fix searches a **bounded window** around it for the nearest segment. On-route that window is small and looks mostly *forward*, which is the whole trick: on a loop or an out-and-back, the cursor follows you forward instead of snapping to the nearby segment you rode an hour ago.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="A route polyline with a cursor on it. A bounded forward window of dozens of segments ahead (and a few behind) is highlighted around the cursor. A GPS fix off to the side is projected onto the nearest segment in that window, giving a progress distance along the route and a cross-track distance to it. A far fix is flagged off-route and freezes progress.">
  <text class="d-tag" x="20" y="24">Snap each fix to the nearest segment in a forward window</text>

  <!-- route -->
  <path d="M40 210 C 120 120, 200 120, 270 170 C 320 205, 380 150, 470 120" fill="none" stroke="#9aa884" stroke-width="3" />
  <!-- forward window (highlighted) -->
  <path d="M200 138 C 235 128, 255 150, 285 168" fill="none" stroke="#cf6a2a" stroke-width="5" stroke-opacity="0.3" />
  <text class="d-sub" x="232" y="120" text-anchor="middle" style="fill:#a9501c;font-size:9px">forward window</text>
  <!-- cursor -->
  <circle cx="210" cy="135" r="5" class="d-hot-fill" /><text class="d-sub" x="186" y="128" style="font-size:9px">cursor</text>
  <!-- a fix near the route -->
  <circle cx="262" cy="178" r="5" class="d-water" />
  <text class="d-sub" x="262" y="200" text-anchor="middle" style="font-size:9px">fix</text>
  <!-- cross-track to nearest segment -->
  <line x1="262" y1="178" x2="259" y2="159" stroke="#33575b" stroke-width="1.5" stroke-dasharray="3 2" />
  <circle cx="259" cy="159" r="3" class="d-water" />
  <text class="d-sub" x="300" y="170" style="font-size:9px">cross-track dist</text>
  <!-- progress label -->
  <text class="d-sub" x="120" y="200" style="font-size:9px">← progress along the route</text>

  <!-- off-route fix -->
  <circle cx="430" cy="210" r="5" class="d-muted" stroke="#c0492e" stroke-width="1.5" />
  <text class="d-sub" x="430" y="230" text-anchor="middle" style="font-size:9px">far fix</text>
  <line x1="430" y1="210" x2="452" y2="135" stroke="#c0492e" stroke-width="1.2" stroke-dasharray="3 3" />
  <text class="d-sub" x="430" y="244" text-anchor="middle" style="fill:#c0492e;font-size:9px">off-route → freeze</text>

  <!-- hysteresis band note -->
  <rect class="d-panel-2" x="500" y="60" width="196" height="120" rx="10" />
  <text class="d-tag" x="516" y="80">off-route hysteresis</text>
  <text class="d-sub" x="516" y="102" style="font-size:10px">&gt; 25 m  → off-route</text>
  <text class="d-sub" x="516" y="122" style="font-size:10px">&lt; 15 m  → back on</text>
  <text class="d-sub" x="516" y="146" style="font-size:9px">the gap is the dead-band that</text>
  <text class="d-sub" x="516" y="160" style="font-size:9px">keeps the flag from flapping</text>
  <text class="d-sub" x="516" y="174" style="font-size:9px">on GPS jitter at the edge</text>
</svg>
<figcaption>The window is the key to cost and correctness both: it makes each match O(window) instead of O(route) — only the few chunks it spans are decoded — and its forward bias stops a second lap from latching onto the first. The very first fix is the exception: it scans the whole route once to lock on from anywhere, preferring the <i>earliest</i> of near-equal matches so an out-and-back doesn't start at the finish line.</figcaption>
</figure>

```rust
pub struct Match {
    pub progress_m: u32, // distance travelled along the route — frozen while off-route
    pub off_route: bool, // nearest segment is past the hysteresis threshold
    pub dist_m: u32,     // cross-track distance to the route — always live
}
```

Two rules keep it honest when you wander. **Off-route freezes progress**: a fix 200 m away can't drag your route position with it — but the search window *widens* so a rejoin further along is still found. And the off-route flag has **hysteresis** (off past 25 m, back under 15 m), so it doesn't flicker while you straddle the threshold. The live cross-track distance feeds the UI's [off-route readout](../ui/#the-whole-flow), and `progress_m` drives "distance to go." The projection it uses is shared with the GPX converter, so the matcher and the stored geometry measure the route the same way.

```rust
let (back, fwd) = if !self.started {
    (i64::MAX, i64::MAX)        // first fix: scan the whole route to lock on
} else if self.off_route {
    (BACK_SEGS, FWD_SEGS_OFF)   // off-route: widen the forward search to find a rejoin
} else {
    (BACK_SEGS, FWD_SEGS_ON)    // on-route: a tight forward window — O(window), forward-biased
};
```

---

## Where this lives

- The packer pipeline driver: [`obc-pack/src/main.rs`](src:firmware/obc-pack/src/main.rs)
- Config + first-match styling: [`obc-pack/src/config.rs`](src:firmware/obc-pack/src/config.rs)
- The config's JSON Schema (served as `obc-pack schema`): [`obc-pack/schema/config.schema.json`](src:firmware/obc-pack/schema/config.schema.json)
- The web builder — FastAPI server + Svelte app: [`packer/web_builder/`](src:packer/web_builder)
- OSM ingest + relation assembly: [`obc-pack/src/ingest.rs`](src:firmware/obc-pack/src/ingest.rs)
- The quadtree build: [`obc-pack/src/quadtree.rs`](src:firmware/obc-pack/src/quadtree.rs)
- Land generation: [`obc-pack/src/land.rs`](src:firmware/obc-pack/src/land.rs)
- POI extraction, classification, name folding + dedup: [`obc-pack/src/poi.rs`](src:firmware/obc-pack/src/poi.rs)
- The navigation-graph build (routable filter, junction detection, edge split + dedup): [`obc-pack/src/nav.rs`](src:firmware/obc-pack/src/nav.rs)
- The on-device router (snap + profile-weighted A\* + OBCR emit): [`obc-route/src/nav.rs`](src:firmware/obc-route/src/nav.rs)
- The route map-matcher: [`obc-route/src/matcher.rs`](src:firmware/obc-route/src/matcher.rs)
- GPX → OBCR conversion: [`obc-route/src/convert.rs`](src:firmware/obc-route/src/convert.rs)

This is the offline bookend to the on-device story: the packer produces the [map format](../formats/) the [renderer](../rendering/) draws, and the matcher drives the navigation the [UI](../ui/) shows.
