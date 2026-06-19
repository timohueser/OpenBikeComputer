# Priority-Based Rendering & Buffer Optimization

Add a user-configurable priority flag per feature type, store it in the OBCM binary, and use it in the renderer to ensure essential features (land, roads, water) are always drawn before nice-to-have ones (buildings, parks) when buffers fill up. Also increase buffer sizes for 512 KB RAM, centralize the constants, and surface saturation stats in the simulator's control panel.

## Overview of Changes

```mermaid
flowchart LR
    A["webapp UI\n+ priority checkbox"] --> B["config.json\n+ priority: true/false"]
    B --> C["packer/pack.py\nserialize priority"]
    C --> D[".obcm file\nstyle table v5\n+ priority bit"]
    D --> E["obc-reader\nparse priority flag"]
    E --> F["obc-render\ntwo-pass collect\n+ stats tracking"]
    F --> G["obc-sim GUI\nstats overlay panel"]
```

---

## 1. Config & Webapp: Priority Checkbox

### Concept

Each feature type in `config.json` gets an optional `"priority": true/false` field (default `false`). Land polygons (from the shapefile) are **always** treated as priority regardless of this flag — enforced in the packer. The webapp shows a checkbox column in the feature table.

### [MODIFY] [config.json](file:///Users/timo/Documents/OSM/config.json) / [user_config.json](file:///Users/timo/Documents/OSM/user_config.json)

Add `"priority": true` to feature types that should be rendered first. Sensible defaults matching the discussion:

```json
"highway": {
    "motorway": {"z_index": 60, "color": "0xE8E4", "weight": 5, "min_lod": 0, "priority": true},
    "trunk":    {"z_index": 55, "color": "0xF9A6", "weight": 4, "min_lod": 0, "priority": true},
    "primary":  {"z_index": 50, "color": "0xF9A6", "weight": 4, "min_lod": 0, "priority": true},
    "secondary":{"z_index": 40, "color": "0xF79E", "weight": 3, "min_lod": 1, "priority": true},
    "tertiary": {"z_index": 30, "color": "0xFFFF", "weight": 2, "min_lod": 1, "priority": true},
    "residential":{"z_index": 20, "color": "0xFFFF", "weight": 1, "min_lod": 2, "priority": true},
    "service":  {"z_index": 10, "color": "0xEEEE", "weight": 1, "min_lod": 2},
    ...
},
"natural": {
    "water": {"z_index": 20, "color": "0x3333", "weight": 1, "min_lod": 1, "priority": true},
    "sea":   {"z_index": 0,  "color": "0x001F", "weight": 1, "min_lod": 0, "priority": true},
    "land":  {"z_index": 1,  "color": "0xEFD5", "weight": 1, "min_lod": 0, "priority": true},
    ...
},
"building": {
    "yes": {"z_index": 5, ..., "priority": false}
}
```

Features without the key default to `false` (no priority).

---

### [MODIFY] [app.js](file:///Users/timo/Documents/OSM/packer/web_builder/static/app.js)

#### Changes to `buildRow()` (~line 601)

Add a priority checkbox column between the enable checkbox and the type name. The checkbox binds to `def.priority`.

```js
// In buildRow(), after the enable checkbox (tdToggle):
const tdPrio = document.createElement("td");
const prioCb = document.createElement("input");
prioCb.type = "checkbox";
prioCb.checked = !!def.priority;
prioCb.title = "Priority: render this feature before non-priority ones when buffers fill up";
prioCb.onchange = () => {
    def.priority = prioCb.checked;
    scheduleSave();
};
tdPrio.appendChild(prioCb);
```

Update the `<thead>` in `renderStyleEditor()` (~line 559) to add a `★` (or `P`) column header:

```diff
-"<thead><tr><th></th><th></th><th>type</th><th>LODs</th><th>color</th><th>z</th><th>w</th><th></th></tr></thead>";
+"<thead><tr><th></th><th></th><th title=\"Priority: drawn first when buffers fill\">★</th><th>type</th><th>LODs</th><th>color</th><th>z</th><th>w</th><th></th></tr></thead>";
```

Update the column array at the end of `buildRow()` (~line 690):

```diff
-for (const td of [tdHandle, tdToggle, tdName, tdLod, tdColor, tdZ, tdW, tdDel]) tr.appendChild(td);
+for (const td of [tdHandle, tdToggle, tdPrio, tdName, tdLod, tdColor, tdZ, tdW, tdDel]) tr.appendChild(td);
```

