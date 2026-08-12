import L from "leaflet";
import "./style.css";
import { parseCrc32, parseObcg, sampleTile, type ObcgObject } from "./obcg";
import { shardKey, shardsForBounds, validateManifest } from "./manifest";
import type { ManifestShard, ProxyStats, RainFrame, RainManifest, ShardId } from "./types";

const $ = <T extends HTMLElement>(id: string): T => {
  const element = document.getElementById(id);
  if (!element) throw new Error(`Missing #${id}`);
  return element as T;
};

const initialMatch = location.hash.match(/^#map=(\d+)\/(-?\d+(?:\.\d+)?)\/(-?\d+(?:\.\d+)?)$/);
const initialCenter: L.LatLngExpression = initialMatch ? [Number(initialMatch[2]), Number(initialMatch[3])] : [48.1, 8];
const initialZoom = initialMatch ? Number(initialMatch[1]) : 5;
const map = L.map("map", { center: initialCenter, zoom: initialZoom, minZoom: 2, maxZoom: 12, worldCopyJump: false, maxBoundsViscosity: 1 });
map.setMaxBounds([[-85.0511, -180], [85.0511, 180]]);
L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
  attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap contributors</a>',
  maxZoom: 19,
  noWrap: true,
}).addTo(map);

const overlay = $<HTMLCanvasElement>("weather-overlay");
const overlayContext = overlay.getContext("2d", { alpha: true })!;
const objectCache = new Map<string, ObcgObject>();
let manifest: RainManifest | null = null;
let frameIndex = 0;
let renderRevision = 0;
let playTimer: number | undefined;
let shardLayer = L.layerGroup().addTo(map);

const palette: ReadonlyArray<readonly [number, number, number, number]> = [
  [0, 0, 0, 0], [99, 217, 255, 150], [66, 203, 255, 165], [43, 226, 181, 175],
  [45, 223, 110, 180], [181, 229, 64, 190], [244, 232, 74, 200], [255, 193, 57, 210],
  [255, 139, 54, 218], [255, 83, 70, 225], [242, 48, 101, 232], [202, 57, 205, 238],
  [184, 81, 255, 245], [0, 0, 0, 0], [0, 0, 0, 0], [142, 160, 166, 95],
];

function status(message: string, error = false): void {
  const node = $("map-status");
  node.textContent = message;
  node.classList.toggle("error", error);
  node.classList.remove("hidden");
}

function hideStatus(): void {
  $("map-status").classList.add("hidden");
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
}

function formatUtc(value: string): string {
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", timeZoneName: "short" }).format(new Date(value));
}

async function pooledMap<T, U>(values: T[], limit: number, task: (value: T) => Promise<U>): Promise<U[]> {
  const results = new Array<U>(values.length);
  let cursor = 0;
  const workers = Array.from({ length: Math.min(limit, values.length) }, async () => {
    for (;;) {
      const index = cursor++;
      if (index >= values.length) return;
      results[index] = await task(values[index]);
    }
  });
  await Promise.all(workers);
  return results;
}

async function fetchObject(key: string, shard: ManifestShard): Promise<ObcgObject> {
  const cached = objectCache.get(key);
  if (cached) return cached;
  const response = await fetch(`/weather/${key}`);
  if (!response.ok) throw new Error(`${key}: HTTP ${response.status}`);
  const buffer = await response.arrayBuffer();
  if (buffer.byteLength !== shard.bytes) throw new Error(`${key}: manifest promised ${shard.bytes} bytes, got ${buffer.byteLength}`);
  const object = parseObcg(buffer, parseCrc32(shard.object_crc32));
  objectCache.set(key, object);
  $("browser-objects").textContent = objectCache.size.toLocaleString();
  return object;
}

function visibleShardIds(): ShardId[] {
  if (!manifest) return [];
  const bounds = map.getBounds();
  return shardsForBounds(manifest.lattice, bounds.getSouth(), bounds.getWest(), bounds.getNorth(), bounds.getEast());
}

