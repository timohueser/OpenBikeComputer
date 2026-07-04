// OBCM Web Builder frontend: region picker (Leaflet) + style editor + build panel.

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
let regions = [];                 // array of GeoJSON features (with cached bbox/area)
const regionsById = new Map();
const selected = new Set();        // selected region ids
let highlightLayer = null;         // Leaflet layer group for selected outlines

let config = null;                 // working copy of config (features tree)
const enabled = new Map();         // "cat/name" -> bool (include in build?)
let catalog = { keys: {} };        // curated OSM keys -> common values (autocomplete)
let dragRow = null;                // feature row currently being drag-reordered

let palette = [];                  // device color swatches [{hex, name?}, …]
let paletteColumns = 8;            // grid width for the palette picker

// Region select vs. bounding-box are mutually exclusive map modes.
let bboxMode = false;
let bboxRect = null;               // Leaflet rectangle layer for the drawn box
let bboxBounds = null;             // L.LatLngBounds of the drawn box (or null)
let bboxHandles = null;            // {nw,ne,se,sw} draggable corner markers
let bboxDrawing = false;           // mid-draw (dragging out a fresh box)
let bboxStart = null;
let drawArmed = false;             // "Draw box" clicked; next drag draws the box

// ---------------------------------------------------------------------------
// Map
// ---------------------------------------------------------------------------
const map = L.map("map", { worldCopyJump: true }).setView([30, 10], 2);
L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
  maxZoom: 19,
  attribution: "&copy; OpenStreetMap contributors",
}).addTo(map);

function bboxOf(feature) {
  let minx = Infinity, miny = Infinity, maxx = -Infinity, maxy = -Infinity;
  const scan = (coords) => {
    for (const c of coords) {
      if (typeof c[0] === "number") {
        if (c[0] < minx) minx = c[0];
        if (c[0] > maxx) maxx = c[0];
        if (c[1] < miny) miny = c[1];
        if (c[1] > maxy) maxy = c[1];
      } else {
        scan(c);
      }
    }
  };
  scan(feature.geometry.coordinates);
  return [minx, miny, maxx, maxy];
}

function pointInRing(x, y, ring) {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const xi = ring[i][0], yi = ring[i][1], xj = ring[j][0], yj = ring[j][1];
    if (((yi > y) !== (yj > y)) && (x < ((xj - xi) * (y - yi)) / (yj - yi) + xi)) {
      inside = !inside;
    }
  }
  return inside;
}

function pointInPolygon(x, y, polygon) {
  if (!pointInRing(x, y, polygon[0])) return false;
  for (let k = 1; k < polygon.length; k++) {
    if (pointInRing(x, y, polygon[k])) return false; // inside a hole
  }
  return true;
}

function featureContains(feature, lng, lat) {
  const [minx, miny, maxx, maxy] = feature._bbox;
  if (lng < minx || lng > maxx || lat < miny || lat > maxy) return false;
  const g = feature.geometry;
  if (g.type === "Polygon") return pointInPolygon(lng, lat, g.coordinates);
  if (g.type === "MultiPolygon") return g.coordinates.some((p) => pointInPolygon(lng, lat, p));
  return false;
}

async function loadRegions() {
  const fc = await fetch("/api/regions").then((r) => {
    if (!r.ok) throw new Error("Failed to load regions");
    return r.json();
  });
  regions = fc.features;
  for (const f of regions) {
    f._bbox = bboxOf(f);
    f._area = (f._bbox[2] - f._bbox[0]) * (f._bbox[3] - f._bbox[1]);
    regionsById.set(f.properties.id, f);
  }
}

map.on("click", (e) => {
  if (bboxMode) return; // drawing a box, not picking regions
  const { lng, lat } = e.latlng;
  const hits = regions
    .filter((f) => featureContains(f, lng, lat))
    .sort((a, b) => a._area - b._area); // most specific (smallest) first
  if (hits.length === 0) return;
  showRegionPopup(e.latlng, hits);
});

function showRegionPopup(latlng, hits) {
  popupOpen = true;
  clearPreview();
  const div = document.createElement("div");
  div.className = "region-popup";
  for (const f of hits) {
    const id = f.properties.id;
    const btn = document.createElement("button");
    btn.textContent = f.properties.name;
    if (selected.has(id)) btn.classList.add("selected");
    // Lock the preview to whichever option is hovered, instead of the raw
    // cursor position shooting "through" the menu.
    btn.addEventListener("mouseenter", () => setPreviewFeature(f));
    btn.onclick = () => {
      toggleRegion(id);
      btn.classList.toggle("selected");
    };
    div.appendChild(btn);
  }
  L.popup({ closeButton: true }).setLatLng(latlng).setContent(div).openOn(map);
  setPreviewFeature(hits[0]); // default to the smallest (top) option
}

function toggleRegion(id) {
  if (selected.has(id)) selected.delete(id);
  else selected.add(id);
  renderSelected();
  renderHighlights();
  clearPreview(); // selection changed under the cursor; recompute on next move
}

// ---------------------------------------------------------------------------
// Hover preview: outline the smallest region under the cursor in a distinct
// color so it's clearly a preview (not a selection).
// ---------------------------------------------------------------------------
const PREVIEW_STYLE = {
  color: "#ff9500", weight: 2, dashArray: "5,4",
  fillColor: "#ff9500", fillOpacity: 0.12, interactive: false,
};
let previewLayer = null;
let previewId = null;
let previewTip = L.tooltip({ className: "preview-tip", direction: "top", offset: [0, -6] });
let pendingLatLng = null;
let rafPending = false;
let popupOpen = false; // a region-level picker is open; freeze the cursor-follow preview

function smallestRegionAt(lng, lat) {
  let best = null;
  for (const f of regions) {
    if (!featureContains(f, lng, lat)) continue;
    if (!best || f._area < best._area) best = f; // most specific wins
  }
  return best;
}

function clearPreview() {
  previewId = null;
  if (previewLayer) { map.removeLayer(previewLayer); previewLayer = null; }
  if (map.hasLayer(previewTip)) map.removeLayer(previewTip);
}

// Draw the orange preview outline for a specific region. When `latlng` is
// given the name tooltip follows the cursor; otherwise no tooltip (the picker
// menu already labels each option).
function setPreviewFeature(f, latlng) {
  const id = f ? f.properties.id : null;
  if (id === previewId && !latlng) return;
  if (previewLayer) { map.removeLayer(previewLayer); previewLayer = null; }
  previewId = id;
  if (id) {
    previewLayer = L.geoJSON(f, { style: PREVIEW_STYLE }).addTo(map);
    if (latlng) previewTip.setContent(f.properties.name).setLatLng(latlng).addTo(map);
    else if (map.hasLayer(previewTip)) map.removeLayer(previewTip);
  } else if (map.hasLayer(previewTip)) {
    map.removeLayer(previewTip);
  }
}