Update `addFeature()` default (~line 748) to include `priority: false`:

```diff
-const def = { z_index: 10, color: "0xFFFF", weight: 1, min_lod: lodCount() - 1 };
+const def = { z_index: 10, color: "0xFFFF", weight: 1, min_lod: lodCount() - 1, priority: false };
```

Update `buildConfigForSubmit()` (~line 802) to include priority:

```diff
-out.features[cat][name] = { ...def, min_lod };
+out.features[cat][name] = { ...def, min_lod, priority: !!def.priority };
```

#### Changes to `addFeature()` colSpan

Update the `colSpan` from 8 to 9 since we added a column:

```diff
-td.colSpan = 8;
+td.colSpan = 9;
```

---

### [MODIFY] [style.css](file:///Users/timo/Documents/OSM/packer/web_builder/static/style.css)

Minor styling for the priority checkbox column — same width as the enable checkbox. No separate class needed; the existing `input[type="checkbox"]` styling applies.

---

## 2. OBCM Format: Priority Flag in Style Table

### Approach: Use a Flags Byte

Currently each style record is 5 bytes: `ID(u8), Z-Index(i8), Color(u16), Weight(u8)`. 

Add a 6th byte: **Flags** (`u8`), where bit 0 = priority. This is a **breaking format change** — bump the version to **v5**.

> [!IMPORTANT]
> This is a version bump from v4 → v5. Existing `.obcm` files won't load in the new reader. This is fine since the user rebuilds maps from the webapp anyway, but worth noting.

New style record (6 bytes):

| Field | Size | Type | Description |
|-------|------|------|-------------|
| ID | 1 | `uint8` | Style ID |
| Z-Index | 1 | `int8` | Painter's order |
| Color | 2 | `uint16` | RGB565 |
| Weight | 1 | `uint8` | Stroke width |
| **Flags** | **1** | **`uint8`** | **Bit 0: priority** |

### [MODIFY] [serialize.py](file:///Users/timo/Documents/OSM/obcm/serialize.py)

#### `pack_style_dict()` (~line 6)

```diff
-data += struct.pack("<BbHB",
-                    s["id"],
-                    s.get("z_index", 0),
-                    color,
-                    s.get("weight", 1))
+flags = 0
+if s.get("priority", False):
+    flags |= 0x01
+data += struct.pack("<BbHBB",
+                    s["id"],
+                    s.get("z_index", 0),
+                    color,
+                    s.get("weight", 1),
+                    flags)
```

#### `serialize_lods()` (~line 184)

Bump the version byte:

```diff
-b"OBCM",
-0x04,
+b"OBCM",
+0x05,
```

### [MODIFY] [packer/pack.py](file:///Users/timo/Documents/OSM/packer/pack.py)

#### Land polygon priority (~line 100)

When inserting land polygons from the shapefile, force `priority: True`:

```diff
-features.append({"style_id": land_style, "min_lod": land_min_lod, "geometry": poly})
+features.append({"style_id": land_style, "min_lod": land_min_lod, "priority": True, "geometry": poly})
```

Actually, priority lives in the *style* (style table), not per-feature. The style for `natural.land` already gets `priority: True` from the config. So no per-feature change is needed in the packer — the config entry for `natural.land` just needs to have `"priority": true` set.

> [!NOTE]
> Priority is a property of the **style**, not individual features. All features of a given style share the same priority. Land polygons use the `natural.land` style, which the config marks as priority. This is correct — land is always priority because its style says so, and shapefile-derived land polygons use that same style.

### [MODIFY] [config.py](file:///Users/timo/Documents/OSM/obcm/config.py)

No changes needed — `assign_style_ids()` only assigns IDs. The `priority` field passes through as a dict key naturally.

---

## 3. Reader: Parse the Priority Flag

### [MODIFY] [reader.rs](file:///Users/timo/Documents/OSM/firmware/obc-reader/src/reader.rs)

#### `Style` struct (~line 28)

```diff
 pub struct Style {
     pub id: u8,
     pub z_index: i8,
     pub color: u16,
     pub weight: u8,
+    pub priority: bool,
 }
```

#### Version check (~line 174)

```diff
-if version != 4 {
+if version != 5 {
```

#### `parse_styles()` (~line 431)

Change the record size from 5 to 6 bytes:

