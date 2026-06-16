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
  if (popupOpen) return; // picker is open; don't shoot the preview through the menu
  pendingLatLng = e.latlng;
  if (map.hasLayer(previewTip)) previewTip.setLatLng(e.latlng); // tip follows cursor
  if (!rafPending) { rafPending = true; requestAnimationFrame(updatePreview); }
});
map.on("mouseout", () => { if (!popupOpen) { pendingLatLng = null; clearPreview(); } });
map.on("popupclose", () => { popupOpen = false; clearPreview(); });

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

// ---------------------------------------------------------------------------
// Style editor
// ---------------------------------------------------------------------------
async function loadConfig() {
  config = await fetch("/api/config/default").then((r) => r.json());
  for (const cat of Object.keys(config.features)) {
    for (const name of Object.keys(config.features[cat])) {
      enabled.set(`${cat}/${name}`, true);
    }
  }
  renderStyleEditor();
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
    summary.innerHTML = `${cat} <span class="count">(${Object.keys(entries).length})</span>`;
    group.appendChild(summary);

    const table = document.createElement("table");
    table.className = "feat-table";
    table.innerHTML =
      "<thead><tr><th></th><th>type</th><th>id</th><th>color</th><th>z</th><th>w</th><th></th></tr></thead>";
    const tbody = document.createElement("tbody");
    for (const name of Object.keys(entries)) {
      tbody.appendChild(buildRow(cat, name, entries[name]));
    }
    table.appendChild(tbody);
    group.appendChild(table);

    const add = document.createElement("button");
    add.className = "add-feat";
    add.textContent = "+ add type";
    add.onclick = () => addFeature(cat, tbody);
    group.appendChild(add);

    root.appendChild(group);
  }
}

function buildRow(cat, name, def) {
  const key = `${cat}/${name}`;
  const tr = document.createElement("tr");

  const tdToggle = document.createElement("td");
  const cb = document.createElement("input");
  cb.type = "checkbox";
  cb.checked = enabled.get(key) !== false;
  cb.onchange = () => {
    enabled.set(key, cb.checked);
    tr.classList.toggle("feat-off", !cb.checked);
  };
  tdToggle.appendChild(cb);

  const tdName = document.createElement("td");
  tdName.className = "feat-name";
  tdName.textContent = name;

  const tdId = document.createElement("td");
  const idIn = numInput(def.id, (v) => (def.id = v));
  tdId.appendChild(idIn);

  const tdColor = document.createElement("td");
  const color = document.createElement("input");
  color.type = "color";
  color.value = rgb565ToHex(def.color);
  const label = document.createElement("span");
  label.className = "rgb565";
  label.textContent = def.color;
  color.oninput = () => {
    def.color = hexToRgb565(color.value);
    label.textContent = def.color;
  };
  tdColor.appendChild(color);
  tdColor.appendChild(label);

  const tdZ = document.createElement("td");
  tdZ.appendChild(numInput(def.z_index, (v) => (def.z_index = v)));

  const tdW = document.createElement("td");
  tdW.appendChild(numInput(def.weight, (v) => (def.weight = v)));

  const tdDel = document.createElement("td");
  const del = document.createElement("button");
  del.className = "add-feat";
  del.textContent = "×";
  del.title = "Remove type";
  del.onclick = () => {
    delete config.features[cat][name];
    enabled.delete(key);
    tr.remove();
  };
  tdDel.appendChild(del);

  if (!cb.checked) tr.classList.add("feat-off");
  for (const td of [tdToggle, tdName, tdId, tdColor, tdZ, tdW, tdDel]) tr.appendChild(td);
  return tr;
}

function numInput(value, onChange) {
  const i = document.createElement("input");
  i.type = "number";
  i.value = value;
  i.oninput = () => onChange(parseInt(i.value, 10) || 0);
  return i;
}

function addFeature(cat, tbody) {
  const name = prompt(`New ${cat} type (OSM tag value, e.g. "steps"):`);
  if (!name) return;
  if (config.features[cat][name]) {
    alert("That type already exists.");
    return;
  }
  const maxId = Math.max(
    0,
    ...Object.values(config.features).flatMap((c) => Object.values(c).map((d) => d.id || 0))
  );
  const def = { id: maxId + 1, z_index: 10, color: "0xFFFF", weight: 1 };
  config.features[cat][name] = def;
  enabled.set(`${cat}/${name}`, true);
  tbody.appendChild(buildRow(cat, name, def));
}

function buildConfigForSubmit() {
  const out = { features: {} };
  for (const cat of Object.keys(config.features)) {
    for (const name of Object.keys(config.features[cat])) {
      if (enabled.get(`${cat}/${name}`) === false) continue;
      out.features[cat] = out.features[cat] || {};
      out.features[cat][name] = config.features[cat][name];
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Build / jobs
// ---------------------------------------------------------------------------
const buildBtn = document.getElementById("build-btn");
const buildStatus = document.getElementById("build-status");
const progressWrap = document.getElementById("progress-wrap");
const progressFill = document.getElementById("progress-fill");
const progressLabel = document.getElementById("progress-label");
const logEl = document.getElementById("log");

const PHASES = ["downloading", "merging", "ingest", "bbox", "land", "quadtree", "serialize"];
let transientLine = null; // last tqdm-style line element

buildBtn.addEventListener("click", async () => {
  if (selected.size === 0) {
    setStatus("Select at least one region first.", "err");
    return;
  }
  const body = {
    region_ids: [...selected],
    config: buildConfigForSubmit(),
    chunk_size: parseInt(document.getElementById("chunk-size").value, 10) || 4096,
    output_name: document.getElementById("output-name").value.trim() || "output.obcm",
  };

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
    finish(true, `Built ${ev.output} (${formatBytes(ev.size)}) in project folder.`);
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
    await Promise.all([loadRegions(), loadConfig()]);
    renderSelected();
  } catch (e) {
    document.getElementById("style-editor").textContent = "Init failed: " + e.message;
    setStatus("Failed to load: " + e.message, "err");
  }
})();