function updatePreview() {
  rafPending = false;
  if (popupOpen || !pendingLatLng) return;
  const f = smallestRegionAt(pendingLatLng.lng, pendingLatLng.lat);
  // Don't preview a region that's already selected (it's shown in blue).
  const region = f && !selected.has(f.properties.id) ? f : null;
  setPreviewFeature(region, region ? pendingLatLng : null);
}

map.on("mousemove", (e) => {
  if (bboxMode || popupOpen) return; // box mode or picker open: no region preview
  pendingLatLng = e.latlng;
  if (map.hasLayer(previewTip)) previewTip.setLatLng(e.latlng); // tip follows cursor
  if (!rafPending) { rafPending = true; requestAnimationFrame(updatePreview); }
});
map.on("mouseout", () => { if (!popupOpen) { pendingLatLng = null; clearPreview(); } });
map.on("popupclose", () => { popupOpen = false; clearPreview(); });

// ---------------------------------------------------------------------------
// Bounding-box mode: an editable rectangle. "Draw box" arms a one-shot drag to
// draw it; afterwards the map pans normally and the box is fine-tuned by dragging
// its corner handles or its body. Mutually exclusive with region picking.
// ---------------------------------------------------------------------------
const BBOX_STYLE = {
  color: "#ff9500", weight: 2, fillColor: "#ff9500", fillOpacity: 0.08, className: "bbox-rect",
};
const CORNERS = ["nw", "ne", "se", "sw"];
const OPPOSITE = { nw: "se", ne: "sw", se: "nw", sw: "ne" };

function cornerLatLng(b, key) {
  return key === "nw" ? b.getNorthWest()
    : key === "ne" ? b.getNorthEast()
    : key === "se" ? b.getSouthEast()
    : b.getSouthWest();
}

// --- Draw a fresh box (only while armed, so plain drags still pan the map). ---
map.on("mousedown", (e) => {
  if (!drawArmed) return;
  bboxDrawing = true;
  bboxStart = e.latlng;
  removeBox();
  bboxRect = L.rectangle(L.latLngBounds(bboxStart, bboxStart), BBOX_STYLE).addTo(map);
});
map.on("mousemove", (e) => {
  if (!drawArmed || !bboxDrawing) return;
  bboxRect.setBounds(L.latLngBounds(bboxStart, e.latlng));
});
map.on("mouseup", () => {
  if (!drawArmed || !bboxDrawing) return;
  bboxDrawing = false;
  finishDraw();
});

function finishDraw() {
  drawArmed = false;
  map.dragging.enable();
  map.getContainer().classList.remove("bbox-cursor");
  const b = bboxRect.getBounds();
  // Ignore a stray click or micro-drag (no real area to build).
  const a = map.latLngToContainerPoint(b.getNorthWest());
  const c = map.latLngToContainerPoint(b.getSouthEast());
  if (Math.abs(a.x - c.x) < 5 || Math.abs(a.y - c.y) < 5) {
    removeBox();
    renderBboxInfo();
    return;
  }
  bboxBounds = b;
  buildHandles();
  enableBoxDrag();
  renderBboxInfo();
}

// Arm / cancel the one-shot draw. While armed, panning is off and the cursor is a
// crosshair; the next drag becomes the new box.
function armDraw() {
  drawArmed = true;
  removeHandles(); // don't let an old box's handles intercept the new draw
  map.dragging.disable();
  map.getContainer().classList.add("bbox-cursor");
  updateBboxButtons();
}
function cancelDraw() {
  if (!drawArmed) return;
  drawArmed = false;
  map.dragging.enable();
  map.getContainer().classList.remove("bbox-cursor");
  if (bboxBounds && bboxRect) buildHandles(); // restore handles on the kept box
  updateBboxButtons();
}

// Four draggable corner markers. Dragging one resizes the box about the opposite
// (fixed) corner; the adjacent handles follow. Leaflet marker-drag suppresses map
// panning for the duration, so corners and panning never fight.
function buildHandles() {
  removeHandles();
  bboxHandles = {};
  for (const key of CORNERS) {
    const m = L.marker(cornerLatLng(bboxBounds, key), {
      draggable: true,
      keyboard: false,
      zIndexOffset: 1000,
      icon: L.divIcon({ className: "bbox-handle", iconSize: [12, 12], iconAnchor: [6, 6] }),
    }).addTo(map);
    m.on("drag", () => {
      const opp = bboxHandles[OPPOSITE[key]].getLatLng();
      bboxBounds = L.latLngBounds(opp, m.getLatLng());
      bboxRect.setBounds(bboxBounds);
      for (const k of CORNERS) if (k !== key) bboxHandles[k].setLatLng(cornerLatLng(bboxBounds, k));
      scheduleBboxInfo();
    });
    m.on("dragend", () => { positionHandles(); renderBboxInfo(); });
    bboxHandles[key] = m;
  }
}
function positionHandles() {
  if (!bboxHandles || !bboxBounds) return;
  for (const key of CORNERS) bboxHandles[key].setLatLng(cornerLatLng(bboxBounds, key));
}
function removeHandles() {
  if (!bboxHandles) return;
  for (const key of CORNERS) map.removeLayer(bboxHandles[key]);
  bboxHandles = null;
}

// Drag the box body to move the whole box. Stop the mousedown so the map doesn't
// pan underneath, then translate the bounds by the cursor delta until release.
function enableBoxDrag() {
  bboxRect.on("mousedown", (e) => {
    if (drawArmed) return; // a redraw is in progress; let the draw handler run
    L.DomEvent.stop(e.originalEvent);
    map.dragging.disable();
    let last = e.latlng;
    const onMove = (ev) => {
      const dLat = ev.latlng.lat - last.lat;
      const dLng = ev.latlng.lng - last.lng;
      last = ev.latlng;
      bboxBounds = L.latLngBounds(
        [bboxBounds.getSouth() + dLat, bboxBounds.getWest() + dLng],
        [bboxBounds.getNorth() + dLat, bboxBounds.getEast() + dLng]
      );
      bboxRect.setBounds(bboxBounds);
      positionHandles();
      scheduleBboxInfo();
    };
    const onUp = () => {
      map.off("mousemove", onMove);
      map.off("mouseup", onUp);
      map.dragging.enable();
      renderBboxInfo();
    };
    map.on("mousemove", onMove);
    map.on("mouseup", onUp);
  });
}