function drawShardBoundaries(ids: ShardId[]): void {
  shardLayer.clearLayers();
  if (!manifest || !(($("show-shards") as HTMLInputElement).checked)) return;
  const grid = manifest.lattice;
  for (const id of ids) {
    const south = (grid.south_lat_udeg + id.row * grid.shard_height * grid.cell_udeg) / 1e6;
    const west = (grid.west_lon_udeg + id.col * grid.shard_width * grid.cell_udeg) / 1e6;
    const north = Math.min((grid.south_lat_udeg + (id.row + 1) * grid.shard_height * grid.cell_udeg) / 1e6, 90);
    const east = Math.min((grid.west_lon_udeg + (id.col + 1) * grid.shard_width * grid.cell_udeg) / 1e6, 180);
    L.rectangle([[south, west], [north, east]], { color: "#61e6bb", weight: 1, fill: false, opacity: 0.65 }).addTo(shardLayer);
  }
}

function resizeOverlay(): void {
  const size = map.getSize();
  const ratio = window.devicePixelRatio || 1;
  overlay.width = Math.round(size.x * ratio);
  overlay.height = Math.round(size.y * ratio);
  overlay.style.width = `${size.x}px`;
  overlay.style.height = `${size.y}px`;
  overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
  overlayContext.imageSmoothingEnabled = false;
}

async function drawObject(object: ObcgObject, revision: number): Promise<{ rendered: number; observed: boolean }> {
  const header = object.header;
  const edge = header.tileEdge;
  const bounds = map.getBounds();
  const objectSouth = header.southLatUdeg / 1e6;
  const objectWest = header.westLonUdeg / 1e6;
  const cellLat = header.cellLatUdeg / 1e6;
  const cellLon = header.cellLonUdeg / 1e6;
  const objectNorth = objectSouth + header.height * cellLat;
  const objectEast = objectWest + header.width * cellLon;
  const south = Math.max(bounds.getSouth(), objectSouth);
  const north = Math.min(bounds.getNorth(), objectNorth);
  const west = Math.max(bounds.getWest(), objectWest);
  const east = Math.min(bounds.getEast(), objectEast);
  if (south >= north || west >= east) return { rendered: 0, observed: header.flags === 1 };

  const firstCol = Math.max(0, Math.floor((west - objectWest) / cellLon / edge));
  const lastCol = Math.min(header.tileCols - 1, Math.ceil((east - objectWest) / cellLon / edge) - 1);
  const firstRow = Math.max(0, Math.floor((south - objectSouth) / cellLat / edge));
  const lastRow = Math.min(header.tileRows - 1, Math.ceil((north - objectSouth) / cellLat / edge) - 1);
  const showNodata = ($("show-nodata") as HTMLInputElement).checked;
  let rendered = 0;
  let processed = 0;
  const tileCanvas = document.createElement("canvas");
  const tileContext = tileCanvas.getContext("2d")!;

  for (let tileRow = firstRow; tileRow <= lastRow; tileRow++) {
    for (let tileCol = firstCol; tileCol <= lastCol; tileCol++) {
      if (revision !== renderRevision) return { rendered, observed: header.flags === 1 };
      const tileIndex = tileRow * header.tileCols + tileCol;
      const tileSouth = objectSouth + tileRow * edge * cellLat;
      const tileWest = objectWest + tileCol * edge * cellLon;
      const tileNorth = Math.min(tileSouth + edge * cellLat, objectNorth);
      const tileEast = Math.min(tileWest + edge * cellLon, objectEast);
      const topLeft = map.latLngToContainerPoint([tileNorth, tileWest]);
      const bottomRight = map.latLngToContainerPoint([tileSouth, tileEast]);
      const drawWidth = Math.max(1, bottomRight.x - topLeft.x);
      const drawHeight = Math.max(1, bottomRight.y - topLeft.y);
      const rasterWidth = Math.max(1, Math.min(edge, Math.ceil(drawWidth)));
      const rasterHeight = Math.max(1, Math.min(edge, Math.ceil(drawHeight)));
      const samples = sampleTile(object, tileIndex, rasterWidth, rasterHeight);
      processed++;
      if (!samples) continue;
      const image = new ImageData(rasterWidth, rasterHeight);
      let hasVisible = false;
      for (let y = 0; y < rasterHeight; y++) {
        for (let x = 0; x < rasterWidth; x++) {
          const intensity = samples[y * rasterWidth + x];
          if (intensity === 15 && !showNodata) continue;
          const color = palette[intensity];
          const output = (y * rasterWidth + x) * 4;
          image.data[output] = color[0]; image.data[output + 1] = color[1]; image.data[output + 2] = color[2]; image.data[output + 3] = color[3];
          hasVisible ||= color[3] > 0;
        }
      }
      if (hasVisible) {
        tileCanvas.width = rasterWidth; tileCanvas.height = rasterHeight;
        tileContext.putImageData(image, 0, 0);
        overlayContext.drawImage(tileCanvas, topLeft.x, topLeft.y, drawWidth, drawHeight);
        rendered++;
      }
      if (processed % 64 === 0) await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    }
  }
  return { rendered, observed: header.flags === 1 };
}

