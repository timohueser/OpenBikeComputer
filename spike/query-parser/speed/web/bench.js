// Runs the parse bench on load, one plan step per page load, and POSTs each step to serve.py.
// URL params: model=/work/onnx/<name>/model.int8.onnx, runs=200, step, v (cache key), once (run one step, then show the table),
// plan=ort:1:cold,ort:4,ort:1:cached,rust:1:model.onnx (runtime:threads[:label]; a rust label names the ONNX file).
import * as ort from "./node_modules/onnxruntime-web/dist/ort.wasm.min.mjs";
import { Tokenizer } from "./node_modules/@huggingface/tokenizers/dist/tokenizers.min.mjs";

const q = new URLSearchParams(location.search);
const MODEL = q.get("model") || "/work/onnx/cut-50k/model.int8.onnx";
const DIR = MODEL.slice(0, MODEL.lastIndexOf("/") + 1);
const RUNS = +(q.get("runs") || 200);
const PLAN = (q.get("plan") || "ort:1:cold,ort:4,ort:1:cached,rust:1:model.int8.onnx,rust:1:model.onnx").split(",");
const STEP = +(q.get("step") || 0);
const V = q.get("v") || String(Date.now());
const KEY = `bench-${V}`;
const $ = (id) => document.getElementById(id);
const status = (s) => ($("status").textContent = s);

const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.round((p / 100) * (s.length - 1)))]; };
const median = (xs) => pct(xs, 50);

async function post(row) {
  await fetch("/results", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(row) }).catch(() => {});
}
window.addEventListener("error", (e) => post({ error: String(e.message), step: STEP, ua: navigator.userAgent }));
window.addEventListener("unhandledrejection", (e) => post({ error: String(e.reason?.stack || e.reason), step: STEP, ua: navigator.userAgent }));

async function get(url) {
  try {
    return await fetch(url);
  } catch (e) {
    throw new Error(`fetch ${url}: ${e}`);
  }
}