function setMode(mode) {
  const toBbox = mode === "bbox";
  if (toBbox === bboxMode) return;
  bboxMode = toBbox;
  document.getElementById("mode-regions").classList.toggle("active", !toBbox);
  document.getElementById("mode-bbox").classList.toggle("active", toBbox);
  document.getElementById("regions-pane").hidden = toBbox;
  document.getElementById("bbox-pane").hidden = !toBbox;
  if (toBbox) {
    clearPreview();   // no region hover preview while in box mode
    renderBboxInfo(); // refresh the pane + buttons (panning stays enabled)
  } else {
    cancelDraw();     // un-arm if needed (re-enables dragging)
    clearBbox();      // remove any box + handles
  }
}

function clearBbox() {
  bboxDrawing = false;
  removeBox();
  renderBboxInfo();
}

function removeBox() {
  removeHandles();
  if (bboxRect) { map.removeLayer(bboxRect); bboxRect = null; }
  bboxBounds = null;
}

// Recompute coverage on every drag frame, but at most once per animation frame.
let bboxInfoRaf = false;
function scheduleBboxInfo() {
  if (bboxInfoRaf) return;
  bboxInfoRaf = true;
  requestAnimationFrame(() => { bboxInfoRaf = false; renderBboxInfo(); });
}

function updateBboxButtons() {
  const draw = document.getElementById("bbox-draw");
  const clear = document.getElementById("bbox-clear");
  if (draw) {
    draw.textContent = drawArmed ? "✕ Cancel" : bboxBounds ? "▢ Redraw" : "▢ Draw box";
    draw.classList.toggle("armed", drawArmed);
  }
  if (clear) clear.hidden = !bboxBounds;
}

// Approximate area of a lon/lat box in km² (good enough for a size hint).
function bboxAreaKm2(w, s, e, n) {
  const latMid = (((s + n) / 2) * Math.PI) / 180;
  const area = Math.abs(n - s) * 110.574 * Math.abs(e - w) * 111.32 * Math.cos(latMid);
  if (area >= 1000) return Math.round(area).toLocaleString() + " km²";
  return area.toFixed(area < 10 ? 1 : 0) + " km²";
}

function renderBboxInfo() {
  const info = document.getElementById("bbox-info");
  if (!info) return;
  if (!bboxBounds) {
    info.textContent = "No box yet — click “Draw box”, then drag on the map.";
    updateBboxButtons();
    return;
  }
  const w = bboxBounds.getWest(), s = bboxBounds.getSouth();
  const e = bboxBounds.getEast(), n = bboxBounds.getNorth();
  const regs = regionsForBbox([w, s, e, n]);
  const cover = regs.length
    ? "source: " + regs.map((r) => r.properties.name).join(", ")
    : "⚠ no downloadable region covers this area";
  info.innerHTML =
    `<div class="bbox-coords">W ${w.toFixed(3)} · S ${s.toFixed(3)} · E ${e.toFixed(3)} · N ${n.toFixed(3)}</div>` +
    `<div class="muted">~${bboxAreaKm2(w, s, e, n)} · ${cover}</div>`;
  updateBboxButtons();
}

// Smallest *leaf* (most specific, downloadable) region at a point. Coarse parent
// regions (continents, countries split into sub-regions) are skipped: their
// children tile the same land, and unlike a parent's simplified outline a leaf
// doesn't sprawl across the surrounding sea — so a point out at sea matches no
// leaf and is correctly treated as water rather than dragging in a continent PBF.
function smallestLeafAt(lng, lat) {
  let best = null;
  for (const f of regions) {
    if (f.properties.has_children) continue;
    if (!featureContains(f, lng, lat)) continue;
    if (!best || f._area < best._area) best = f;
  }
  return best;
}

// Pick the Geofabrik regions whose PBFs cover a drawn box: union the smallest leaf
// region under a grid of sample points. A box inside one region yields just that
// region; a box spanning a border yields the few leaf regions it touches — always
// the minimal download. Sea points match nothing and are skipped. Only if the box
// samples no land at all (e.g. a thin offshore strip) do we fall back to the
// smallest leaf whose bbox overlaps it.
function regionsForBbox(bbox) {
  const [w, s, e, n] = bbox;
  const N = 6; // 6x6 grid; dense enough to catch small regions inside the box
  const set = new Map();
  for (let i = 0; i < N; i++) {
    for (let j = 0; j < N; j++) {
      const x = w + ((e - w) * i) / (N - 1);
      const y = s + ((n - s) * j) / (N - 1);
      const f = smallestLeafAt(x, y);
      if (f) set.set(f.properties.id, f);
    }
  }
  if (set.size) return [...set.values()];

  // No sampled point hit land — grab the smallest leaf whose bbox overlaps the box.
  const overlaps = regions
    .filter((f) => !f.properties.has_children)
    .filter((f) => {
      const b = f._bbox;
      return b[0] <= e && b[2] >= w && b[1] <= n && b[3] >= s;
    })
    .sort((a, b) => a._area - b._area);
  return overlaps.length ? [overlaps[0]] : [];
}

function renderHighlights() {
  if (highlightLayer) map.removeLayer(highlightLayer);
  const feats = [...selected].map((id) => regionsById.get(id)).filter(Boolean);
  highlightLayer = L.geoJSON(feats, {
    style: { color: "#4a9eff", weight: 2, fillColor: "#4a9eff", fillOpacity: 0.25 },
  }).addTo(map);
}

function renderSelected() {
  const box = document.getElementById("selected-regions");
  box.innerHTML = "";
  if (selected.size === 0) {
    box.innerHTML = '<span class="muted">No regions selected. Click the map or search above.</span>';
    return;
  }
  for (const id of selected) {
    const f = regionsById.get(id);
    const chip = document.createElement("span");
    chip.className = "chip";
    chip.textContent = f ? f.properties.name : id;
    const x = document.createElement("button");
    x.textContent = "×";
    x.title = "Remove";
    x.onclick = () => toggleRegion(id);
    chip.appendChild(x);
    box.appendChild(chip);
  }
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------
const searchInput = document.getElementById("region-search");
const searchResults = document.getElementById("search-results");

searchInput.addEventListener("input", () => {
  const q = searchInput.value.trim().toLowerCase();
  searchResults.innerHTML = "";
  if (q.length < 2) return;
  const matches = regions
    .filter((f) => f.properties.name.toLowerCase().includes(q))
    .slice(0, 30);
  for (const f of matches) {
    const id = f.properties.id;
    const item = document.createElement("div");
    item.className = "search-item";
    const sel = selected.has(id) ? "✓ " : "";
    item.innerHTML = `<span>${sel}${f.properties.name}</span><span class="pid">${id}</span>`;
    item.onclick = () => {
      if (!selected.has(id)) toggleRegion(id);
      const [minx, miny, maxx, maxy] = f._bbox;
      map.fitBounds([[miny, minx], [maxy, maxx]], { maxZoom: 8 });
      searchInput.value = "";
      searchResults.innerHTML = "";
    };
    searchResults.appendChild(item);
  }
});

// ---------------------------------------------------------------------------
// RGB565 <-> hex helpers
// ---------------------------------------------------------------------------
function rgb565ToHex(str) {
  const v = parseInt(str, 16);
  const r5 = (v >> 11) & 0x1f, g6 = (v >> 5) & 0x3f, b5 = v & 0x1f;
  const r = Math.round((r5 * 255) / 31);
  const g = Math.round((g6 * 255) / 63);
  const b = Math.round((b5 * 255) / 31);
  return "#" + [r, g, b].map((n) => n.toString(16).padStart(2, "0")).join("");
}

function hexToRgb565(hex) {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  const v = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3);
  return "0x" + v.toString(16).toUpperCase().padStart(4, "0");
}