```diff
-if o + 5 > data.len() {
+if o + 6 > data.len() {
     break;
 }
 let id = data[o];
 let z_index = data[o + 1] as i8;
 let color = rd_u16(data, o + 2);
 let weight = data[o + 4];
-styles[id as usize] = Some(Style { id, z_index, color, weight });
-o += 5;
+let flags = data[o + 5];
+let priority = flags & 0x01 != 0;
+styles[id as usize] = Some(Style { id, z_index, color, weight, priority });
+o += 6;
```

---

## 4. Renderer: Two-Pass Priority Collection & Buffer Sizes

### [MODIFY] [lib.rs](file:///Users/timo/Documents/OSM/firmware/obc-render/src/lib.rs)

#### Centralized buffer size constants (new, at top of file)

Add prominent, well-documented constants for all buffer sizes:

```rust
// ---------------------------------------------------------------------------
// Buffer capacity constants.
//
// These control the maximum number of features, points, and rings the renderer
// can hold per frame.  Tuned for an MCU with 512 KB of RAM.  Every buffer is
// statically allocated (heapless::Vec), so increasing these costs RAM at boot,
// not per-frame.  Adjust if moving to a different target.
// ---------------------------------------------------------------------------

/// Maximum visible features per frame (each is a `Span` — ~20 bytes).
pub const MAX_SPANS: usize = 1024;

/// Maximum total vertices across all visible features per frame (8 bytes each).
pub const MAX_FRAME_POINTS: usize = 16_384;

/// Maximum total ring entries across all visible features per frame.
pub const MAX_FRAME_RINGS: usize = 4096;

/// Maximum vertices for a single feature during decode (reused per feature).
pub const MAX_DECODE_POINTS: usize = 2048;

/// Maximum rings for a single feature during decode.
pub const MAX_DECODE_RINGS: usize = 32;

/// Maximum quadtree leaf nodes overlapping the viewport.
pub const MAX_CHUNKS: usize = 128;

/// Maximum projected screen points for drawing one feature.
pub const MAX_SCREEN_POINTS: usize = 4096;

/// Maximum scanline crossings for polygon fill.
pub const MAX_CROSSINGS: usize = 256;
```

**Memory budget** (approximate):

| Buffer | Items | Item size | Total |
|--------|-------|-----------|-------|
| `spans` | 1,024 | ~20 B | 20 KB |
| `frame_points` | 16,384 | 8 B | 128 KB |
| `frame_ring_lens` | 4,096 | 8 B | 32 KB |
| `dec_points` | 2,048 | 8 B | 16 KB |
| `dec_ring_lens` | 32 | 8 B | 0.25 KB |
| `chunks` | 128 | 20 B | 2.5 KB |
| `screen` | 4,096 | 8 B | 32 KB |
| `xs` | 256 | 4 B | 1 KB |
| **Total** | | | **~232 KB** |

That leaves ~280 KB of the 512 KB for stack, the reader, the app state, the framebuffer (240×320×2 = 150 KB for RGB565), and other firmware needs. Tight but workable.

> [!WARNING]
> The framebuffer alone is ~150 KB (240×320 RGB565). With 232 KB of render buffers + 150 KB framebuffer = 382 KB, leaving only 130 KB for stack, reader, app state, and firmware overhead. If this is too tight, `MAX_FRAME_POINTS` is the biggest knob to turn (halving it saves 64 KB).

#### `MapRenderer` struct (~line 182)

Use the constants:

```rust
pub struct MapRenderer {
    dec_points: Vec<(i32, i32), MAX_DECODE_POINTS>,
    dec_ring_lens: Vec<usize, MAX_DECODE_RINGS>,
    chunks: Vec<(u32, BBox), MAX_CHUNKS>,
    frame_points: Vec<(i32, i32), MAX_FRAME_POINTS>,
    frame_ring_lens: Vec<usize, MAX_FRAME_RINGS>,
    spans: Vec<Span, MAX_SPANS>,
    screen: Vec<Point, MAX_SCREEN_POINTS>,
    xs: Vec<f32, MAX_CROSSINGS>,
}
```

#### `RenderStats` (~line 156)

Add saturation tracking:

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderStats {
    pub lod: usize,
    pub features_tried: usize,
    pub features_drawn: usize,
    pub features_dropped: usize,
    pub points_tried: usize,
    pub points_drawn: usize,
    // Buffer utilization (0.0–1.0) for saturation display.
    pub span_utilization: f32,
    pub point_utilization: f32,
    pub ring_utilization: f32,
}
```

#### Two-pass collect phase (~line 228–300)

Extract the feature-collection logic into a helper closure/method, then call it twice:

```rust
// --- Collect phase: two-pass priority system. ---
// Pass 1: collect features whose style has `priority == true`.
// Pass 2: fill remaining capacity with non-priority features.
// This ensures that when buffers fill up, the dropped features are
// the least visually important ones (buildings, minor paths, etc.).

