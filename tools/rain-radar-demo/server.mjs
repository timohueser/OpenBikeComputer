import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL(".", import.meta.url));
const dist = join(root, "dist");
const port = Number(process.argv[2] ?? process.env.OBC_RADAR_PORT ?? 4174);
const weatherBase = (process.env.OBC_WX_BASE_URL ?? "https://wx.openbikecomputer.com").replace(/\/+$/, "");
const maxRequests = positiveInt(process.env.OBC_RADAR_MAX_REQUESTS, 2000);
const maxCacheBytes = positiveInt(process.env.OBC_RADAR_CACHE_MB, 256) * 1024 * 1024;
const cache = new Map();
const inFlight = new Map();
let cacheBytes = 0;
const counters = { upstreamRequests: 0, cacheHits: 0, coalescedHits: 0, upstreamBytes: 0, errors: 0 };

function positiveInt(raw, fallback) {
  const value = Number(raw);
  return Number.isSafeInteger(value) && value > 0 ? value : fallback;
}

function json(response, status, value) {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, { "content-type": "application/json", "content-length": body.length, "cache-control": "no-store" });
  response.end(body);
}

function stats() {
  return { weatherBase, maxRequests, ...counters, inFlight: inFlight.size, cacheEntries: cache.size, cacheBytes };
}

function maxAge(headers, key) {
  if (key.endsWith("/manifest.json")) return Math.min(60, Number(headers.get("cache-control")?.match(/max-age=(\d+)/)?.[1] ?? 60));
  return 24 * 60 * 60;
}

function remember(key, value) {
  cache.delete(key);
  cache.set(key, value);
  cacheBytes += value.body.length;
  while (cacheBytes > maxCacheBytes && cache.size > 1) {
    const oldest = cache.keys().next().value;
    const removed = cache.get(oldest);
    cache.delete(oldest);
    cacheBytes -= removed.body.length;
  }
}

async function upstream(key) {
  const cached = cache.get(key);
  if (cached && cached.expiresAt > Date.now()) {
    counters.cacheHits++;
    cache.delete(key); cache.set(key, cached);
    return cached;
  }
  if (inFlight.has(key)) {
    counters.coalescedHits++;
    return inFlight.get(key);
  }
  if (counters.upstreamRequests >= maxRequests) {
    const error = new Error(`Session request ceiling reached (${maxRequests.toLocaleString()})`);
    error.status = 429;
    throw error;
  }
  const request = (async () => {
    counters.upstreamRequests++;
    const response = await fetch(`${weatherBase}/${key}`, { headers: { "user-agent": "OpenBikeComputer rain-radar-demo/0.1" } });
    if (!response.ok) {
      counters.errors++;
      const error = new Error(`Weather origin returned HTTP ${response.status}`);
      error.status = response.status;
      throw error;
    }
    const body = Buffer.from(await response.arrayBuffer());
    counters.upstreamBytes += body.length;
    const value = {
      body,
      contentType: response.headers.get("content-type") ?? "application/octet-stream",
      etag: response.headers.get("etag"),
      expiresAt: Date.now() + maxAge(response.headers, key) * 1000,
    };
    remember(key, value);
    return value;
  })().finally(() => inFlight.delete(key));
  inFlight.set(key, request);
  return request;
}

async function serveWeather(request, response, pathname) {
  const encodedKey = pathname.slice("/weather/".length);
  let key;
  try { key = decodeURIComponent(encodedKey); } catch { return json(response, 400, { error: "Malformed weather object key" }); }
  if (!/^wx\/v2\/(?:manifest\.json|[A-Za-z0-9T-]+\/f\d+\/s\d+-\d+\.obcg)$/.test(key)) {
    return json(response, 400, { error: "Only v2 manifest and shard objects may be proxied" });
  }
  try {
    const value = await upstream(key);
    response.writeHead(200, {
      "content-type": value.contentType,
      "content-length": value.body.length,
      "cache-control": key.endsWith("manifest.json") ? "no-cache" : "private, max-age=86400, immutable",
      ...(value.etag ? { etag: value.etag } : {}),
    });
    response.end(value.body);
  } catch (error) {
    json(response, error.status ?? 502, { error: error.message });
  }
}

const mime = new Map([[".html", "text/html; charset=utf-8"], [".js", "text/javascript; charset=utf-8"], [".css", "text/css; charset=utf-8"], [".map", "application/json"], [".svg", "image/svg+xml"]]);

async function serveStatic(response, pathname) {
  const requested = pathname === "/" ? "index.html" : pathname.slice(1);
  const safe = normalize(requested).replace(/^(\.\.[/\\])+/, "");
  let file = join(dist, safe);
  try {
    if ((await stat(file)).isDirectory()) file = join(file, "index.html");
    const body = await readFile(file);
    response.writeHead(200, { "content-type": mime.get(extname(file)) ?? "application/octet-stream", "content-length": body.length, "cache-control": "no-cache" });
    response.end(body);
  } catch {
    json(response, 404, { error: "Not found" });
  }
}

createServer(async (request, response) => {
  if (request.method !== "GET") return json(response, 405, { error: "GET only" });
  const url = new URL(request.url, `http://${request.headers.host ?? "localhost"}`);
  if (url.pathname === "/api/stats") return json(response, 200, stats());
  if (url.pathname.startsWith("/weather/")) return serveWeather(request, response, url.pathname);
  return serveStatic(response, url.pathname);
}).listen(port, "127.0.0.1", () => {
  console.log(`OBC rain radar: http://127.0.0.1:${port}`);
  console.log(`Weather origin: ${weatherBase}`);
  console.log(`Session ceiling: ${maxRequests.toLocaleString()} upstream requests · cache: ${maxCacheBytes / 1024 / 1024} MiB`);
});