// Quantize an RGB565 value to the device's 64-color RGB222 gamut and return it
// as a CSS hex — i.e. exactly what the panel will display (mirrors the firmware's
// `rgb565_to_device64`). Swatches show this, so the UI never promises a color
// the device can't render.
function rgb565ToDeviceHex(str) {
  const v = parseInt(str, 16);
  const r5 = (v >> 11) & 0x1f, g6 = (v >> 5) & 0x3f, b5 = v & 0x1f;
  const r8 = (r5 << 3) | (r5 >> 2);
  const g8 = (g6 << 2) | (g6 >> 4);
  const b8 = (b5 << 3) | (b5 >> 2);
  const q = (x) => (x >> 6) * 85; // keep top 2 bits, expand (step = 85)
  return "#" + [q(r8), q(g8), q(b8)].map((n) => n.toString(16).padStart(2, "0")).join("");
}

// ---------------------------------------------------------------------------
// Color picker: a swatch button + RGB565 label. Clicking opens a popover with
// the device's 64-color palette (default) plus the OS picker for custom colors.
// ---------------------------------------------------------------------------
async function loadPalette() {
  try {
    const p = await fetch("/api/palette").then((r) => (r.ok ? r.json() : null));
    if (p && Array.isArray(p.colors)) {
      palette = p.colors.map((c) => (typeof c === "string" ? { hex: c } : c));
      if (Number.isFinite(p.columns) && p.columns > 0) paletteColumns = p.columns;
    }
  } catch (_) { /* non-fatal: the popover falls back to the custom picker */ }
}

// A swatch + label bound to one RGB565 value. `onChange(newRgb565)` fires on every
// pick. Returns the wrapper element.
function createColorControl(rgb565, onChange) {
  const wrap = document.createElement("span");
  wrap.className = "color-control";
  const swatch = document.createElement("button");
  swatch.type = "button";
  swatch.className = "color-swatch";
  swatch.title = "Pick a color";
  const label = document.createElement("span");
  label.className = "rgb565";
  let value = rgb565;
  const apply = (v) => {
    value = v;
    swatch.style.background = rgb565ToDeviceHex(v);
    label.textContent = v;
  };
  apply(value);
  swatch.onclick = () =>
    openColorPopover(swatch, value, (v) => { apply(v); onChange(v); });
  wrap.appendChild(swatch);
  wrap.appendChild(label);
  return wrap;
}

let activePopover = null;
let popoverCleanup = null;

function closeColorPopover() {
  if (popoverCleanup) { popoverCleanup(); popoverCleanup = null; }
  if (activePopover) { activePopover.remove(); activePopover = null; }
}

function positionPopover(pop, anchor) {
  const r = anchor.getBoundingClientRect();
  const pw = pop.offsetWidth, ph = pop.offsetHeight;
  let left = Math.min(r.left, window.innerWidth - 8 - pw);
  left = Math.max(8, left);
  let top = r.bottom + 6;
  if (top + ph > window.innerHeight - 8) top = r.top - 6 - ph; // flip above
  top = Math.max(8, top);
  pop.style.left = left + "px";
  pop.style.top = top + "px";
}

function openColorPopover(anchorEl, currentRgb565, onPick) {
  closeColorPopover();
  const pop = document.createElement("div");
  pop.className = "color-popover";

  const title = document.createElement("div");
  title.className = "popover-title";
  title.textContent = "Device palette";
  pop.appendChild(title);

  const grid = document.createElement("div");
  grid.className = "palette-grid";
  grid.style.gridTemplateColumns = `repeat(${paletteColumns}, 1fr)`;
  const curDev = rgb565ToDeviceHex(currentRgb565).toUpperCase();
  for (const c of palette) {
    const rgb = hexToRgb565(c.hex);
    const cell = document.createElement("button");
    cell.type = "button";
    cell.className = "palette-cell";
    cell.style.background = c.hex;
    cell.title = `${c.name ? c.name + " · " : ""}${c.hex} · ${rgb}`;
    if (c.hex.toUpperCase() === curDev) cell.classList.add("current");
    cell.onclick = () => { onPick(rgb); closeColorPopover(); };
    grid.appendChild(cell);
  }
  pop.appendChild(grid);

  // Custom color: the OS picker, always available. The device quantizes it, so
  // show a small preview of the actual on-device color next to it.
  const custom = document.createElement("div");
  custom.className = "popover-custom";
  const clab = document.createElement("span");
  clab.className = "muted small";
  clab.textContent = "Custom";
  const native = document.createElement("input");
  native.type = "color";
  native.value = rgb565ToHex(currentRgb565);
  const prev = document.createElement("span");
  prev.className = "device-preview";
  prev.title = "How the device will show this color";
  prev.style.background = rgb565ToDeviceHex(currentRgb565);
  native.oninput = () => {
    const v = hexToRgb565(native.value);
    prev.style.background = rgb565ToDeviceHex(v);
    onPick(v); // live-apply; popover stays open for tweaking
  };
  custom.appendChild(clab);
  custom.appendChild(native);
  const arrow = document.createElement("span");
  arrow.className = "muted small";
  arrow.textContent = "→ device";
  custom.appendChild(arrow);
  custom.appendChild(prev);
  pop.appendChild(custom);

  document.body.appendChild(pop);
  positionPopover(pop, anchorEl);
  activePopover = pop;

  const onDocDown = (ev) => {
    if (!pop.contains(ev.target) && !anchorEl.contains(ev.target)) closeColorPopover();
  };
  const onKey = (ev) => { if (ev.key === "Escape") closeColorPopover(); };
  const sidePanel = document.getElementById("side-panel");
  // The popover is position:fixed, so re-anchor isn't free — just close on scroll/resize.
  setTimeout(() => document.addEventListener("mousedown", onDocDown), 0);
  document.addEventListener("keydown", onKey);
  if (sidePanel) sidePanel.addEventListener("scroll", closeColorPopover, { passive: true });
  window.addEventListener("resize", closeColorPopover);
  popoverCleanup = () => {
    document.removeEventListener("mousedown", onDocDown);
    document.removeEventListener("keydown", onKey);
    if (sidePanel) sidePanel.removeEventListener("scroll", closeColorPopover);
    window.removeEventListener("resize", closeColorPopover);
  };
}