let collect_features = |
    chunks: &Vec<(u32, BBox), MAX_CHUNKS>,
    reader: &Reader,
    lod: usize,
    dec_points: &mut Vec<(i32, i32), MAX_DECODE_POINTS>,
    dec_ring_lens: &mut Vec<usize, MAX_DECODE_RINGS>,
    frame_points: &mut Vec<(i32, i32), MAX_FRAME_POINTS>,
    frame_ring_lens: &mut Vec<usize, MAX_FRAME_RINGS>,
    spans: &mut Vec<Span, MAX_SPANS>,
    stats: &mut RenderStats,
    view: &BBox,
    priority_pass: bool,  // true = only priority, false = only non-priority
| {
    for &(cid, node) in chunks.iter() {
        reader.for_each_feature(lod, cid, &node, dec_points, dec_ring_lens, |f| {
            let style = match reader.style(f.style_id) {
                Some(s) => s,
                None => return,
            };

            // Filter by priority pass.
            if style.priority != priority_pass {
                return;
            }

            let pts = f.points();
            let lens = f.ring_lens();

            stats.features_tried += 1;
            stats.points_tried += pts.len();

            if pts.is_empty() { return; }

            // Feature bbox check (unchanged).
            // ... existing bbox intersection code ...

            // Capacity check.
            if spans.is_full()
                || frame_points.capacity() - frame_points.len() < pts.len()
                || frame_ring_lens.capacity() - frame_ring_lens.len() < lens.len()
            {
                stats.features_dropped += 1;
                return;
            }

            stats.features_drawn += 1;
            stats.points_drawn += pts.len();

            // Push span + geometry (unchanged).
            // ...
        });
    }
};

// Pass 1: priority features.
collect_features(chunks, reader, lod, dec_points, dec_ring_lens,
    frame_points, frame_ring_lens, spans, &mut stats, &view, true);

// Pass 2: non-priority features.
collect_features(chunks, reader, lod, dec_points, dec_ring_lens,
    frame_points, frame_ring_lens, spans, &mut stats, &view, false);

// Record utilization for the stats panel.
stats.span_utilization = spans.len() as f32 / spans.capacity() as f32;
stats.point_utilization = frame_points.len() as f32 / frame_points.capacity() as f32;
stats.ring_utilization = frame_ring_lens.len() as f32 / frame_ring_lens.capacity() as f32;
```

> [!NOTE]
> Since this is `no_std` and uses `heapless::Vec` with const generics, the helper will likely need to be a regular function (not a closure) to avoid issues with the const generic parameters. Alternatively, it can be a method on `MapRenderer` that borrows the split fields. The exact approach will be determined during implementation.

#### Draw phase

No changes — the painter's algorithm sort by `(z, seq)` remains identical. Priority only affects *which* features make it into the buffers, not draw order.

---

## 5. Simulator GUI: Stats Overlay Panel

### [MODIFY] [gui.rs](file:///Users/timo/Documents/OSM/firmware/obc-sim/src/gui.rs)

#### Store last frame's stats in `SimGui`

Add a field:

```rust
struct SimGui {
    // ... existing fields ...
    last_stats: obc_render::RenderStats,
}
```

Updated in `render_to_texture()` after the render call.

#### Remove the `println!` spam (~line 244)

```diff
-println!("RenderStats: LOD={}, Features: {}/{} drawn, Points: {}/{} drawn", ...);
+self.last_stats = stats;
```

#### New stats section in `show_control_panel()` (~after line 427)

Add a collapsible "Render Stats" section at the bottom of the control panel:

```rust
ui.add_space(6.0);
ui.separator();