async function render(): Promise<void> {
  if (!manifest) return;
  const revision = ++renderRevision;
  resizeOverlay();
  overlayContext.clearRect(0, 0, overlay.width, overlay.height);
  const frame = manifest.frames[frameIndex];
  const ids = visibleShardIds();
  drawShardBoundaries(ids);
  $("visible-shards").textContent = `${ids.length} (${ids.map((id) => `${id.col},${id.row}`).join(" · ") || "none"})`;
  updateViewReadout();

  const byId = new Map(frame.shards.map((shard) => [`${shard.col},${shard.row}`, shard]));
  const present = ids.flatMap((id) => {
    const shard = byId.get(`${id.col},${id.row}`);
    return shard ? [{ id, shard }] : [];
  });
  const dry = ids.length - present.length;
  status(`Loading ${present.length} visible shard${present.length === 1 ? "" : "s"}${dry ? ` · ${dry} dry` : ""}…`);
  try {
    const loaded = await pooledMap(present, 4, async ({ id, shard }) => ({
      object: await fetchObject(shardKey(manifest!.key_prefix, manifest!.generation, frame.offset_min, id), shard),
    }));
    if (revision !== renderRevision) return;
    overlayContext.clearRect(0, 0, overlay.width, overlay.height);
    let rendered = 0;
    let observed = 0;
    for (const item of loaded) {
      const result = await drawObject(item.object, revision);
      rendered += result.rendered;
      observed += Number(result.observed);
    }
    if (revision !== renderRevision) return;
    $("rendered-tiles").textContent = rendered.toLocaleString();
    $("source-class").textContent = present.length ? `${observed} observed · ${present.length - observed} forecast` : "Dry (manifest)";
    hideStatus();
  } catch (error) {
    if (revision !== renderRevision) return;
    status(error instanceof Error ? error.message : String(error), true);
  }
  void refreshStats();
}

function updateTimeline(): void {
  if (!manifest) return;
  const frame = manifest.frames[frameIndex];
  $("frame-time").textContent = formatUtc(frame.valid_at);
  $("frame-offset").textContent = frame.offset_min === 0 ? "Analysis / now" : `+${frame.offset_min} min forecast`;
  ($("timeline") as HTMLInputElement).value = String(frameIndex);
}

function updateViewReadout(): void {
  const center = map.getCenter();
  $("center").textContent = `${center.lat.toFixed(3)}°, ${center.lng.toFixed(3)}°`;
  $("zoom").textContent = map.getZoom().toFixed(0);
}