// ---------------------------------------------------------------------------
// Style editor
// ---------------------------------------------------------------------------
async function loadConfig() {
  // Load the OSM autocomplete catalog (non-fatal if missing) and the active
  // config (user edits if persisted, else factory defaults) in parallel.
  const [cat, cfg] = await Promise.all([
    fetch("/static/osm_catalog.json").then((r) => (r.ok ? r.json() : { keys: {} })).catch(() => ({ keys: {} })),
    fetch("/api/config").then((r) => r.json()),
  ]);
  catalog = cat && cat.keys ? cat : { keys: {} };
  populateKeyDatalist();
  applyConfig(cfg);
}

// Adopt a loaded config object (from the server, a reset, or an imported
// stylesheet) as the working state and re-render the editors.
function applyConfig(cfg) {
  config = cfg;
  if (!Array.isArray(config.lods) || config.lods.length === 0) {
    config.lods = [{ max_mpp: null, simplify: 0 }];
  }
  config.lods[0].max_mpp = null; // coarsest is always +inf
  config.features = config.features || {};
  // User-position marker: a single global color (RGB565). Default bright red.
  config.marker = config.marker || { color: "0xF800" };
  // A stylesheet may carry a `disabled` list of "cat/name" keys; everything
  // else defaults to enabled. Strip it from the working tree.
  const disabled = new Set(Array.isArray(config.disabled) ? config.disabled : []);
  delete config.disabled;
  enabled.clear();
  for (const cat of Object.keys(config.features)) {
    for (const name of Object.keys(config.features[cat])) {
      const def = config.features[cat][name];
      if (typeof def.min_lod !== "number") def.min_lod = config.lods.length - 1;
      def.min_lod = clampLod(def.min_lod);
      enabled.set(`${cat}/${name}`, !disabled.has(`${cat}/${name}`));
    }
  }
  renderLodEditor();
  renderMarkerEditor();
  renderStyleEditor();
}

// The working config plus the list of disabled features, used both for
// autosave (-> user_config.json) and for exported stylesheets.
function serializeWorkingConfig() {
  const disabled = [];
  for (const [key, on] of enabled) if (on === false) disabled.push(key);
  const out = { lods: config.lods, features: config.features, marker: config.marker };
  if (disabled.length) out.disabled = disabled;
  return out;
}

// ---------------------------------------------------------------------------
// Autosave: persist edits to user_config.json (debounced).
// ---------------------------------------------------------------------------
let saveTimer = null;

function setSaveStatus(text, cls) {
  const el = document.getElementById("save-status");
  if (!el) return;
  el.textContent = text;
  el.className = "save-status small " + (cls || "muted");
}

function scheduleSave() {
  if (!config) return;
  setSaveStatus("Saving…", "muted");
  clearTimeout(saveTimer);
  saveTimer = setTimeout(saveConfig, 500);
}

async function saveConfig() {
  try {
    const res = await fetch("/api/config", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(serializeWorkingConfig()),
    });
    if (!res.ok) throw new Error(await res.text());
    setSaveStatus("All changes saved", "muted");
  } catch (e) {
    setSaveStatus("Save failed: " + e.message, "err-text");
  }
}

// Populate the shared <datalist> of OSM keys used by the "add category" input.
function populateKeyDatalist() {
  const dl = document.getElementById("osm-keys");
  if (!dl) return;
  dl.innerHTML = "";
  for (const key of Object.keys(catalog.keys || {})) {
    const opt = document.createElement("option");
    opt.value = key;
    dl.appendChild(opt);
  }
}

// ---------------------------------------------------------------------------
// Levels of detail
// ---------------------------------------------------------------------------
function lodCount() {
  return config.lods.length;
}

function clampLod(i) {
  return Math.max(0, Math.min(lodCount() - 1, i | 0));
}

function renderLodEditor() {
  const root = document.getElementById("lod-editor");
  root.innerHTML = "";
  const table = document.createElement("table");
  table.className = "lod-table";
  table.innerHTML =
    "<thead><tr><th>tier</th><th>max&nbsp;m/px</th><th>simplify&nbsp;(m)</th><th></th></tr></thead>";
  const tbody = document.createElement("tbody");
  const n = lodCount();
  config.lods.forEach((lod, i) => {
    const tr = document.createElement("tr");

    const tdName = document.createElement("td");
    tdName.className = "lod-name";
    let tag = "";
    if (i === 0) tag = ' <span class="muted small">coarsest</span>';
    else if (i === n - 1) tag = ' <span class="muted small">finest</span>';
    tdName.innerHTML = `LOD ${i}${tag}`;

    const tdMpp = document.createElement("td");
    if (i === 0) {
      lod.max_mpp = null;
      const inf = document.createElement("span");
      inf.className = "inf";
      inf.textContent = "∞";
      inf.title = "Coarsest tier — drawn when fully zoomed out";
      tdMpp.appendChild(inf);
    } else {
      tdMpp.appendChild(floatInput(lod.max_mpp, (v) => (lod.max_mpp = v)));
    }

    const tdSimp = document.createElement("td");
    tdSimp.appendChild(floatInput(lod.simplify, (v) => (lod.simplify = v)));

    const tdDel = document.createElement("td");
    if (n > 1) {
      const del = document.createElement("button");
      del.className = "add-feat";
      del.textContent = "×";
      del.title = "Remove this tier";
      del.onclick = () => removeLod(i);
      tdDel.appendChild(del);
    }

    for (const td of [tdName, tdMpp, tdSimp, tdDel]) tr.appendChild(td);
    tbody.appendChild(tr);
  });
  table.appendChild(tbody);
  root.appendChild(table);
}

// User-position marker editor: a single color picker (shape/size are fixed in
// the renderer). Mirrors the per-feature color control — an <input type="color">
// alongside the raw RGB565 value.
function renderMarkerEditor() {
  const root = document.getElementById("marker-editor");
  if (!root) return;
  root.innerHTML = "";
  if (!config.marker) config.marker = { color: "0xF800" };

  const row = document.createElement("div");
  row.className = "marker-row";

  const lab = document.createElement("label");
  lab.textContent = "Color";

  const ctrl = createColorControl(config.marker.color, (v) => {
    config.marker.color = v;
    scheduleSave();
  });

  row.appendChild(lab);
  row.appendChild(ctrl);
  root.appendChild(row);
}