ui.collapsing("Render Stats", |ui| {
    let s = &self.last_stats;

    egui::Grid::new("render_stats").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label("LOD");
        ui.label(format!("{}", s.lod));
        ui.end_row();

        ui.label("Features");
        ui.label(format!("{} / {} drawn", s.features_drawn, s.features_tried));
        ui.end_row();

        ui.label("Dropped");
        let drop_color = if s.features_dropped > 0 {
            egui::Color32::from_rgb(220, 80, 80)
        } else {
            ui.visuals().text_color()
        };
        ui.colored_label(drop_color, format!("{}", s.features_dropped));
        ui.end_row();

        ui.label("Points");
        ui.label(format!("{} / {} drawn", s.points_drawn, s.points_tried));
        ui.end_row();
    });

    ui.add_space(4.0);
    ui.label("Buffer utilization");

    // Span buffer bar
    let span_pct = s.span_utilization;
    ui.horizontal(|ui| {
        ui.label("Spans");
        let bar = egui::ProgressBar::new(span_pct)
            .text(format!("{:.0}%", span_pct * 100.0));
        ui.add(bar);
    });

    // Points buffer bar
    let pt_pct = s.point_utilization;
    ui.horizontal(|ui| {
        ui.label("Points");
        let bar = egui::ProgressBar::new(pt_pct)
            .text(format!("{:.0}%", pt_pct * 100.0));
        ui.add(bar);
    });

    // Rings buffer bar
    let ring_pct = s.ring_utilization;
    ui.horizontal(|ui| {
        ui.label("Rings");
        let bar = egui::ProgressBar::new(ring_pct)
            .text(format!("{:.0}%", ring_pct * 100.0));
        ui.add(bar);
    });
});
```

This gives a real-time view of how close the buffers are to saturation, making it easy to tune buffer sizes and priority assignments.

#### Also update headless stats line in [main.rs](file:///Users/timo/Documents/OSM/firmware/obc-sim/src/main.rs#L203)

```diff
-eprintln!("rendered {}/{} features (LOD {}) in {ms:.2} ms", stats.features_drawn, stats.features_tried, stats.lod);
+eprintln!("rendered {}/{} features (LOD {}, {} dropped) in {ms:.2} ms | spans {:.0}% points {:.0}% rings {:.0}%",
+    stats.features_drawn, stats.features_tried, stats.lod, stats.features_dropped,
+    stats.span_utilization * 100.0, stats.point_utilization * 100.0, stats.ring_utilization * 100.0);
```

---

## 6. Spec Update

### [MODIFY] [OBCM_Spec.md](file:///Users/timo/Documents/OSM/OBCM_Spec.md)

- Bump version references from v4 to v5
- Update the style record table to show the new 6-byte format with the Flags byte
- Document the priority bit semantics
- Update the style record size from 5 to 6 bytes everywhere

---

## File Change Summary

| File | Change |
|------|--------|
| [config.json](file:///Users/timo/Documents/OSM/config.json) | Add `"priority": true/false` to each feature type |
| [user_config.json](file:///Users/timo/Documents/OSM/user_config.json) | Same |
| [app.js](file:///Users/timo/Documents/OSM/packer/web_builder/static/app.js) | Priority checkbox column in feature table + buildConfigForSubmit |
| [style.css](file:///Users/timo/Documents/OSM/packer/web_builder/static/style.css) | Minor: column width for priority checkbox |
| [serialize.py](file:///Users/timo/Documents/OSM/obcm/serialize.py) | 6-byte style records with flags byte; version bump to v5 |
| [OBCM_Spec.md](file:///Users/timo/Documents/OSM/OBCM_Spec.md) | Document v5 changes |
| [reader.rs](file:///Users/timo/Documents/OSM/firmware/obc-reader/src/reader.rs) | Parse 6-byte styles, expose `priority` on `Style`, version check v5 |
| [lib.rs (render)](file:///Users/timo/Documents/OSM/firmware/obc-render/src/lib.rs) | Centralized buffer constants, two-pass collect, RenderStats with saturation |
| [gui.rs](file:///Users/timo/Documents/OSM/firmware/obc-sim/src/gui.rs) | Stats overlay panel with utilization bars |
| [main.rs](file:///Users/timo/Documents/OSM/firmware/obc-sim/src/main.rs) | Updated headless stats printout |

## Verification Plan

### Automated Tests
```bash
cd firmware && cargo test --workspace
```

Also run the existing Python tests if any:
```bash
cd /Users/timo/Documents/OSM && python -m pytest tests/
```

### Visual Verification

1. Rebuild the Freiburg map with the updated packer (new v5 format with priority flags)
2. Render at high zoom where buffer saturation previously caused artifacts
3. Compare before/after: land and roads should be intact; buildings/parks are the ones that get dropped
4. Check the stats panel shows utilization and dropped count
5. Toggle priority checkboxes in the webapp and rebuild to verify the flag flows end-to-end