async function fetchTimed(url) {
  const t = performance.now();
  const res = await get(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  const buf = await res.arrayBuffer();
  const entry = performance.getEntriesByName(new URL(url, location.href).href).pop();
  return { buf, ms: performance.now() - t, fromCache: entry ? entry.transferSize === 0 : null };
}

// A runtime loads the model and returns parse(text) -> [totalMs, tokenizeMs] plus a tokenize(text) for the id check.
async function ortRuntime(threads) {
  ort.env.wasm.wasmPaths = new URL("./node_modules/onnxruntime-web/dist/", import.meta.url).href;
  ort.env.wasm.numThreads = threads;
  const t0 = performance.now();
  const [tj, tc] = await Promise.all(["tokenizer.json", "tokenizer_config.json"].map((f) => get(`${DIR}${f}?v=${V}`).then((r) => (r.ok ? r.json() : {}))));
  const tok = new Tokenizer(tj, tc);
  const tokenizerLoadMs = performance.now() - t0;
  const f = await fetchTimed(`${MODEL}?v=${V}`);
  const t = performance.now();
  const session = await ort.InferenceSession.create(new Uint8Array(f.buf), { executionProviders: ["wasm"], graphOptimizationLevel: "all" });
  const createMs = performance.now() - t;
  const parse = async (s) => {
    const t0 = performance.now();
    const ids = tok.encode(s).ids;
    const t1 = performance.now();
    const n = ids.length;
    await session.run({
      input_ids: new ort.Tensor("int64", BigInt64Array.from(ids, BigInt), [1, n]),
      attention_mask: new ort.Tensor("int64", new BigInt64Array(n).fill(1n), [1, n]),
    });
    return [performance.now() - t0, t1 - t0];
  };
  return { parse, tokenize: (s) => tok.encode(s).ids, model: MODEL, fetchMs: f.ms, fromCache: f.fromCache, createMs, tokenizerLoadMs, threadsEffective: ort.env.wasm.numThreads };
}

async function rustRuntime(file) {
  const url = `${DIR}${file || "model.int8.onnx"}`;
  const t0 = performance.now();
  const mod = await import(`/rust/pkg/parser_bench.js?v=${V}`);
  await mod.default();
  const tj = await get(`${DIR}tokenizer.json?v=${V}`).then((r) => r.text());
  const tokenizerLoadMs = performance.now() - t0;
  const f = await fetchTimed(`${url}?v=${V}`);
  const t = performance.now();
  const model = new mod.Model(new Uint8Array(f.buf), tj);
  const createMs = performance.now() - t;
  const parse = async (s) => {
    const t0 = performance.now();
    const ids = model.tokenize(s);
    const t1 = performance.now();
    model.run(ids);
    return [performance.now() - t0, t1 - t0];
  };
  return { parse, tokenize: (s) => Array.from(model.tokenize(s)), model: `${url} (tract)`, fetchMs: f.ms, fromCache: f.fromCache, createMs, tokenizerLoadMs, threadsEffective: 1 };
}

async function memoryMb() {
  if (!crossOriginIsolated || !performance.measureUserAgentSpecificMemory) return null;
  try {
    const m = await Promise.race([performance.measureUserAgentSpecificMemory(), new Promise((r) => setTimeout(() => r(null), 20000))]);
    return m ? m.bytes / 1e6 : null;
  } catch {
    return null;
  }
}

function render(rows) {
  $("results").tBodies[0].innerHTML = rows.map((r) => `<tr><td>${r.step} ${r.runtime} ${r.label}</td><td>${r.threadsEffective}</td>
    <td>${(r.fetchMs + r.createMs).toFixed(0)}${r.fromCache ? " (cache)" : ""}</td><td>${r.firstMs.toFixed(1)}</td>
    <td>${r.medianMs.toFixed(1)}</td><td>${r.p95Ms.toFixed(1)}</td></tr>`).join("");
}

async function main() {
  const rows = JSON.parse(sessionStorage.getItem(KEY) || "[]");
  render(rows);
  if (STEP >= PLAN.length) { status(`done: ${navigator.userAgent}`); return; }
  const [runtime, threadsArg, label = ""] = PLAN[STEP].split(":");
  const threads = +threadsArg;
  status(`step ${STEP + 1}/${PLAN.length}: ${runtime}, ${threads} thread(s), crossOriginIsolated=${crossOriginIsolated}`);

  const focusStart = `${document.visibilityState}/${document.hasFocus() ? "focus" : "blur"}`;
  const expected = await get(`${DIR}expected_ids.json?v=${V}`).then((r) => r.json());
  const rt = runtime === "rust" ? await rustRuntime(label) : await ortRuntime(threads);
  let tokMismatch = 0;
  for (const e of expected) if (rt.tokenize(e.text).join() !== e.ids.join()) tokMismatch++;
  const parse = rt.parse;
  const sentences = expected.map((e) => e.text);
  const [firstMs] = await parse(sentences[0]);
  const times = [], tokTimes = [];
  for (let i = 0; i < RUNS; i++) {
    const [total, tk] = await parse(sentences[(i + 1) % sentences.length]);
    times.push(total); tokTimes.push(tk);
    if (i % 20 === 0) status(`step ${STEP + 1}/${PLAN.length}: ${i}/${RUNS} parses, median so far ${median(times).toFixed(1)} ms`);
  }
  const row = {
    step: STEP, v: V, label, runtime, threadsRequested: threads, threadsEffective: rt.threadsEffective, crossOriginIsolated,
    model: rt.model, fetchMs: rt.fetchMs, fromCache: rt.fromCache, createMs: rt.createMs,
    tokenizerLoadMs: rt.tokenizerLoadMs, firstMs, medianMs: median(times), p95Ms: pct(times, 95), tokenizeMedianMs: median(tokTimes),
    runs: RUNS, tokMismatch, focus: `${focusStart} -> ${document.visibilityState}/${document.hasFocus() ? "focus" : "blur"}`, sentences: sentences.length, memoryMb: await memoryMb(), ua: navigator.userAgent,
  };
  rows.push(row);
  sessionStorage.setItem(KEY, JSON.stringify(rows));
  render(rows);
  await post(row);
  const next = new URLSearchParams(q);
  next.set("step", q.has("once") ? PLAN.length : STEP + 1); next.set("v", V); next.set("model", MODEL); next.set("plan", PLAN.join(","));
  location.replace(`?${next}`);
}

main().catch((e) => { status(`error: ${e}`); post({ error: String(e?.stack || e), step: STEP, ua: navigator.userAgent }); });