function addLod() {
  const last = config.lods[config.lods.length - 1];
  const prev = last.max_mpp != null ? last.max_mpp : 120;
  config.lods.push({ max_mpp: Math.max(1, Math.round(prev / 2)), simplify: 0 });
  renderLodEditor();
  renderStyleEditor(); // pickers gain a segment
  scheduleSave();
}

function removeLod(k) {
  if (config.lods.length <= 1) return;
  config.lods.splice(k, 1);
  config.lods[0].max_mpp = null; // whatever is now coarsest is +inf
  // Remap feature start-tiers: levels above the removed one shift down by one;
  // the removed level collapses into the tier that took its index.
  const n = config.lods.length;
  for (const cat of Object.keys(config.features)) {
    for (const name of Object.keys(config.features[cat])) {
      const def = config.features[cat][name];
      let m = typeof def.min_lod === "number" ? def.min_lod : 0;
      if (m > k) m -= 1;
      def.min_lod = Math.max(0, Math.min(n - 1, m));
    }
  }
  renderLodEditor();
  renderStyleEditor();
  scheduleSave();
}

function floatInput(value, onChange) {
  const i = document.createElement("input");
  i.type = "number";
  i.min = "0";
  i.value = value == null ? "" : value;
  i.oninput = () => {
    const v = parseFloat(i.value);
    onChange(Number.isFinite(v) ? v : 0);
    scheduleSave();
  };
  return i;
}

function buildLodPicker(def) {
  const n = lodCount();
  if (typeof def.min_lod !== "number") def.min_lod = n - 1;
  def.min_lod = clampLod(def.min_lod);
  const wrap = document.createElement("div");
  wrap.className = "lod-picker";
  for (let i = 0; i < n; i++) {
    const seg = document.createElement("button");
    seg.type = "button";
    seg.className = "lod-seg" + (i >= def.min_lod ? " on" : "");
    seg.textContent = i;
    let where = i === 0 ? " (coarsest)" : i === n - 1 ? " (finest)" : "";
    seg.title = `Show from LOD ${i}${where} and every finer tier`;
    seg.onclick = () => {
      def.min_lod = i;
      [...wrap.children].forEach((c, idx) => c.classList.toggle("on", idx >= i));
      scheduleSave();
    };
    wrap.appendChild(seg);
  }
  return wrap;
}

function renderStyleEditor() {
  const root = document.getElementById("style-editor");
  root.innerHTML = "";
  for (const cat of Object.keys(config.features)) {
    const group = document.createElement("details");
    group.className = "feat-group";
    group.open = true;
    const entries = config.features[cat];
    const summary = document.createElement("summary");
    const label = document.createElement("span");
    label.innerHTML = `${cat} <span class="count">(${Object.keys(entries).length})</span>`;
    summary.appendChild(label);
    const delCat = document.createElement("button");
    delCat.className = "del-cat";
    delCat.textContent = "× category";
    delCat.title = `Remove the "${cat}" category and all its types`;
    delCat.onclick = (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (!confirm(`Remove the "${cat}" category and all ${Object.keys(entries).length} of its types?`)) return;
      for (const name of Object.keys(entries)) enabled.delete(`${cat}/${name}`);
      delete config.features[cat];
      renderStyleEditor();
      scheduleSave();
    };
    summary.appendChild(delCat);
    group.appendChild(summary);

    const table = document.createElement("table");
    table.className = "feat-table";
    table.innerHTML =
      "<thead><tr><th></th><th></th><th title=\"Priority: 1 (highest) to 4 (lowest)\">prio</th><th>type</th><th>LODs</th><th>color</th><th>z</th><th>w</th><th></th></tr></thead>";
    const tbody = document.createElement("tbody");
    for (const name of Object.keys(entries)) {
      tbody.appendChild(buildRow(cat, name, entries[name]));
    }
    table.appendChild(tbody);
    group.appendChild(table);

    // Per-category datalist of common OSM values for the "add type" field.
    const dl = document.createElement("datalist");
    dl.id = valueListId(cat);
    group.appendChild(dl);

    const add = document.createElement("button");
    add.className = "add-feat";
    add.textContent = "+ add type";
    add.onclick = () => addFeature(cat, tbody, add);
    group.appendChild(add);

    root.appendChild(group);
  }
}

function valueListId(cat) {
  return "dl-vals-" + cat.replace(/[^a-zA-Z0-9_-]/g, "_");
}

// Refresh a category's value datalist to the catalog values not yet used.
function refreshValueList(cat) {
  const dl = document.getElementById(valueListId(cat));
  if (!dl) return;
  dl.innerHTML = "";
  const used = new Set(Object.keys(config.features[cat] || {}));
  for (const v of catalog.keys[cat] || []) {
    if (used.has(v)) continue;
    const opt = document.createElement("option");
    opt.value = v;
    dl.appendChild(opt);
  }
}