function updateDataset(): void {
  if (!manifest) return;
  $("generation").textContent = manifest.generation;
  $("reference").textContent = formatUtc(manifest.reference_time);
  $("grid").textContent = `${manifest.lattice.width.toLocaleString()} × ${manifest.lattice.height.toLocaleString()} · ${(manifest.lattice.cell_size_m / 1000).toFixed(1)} km`;
  $("fresh-until").textContent = formatUtc(manifest.freshness.stale_after);
  const attribution = $("attribution");
  attribution.replaceChildren(...manifest.attribution.map((source) => {
    const link = document.createElement("a");
    link.href = source.url; link.target = "_blank"; link.rel = "noreferrer"; link.textContent = source.source_id;
    link.title = source.text;
    return link;
  }));
  const timeline = $("timeline") as HTMLInputElement;
  timeline.max = String(manifest.frames.length - 1);
  const last = manifest.frames.at(-1)?.offset_min ?? 0;
  $("timeline-labels").innerHTML = `<span>Now</span><span>+${last / 60}h</span>`;
  updateTimeline();
}

async function loadManifest(): Promise<void> {
  status("Loading manifest…");
  try {
    const response = await fetch("/weather/wx/v2/manifest.json", { cache: "no-cache" });
    if (!response.ok) throw new Error(`Manifest request failed: HTTP ${response.status}`);
    manifest = validateManifest(await response.json());
    frameIndex = Math.min(frameIndex, manifest.frames.length - 1);
    updateDataset();
    const health = $("service-health");
    health.className = "health good";
    health.innerHTML = "<i></i> Live dataset";
    await render();
  } catch (error) {
    const health = $("service-health");
    health.className = "health bad";
    health.innerHTML = "<i></i> Dataset error";
    status(error instanceof Error ? error.message : String(error), true);
  }
}

async function refreshStats(): Promise<void> {
  try {
    const stats = await fetch("/api/stats", { cache: "no-store" }).then((response) => response.json()) as ProxyStats;
    const percent = Math.min(100, stats.upstreamRequests / stats.maxRequests * 100);
    $("upstream-requests").textContent = `${stats.upstreamRequests.toLocaleString()} / ${stats.maxRequests.toLocaleString()}`;
    $("cache-hits").textContent = (stats.cacheHits + stats.coalescedHits).toLocaleString();
    $("downloaded").textContent = formatBytes(stats.upstreamBytes);
    $("request-percent").textContent = `${percent.toFixed(percent < 1 ? 1 : 0)}%`;
    ($("budget-bar") as HTMLElement).style.width = `${percent}%`;
  } catch { /* The map error state is more useful than a second stats error. */ }
}

map.on("moveend zoomend resize", () => { void render(); });
$("timeline").addEventListener("input", (event) => {
  frameIndex = Number((event.target as HTMLInputElement).value);
  updateTimeline();
  void render();
});
$("opacity").addEventListener("input", (event) => {
  const value = Number((event.target as HTMLInputElement).value);
  overlay.style.opacity = String(value / 100);
  $("opacity-value").textContent = `${value}%`;
});
$("show-nodata").addEventListener("change", () => { void render(); });
$("show-shards").addEventListener("change", () => drawShardBoundaries(visibleShardIds()));
$("refresh").addEventListener("click", () => { void loadManifest(); });
$("world-view").addEventListener("click", () => map.setView([0, 0], 2));
$("play").addEventListener("click", () => {
  const button = $("play");
  if (playTimer !== undefined) {
    window.clearTimeout(playTimer); playTimer = undefined; button.textContent = "▶"; button.classList.remove("playing"); return;
  }
  button.textContent = "Ⅱ"; button.classList.add("playing");
  const advance = async () => {
    if (!manifest) return;
    frameIndex = (frameIndex + 1) % manifest.frames.length;
    updateTimeline(); await render();
    if (playTimer !== undefined) playTimer = window.setTimeout(() => { void advance(); }, 800);
  };
  playTimer = window.setTimeout(() => { void advance(); }, 250);
});
$("copy-view").addEventListener("click", async () => {
  const center = map.getCenter();
  const url = new URL(location.href);
  url.hash = `map=${map.getZoom()}/${center.lat.toFixed(5)}/${center.lng.toFixed(5)}`;
  await navigator.clipboard.writeText(url.toString());
  $("copy-view").textContent = "Copied";
  window.setTimeout(() => { $("copy-view").textContent = "Copy link"; }, 1200);
});

overlay.style.opacity = "0.72";
setInterval(() => { void refreshStats(); }, 2000);
void refreshStats();
void loadManifest();
