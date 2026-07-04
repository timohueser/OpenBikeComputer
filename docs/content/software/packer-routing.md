---
title: Packer & routing
description: How OpenStreetMap data becomes a device-ready OBCM map (the obc-pack pipeline), and how your uploaded route is followed — converted to OBCR and map-matched to your live position as you ride.
---

# Packer & routing

Two jobs bracket the device's own work. **Packing** turns raw OpenStreetMap data into a styled `.obcm` map — a heavy job, run once on a computer. **Routing** is the lighter on-device pair: turning a GPX you upload into a navigable `.obcr`, and **map-matching** your live position onto it as you ride. The device never computes a route for you — you bring your own line — so "routing" here means *following*, not pathfinding.

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
  <rect class="d-hot" x="548" y="56" width="150" height="120" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="564" y="76" style="fill:#a9501c">style #5</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="564" y="98">color (RGB565)</text>
    <text class="d-sub" x="564" y="118">z_index · weight</text>
    <text class="d-sub" x="564" y="138">priority 1–4</text>
    <text class="d-sub" x="564" y="158">min_lod</text>
  </g>
</svg>
<figcaption>A style carries everything the renderer later needs: a colour, a paint order (<code>z_index</code>), a line weight, a drop-priority, and a <code>min_lod</code> — the zoom tier below which the feature isn't included. These become the <a href="../formats/#the-header">style table</a> in the file, and the colours resolve through the very same <a href="../architecture/#two-hosts-one-core-and-the-seams-between-them"><code>color_fn</code></a> the UI uses.</figcaption>
</figure>

```rust
pub fn get_style(&self, tags: &HashMap<&str, &str>) -> Option<&FeatureStyle> {
    for (tag_key, by_value) in &self.features {   // walked in document order
        if let Some(val) = tags.get(tag_key.as_str()) {
            if let Some(style) = by_value.get(*val) {
                return Some(style);               // first match wins
            }
        }
    }
    None                                          // unstyled → dropped
}
```

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

### Building the LOD pyramid

Now the heart of it. The file is a [pyramid of detail levels](../formats/#the-file-front-to-back), and the packer builds each one independently. Two knobs from the config drive it: every feature's **`min_lod`** (the coarsest tier it's allowed into) and each tier's **simplify tolerance**. So the country tier holds a handful of feature types, heavily simplified; the street tier holds everything, at full detail.

<figure class="fig">
<svg viewBox="0 0 720 270" role="img" aria-label="A pool of features each tagged with a min-LOD flows into three tiers. The country tier takes only features with min-LOD 0 and simplifies them at 50 metres. The region tier adds min-LOD 1 features at 12 metres. The street tier adds everything at full detail. Each tier becomes its own quadtree.">
  <defs>
    <marker id="aP5" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Each tier: filter by min_lod, simplify, then quadtree</text>

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
    <text class="d-sub" x="234" y="96" style="font-size:9px">min_lod ≤ 0 · simplify 50 m</text>

    <line class="d-flow" x1="174" y1="135" x2="214" y2="135" marker-end="url(#aP5)" />
    <rect class="d-panel" x="220" y="114" width="220" height="42" rx="8" />
    <text class="d-label" x="234" y="132">LOD 1 · region</text>
    <text class="d-sub" x="234" y="148" style="font-size:9px">+ min_lod ≤ 1 · simplify 12 m</text>

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
    let level: Vec<(u8, Geom)> = features
        .par_iter()                                   // rayon — one GEOS context per thread
        .filter(|f| f.min_lod <= i)                   // the LOD gate
        .map(|f| (f.style_id, simplify(&f.geom, tol)))
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

- **Presets over knobs.** The main page offers complete style presets — Bikepacking, Minimal, High detail — each a full packer config shipped in [`packer/presets/`](src:packer/presets) and directly usable with the CLI. An advanced editor still exposes every field the packer accepts (per-feature styling, LOD tiers, output settings), so nothing is lost for fine-grained work; exports are, again, plain CLI configs.
- **The binary is the schema authority.** `obc-pack schema` prints a JSON Schema describing exactly the config the installed binary parses, and the editor derives its capability from it. When the format grows — say v6's line styles — the new fields appear in the editor because the *schema* says so, not because the frontend shipped in lockstep.
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
- The route map-matcher: [`obc-route/src/matcher.rs`](src:firmware/obc-route/src/matcher.rs)
- GPX → OBCR conversion: [`obc-route/src/convert.rs`](src:firmware/obc-route/src/convert.rs)

This is the offline bookend to the on-device story: the packer produces the [map format](../formats/) the [renderer](../rendering/) draws, and the matcher drives the navigation the [UI](../ui/) shows.