function buildRow(cat, name, def) {
  const key = `${cat}/${name}`;
  const tr = document.createElement("tr");
  tr.dataset.cat = cat;
  tr.dataset.name = name;

  // Drag handle: rows reorder within their category (order is preserved in the
  // stylesheet and drives style-ID assignment). The row is only draggable while
  // the handle is held, so the inputs stay usable.
  const tdHandle = document.createElement("td");
  const handle = document.createElement("span");
  handle.className = "drag-handle";
  handle.textContent = "⋮⋮";
  handle.title = "Drag to reorder";
  handle.onmousedown = () => { tr.draggable = true; };
  tdHandle.appendChild(handle);
  tr.addEventListener("dragstart", (e) => {
    dragRow = tr;
    tr.classList.add("dragging");
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", name);
  });
  tr.addEventListener("dragover", (e) => {
    if (!dragRow || dragRow === tr || dragRow.dataset.cat !== tr.dataset.cat) return;
    e.preventDefault();
    const rect = tr.getBoundingClientRect();
    const before = e.clientY - rect.top < rect.height / 2;
    tr.parentNode.insertBefore(dragRow, before ? tr : tr.nextSibling);
  });
  tr.addEventListener("dragend", () => {
    tr.draggable = false;
    tr.classList.remove("dragging");
    commitRowOrder(cat, tr.parentNode);
    dragRow = null;
  });

  const tdToggle = document.createElement("td");
  const cb = document.createElement("input");
  cb.type = "checkbox";
  cb.checked = enabled.get(key) !== false;
  cb.onchange = () => {
    enabled.set(key, cb.checked);
    tr.classList.toggle("feat-off", !cb.checked);
    scheduleSave();
  };
  tdToggle.appendChild(cb);

  const tdPrio = document.createElement("td");
  const prioSel = document.createElement("select");
  prioSel.className = "prio-select";
  prioSel.title = "Priority level: 1 (highest) to 4 (lowest)";
  for (let i = 1; i <= 4; i++) {
    const opt = document.createElement("option");
    opt.value = i;
    opt.textContent = i;
    if ((def.priority || 3) === i) {
      opt.selected = true;
    }
    prioSel.appendChild(opt);
  }
  prioSel.onchange = () => {
    def.priority = parseInt(prioSel.value, 10);
    scheduleSave();
  };
  tdPrio.appendChild(prioSel);

  const tdName = document.createElement("td");
  tdName.className = "feat-name";
  tdName.textContent = name;

  const tdColor = document.createElement("td");
  tdColor.appendChild(createColorControl(def.color, (v) => {
    def.color = v;
    scheduleSave();
  }));

  const tdZ = document.createElement("td");
  tdZ.appendChild(numInput(def.z_index, (v) => (def.z_index = v)));

  const tdW = document.createElement("td");
  tdW.appendChild(numInput(def.weight, (v) => (def.weight = v)));

  const tdLod = document.createElement("td");
  tdLod.appendChild(buildLodPicker(def));

  const tdDel = document.createElement("td");
  const del = document.createElement("button");
  del.className = "add-feat";
  del.textContent = "×";
  del.title = "Remove type";
  del.onclick = () => {
    delete config.features[cat][name];
    enabled.delete(key);
    tr.remove();
    scheduleSave();
  };
  tdDel.appendChild(del);

  if (!cb.checked) tr.classList.add("feat-off");
  for (const td of [tdHandle, tdToggle, tdPrio, tdName, tdLod, tdColor, tdZ, tdW, tdDel]) tr.appendChild(td);
  return tr;
}

// Rebuild config.features[cat] in the current DOM row order, then persist.
function commitRowOrder(cat, tbody) {
  const names = [...tbody.querySelectorAll("tr[data-name]")].map((r) => r.dataset.name);
  const entries = config.features[cat];
  const reordered = {};
  for (const name of names) {
    if (name in entries) reordered[name] = entries[name];
  }
  config.features[cat] = reordered;
  scheduleSave();
}

function numInput(value, onChange) {
  const i = document.createElement("input");
  i.type = "number";
  i.value = value;
  i.oninput = () => {
    onChange(parseInt(i.value, 10) || 0);
    scheduleSave();
  };
  return i;
}

// Inline "add type" row: a text input backed by the category's value datalist
// so common OSM values autocomplete, while any freeform value is still allowed.
function addFeature(cat, tbody, addBtn) {
  refreshValueList(cat);
  const tr = document.createElement("tr");
  tr.className = "feat-add-row";
  const td = document.createElement("td");
  td.colSpan = 9;
  const input = document.createElement("input");
  input.type = "text";
  input.className = "feat-add-input";
  input.placeholder = `OSM ${cat} value (e.g. "steps")…`;
  input.setAttribute("list", valueListId(cat));
  td.appendChild(input);
  tr.appendChild(td);
  tbody.appendChild(tr);
  if (addBtn) addBtn.disabled = true;
  input.focus();

  const cleanup = () => {
    tr.remove();
    if (addBtn) addBtn.disabled = false;
  };
  const commit = () => {
    const name = input.value.trim();
    if (!name) return cleanup();
    if (config.features[cat][name]) {
      input.classList.add("dupe");
      input.title = "That type already exists.";
      return;
    }
    const def = { z_index: 10, color: "0xFFFF", weight: 1, min_lod: lodCount() - 1, priority: 3 };
    config.features[cat][name] = def;
    enabled.set(`${cat}/${name}`, true);
    cleanup();
    tbody.appendChild(buildRow(cat, name, def));
    scheduleSave();
  };
  input.oninput = () => { input.classList.remove("dupe"); input.title = ""; };
  input.onkeydown = (e) => {
    if (e.key === "Enter") { e.preventDefault(); commit(); }
    else if (e.key === "Escape") { e.preventDefault(); cleanup(); }
  };
  input.onblur = commit;
}

// Add a new top-level category (OSM tag key) via an autocompleted prompt-row at
// the bottom of the style editor.
function addCategory() {
  const root = document.getElementById("style-editor");
  if (document.getElementById("cat-add-row")) return; // already adding
  const row = document.createElement("div");
  row.id = "cat-add-row";
  row.className = "cat-add-row";
  const input = document.createElement("input");
  input.type = "text";
  input.className = "feat-add-input";
  input.placeholder = 'OSM tag key (e.g. "railway")…';
  input.setAttribute("list", "osm-keys");
  row.appendChild(input);
  root.appendChild(row);
  input.focus();

  const cleanup = () => row.remove();
  const commit = () => {
    const key = input.value.trim();
    if (!key) return cleanup();
    if (config.features[key]) {
      input.classList.add("dupe");
      input.title = "That category already exists.";
      return;
    }
    config.features[key] = {};
    cleanup();
    renderStyleEditor();
    scheduleSave();
  };
  input.oninput = () => { input.classList.remove("dupe"); input.title = ""; };
  input.onkeydown = (e) => {
    if (e.key === "Enter") { e.preventDefault(); commit(); }
    else if (e.key === "Escape") { e.preventDefault(); cleanup(); }
  };
  input.onblur = commit;
}

function buildConfigForSubmit() {
  const n = lodCount();
  const lods = config.lods.map((l, i) => ({
    max_mpp: i === 0 ? null : (l.max_mpp != null ? l.max_mpp : null),
    simplify: l.simplify || 0,
  }));
  const out = { lods, features: {}, marker: config.marker };
  for (const cat of Object.keys(config.features)) {
    for (const name of Object.keys(config.features[cat])) {
      if (enabled.get(`${cat}/${name}`) === false) continue;
      const def = config.features[cat][name];
      const min_lod = Math.max(0, Math.min(n - 1, def.min_lod | 0));
      out.features[cat] = out.features[cat] || {};
      out.features[cat][name] = { ...def, min_lod, priority: def.priority || 3 };
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Build / jobs
// ---------------------------------------------------------------------------
document.getElementById("add-lod").addEventListener("click", addLod);
document.getElementById("add-category").addEventListener("click", addCategory);
document.getElementById("mode-regions").addEventListener("click", () => setMode("regions"));
document.getElementById("mode-bbox").addEventListener("click", () => setMode("bbox"));
document.getElementById("bbox-draw").addEventListener("click", () => (drawArmed ? cancelDraw() : armDraw()));
document.getElementById("bbox-clear").addEventListener("click", clearBbox);

// ---------------------------------------------------------------------------
// Stylesheet export / import (share configs independently of any .obcm)
// ---------------------------------------------------------------------------
document.getElementById("export-style").addEventListener("click", () => {
  const blob = new Blob([JSON.stringify(serializeWorkingConfig(), null, 2)], {
    type: "application/json",
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = "stylesheet.json";
  a.click();
  URL.revokeObjectURL(url);
});

const importFile = document.getElementById("import-file");
document.getElementById("import-style").addEventListener("click", () => importFile.click());
importFile.addEventListener("change", async () => {
  const file = importFile.files[0];
  importFile.value = ""; // allow re-importing the same file
  if (!file) return;
  try {
    const parsed = JSON.parse(await file.text());
    if (!parsed || typeof parsed.features !== "object" || Array.isArray(parsed.features)) {
      throw new Error("not a valid stylesheet (missing a features object)");
    }
    applyConfig(parsed);
    setSaveStatus("Imported " + file.name, "muted");
    scheduleSave(); // persist the imported stylesheet as the user's config
  } catch (e) {
    setSaveStatus("Import failed: " + e.message, "err-text");
  }
});

document.getElementById("reset-style").addEventListener("click", async () => {
  if (!confirm("Discard your edits and restore the factory defaults?")) return;
  try {
    const factory = await fetch("/api/config", { method: "DELETE" }).then((r) => r.json());
    applyConfig(factory);
    setSaveStatus("Restored factory defaults", "muted");
  } catch (e) {
    setSaveStatus("Restore failed: " + e.message, "err-text");
  }
});

const buildBtn = document.getElementById("build-btn");
const buildStatus = document.getElementById("build-status");
const progressWrap = document.getElementById("progress-wrap");
const progressFill = document.getElementById("progress-fill");
const progressLabel = document.getElementById("progress-label");
const logEl = document.getElementById("log");

const PHASES = ["downloading", "cropping", "merging", "ingest", "bbox", "land", "quadtree", "serialize"];
let transientLine = null; // last tqdm-style line element

buildBtn.addEventListener("click", async () => {
  let regionIds, bbox = null;
  if (bboxMode) {
    if (!bboxBounds) {
      setStatus("Draw a bounding box on the map first.", "err");
      return;
    }
    bbox = [bboxBounds.getWest(), bboxBounds.getSouth(), bboxBounds.getEast(), bboxBounds.getNorth()];
    const regs = regionsForBbox(bbox);
    if (regs.length === 0) {
      setStatus("No downloadable region covers that box — draw it over land within a known region.", "err");
      return;
    }
    regionIds = regs.map((r) => r.properties.id);
  } else {
    if (selected.size === 0) {
      setStatus("Select at least one region first.", "err");
      return;
    }
    regionIds = [...selected];
  }
  const body = {
    region_ids: regionIds,
    config: buildConfigForSubmit(),
    chunk_size: parseInt(document.getElementById("chunk-size").value, 10) || 4096,
    output_name: document.getElementById("output-name").value.trim() || "output.obcm",
  };
  if (bbox) body.bbox = bbox;

  buildBtn.disabled = true;
  setStatus("Starting…", "");
  logEl.hidden = false;
  logEl.textContent = "";
  transientLine = null;
  progressWrap.hidden = false;
  setProgress(0, "queued");

  let res;
  try {
    res = await fetch("/api/jobs", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch (e) {
    finish(false, "Network error: " + e.message);
    return;
  }
  if (!res.ok) {
    finish(false, "Server rejected job: " + (await res.text()));
    return;
  }
  const { job_id } = await res.json();
  followJob(job_id);
});

function followJob(jobId) {
  const es = new EventSource(`/api/jobs/${jobId}/events`);
  es.onmessage = (msg) => {
    const ev = JSON.parse(msg.data);
    handleEvent(ev);
    if (ev.type === "done" || ev.type === "error") es.close();
  };
  es.onerror = () => {
    // Stream ends after the job finishes; only flag if we never finished.
    if (buildBtn.disabled) {
      es.close();
      finish(false, "Connection to job lost.");
    }
  };
}

function handleEvent(ev) {
  if (ev.type === "status") {
    const i = PHASES.indexOf(ev.detail);
    const pct = i >= 0 ? Math.round(((i + 1) / (PHASES.length + 1)) * 100) : null;
    if (pct !== null) setProgress(pct, ev.detail);
    else progressLabel.textContent = `${ev.status}: ${ev.detail}`;
  } else if (ev.type === "progress" && ev.phase === "download") {
    setProgress(Math.round((ev.pct / 100) * (100 / (PHASES.length + 1))),
      `downloading ${ev.region} ${ev.pct}%`);
  } else if (ev.type === "log") {
    appendLog(ev.line, ev.transient);
  } else if (ev.type === "done") {
    setProgress(100, "done");
    finish(true, `Built ${ev.output} (${formatBytes(ev.size)}).`);
    if (ev.download_url) {
      const a = document.createElement("a");
      a.href = ev.download_url;
      a.textContent = "Download";
      buildStatus.append(" ", a);
    }
  } else if (ev.type === "error") {
    finish(false, ev.message);
  }
}

function appendLog(line, transient) {
  if (transient) {
    if (!transientLine) {
      transientLine = document.createElement("div");
      logEl.appendChild(transientLine);
    }
    transientLine.textContent = line;
  } else {
    transientLine = null;
    const div = document.createElement("div");
    div.textContent = line;
    logEl.appendChild(div);
  }
  logEl.scrollTop = logEl.scrollHeight;
}

function setProgress(pct, label) {
  progressFill.style.width = pct + "%";
  if (label) progressLabel.textContent = label;
}

function setStatus(text, cls) {
  buildStatus.textContent = text;
  buildStatus.className = cls;
}

function finish(ok, message) {
  buildBtn.disabled = false;
  setStatus(message, ok ? "ok" : "err");
}

function formatBytes(n) {
  if (n > 1 << 20) return (n / (1 << 20)).toFixed(1) + " MB";
  if (n > 1 << 10) return (n / (1 << 10)).toFixed(1) + " KB";
  return n + " B";
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------
(async function init() {
  try {
    await Promise.all([loadRegions(), loadConfig(), loadPalette()]);
    renderSelected();
  } catch (e) {
    document.getElementById("style-editor").textContent = "Init failed: " + e.message;
    setStatus("Failed to load: " + e.message, "err");
  }
})();
