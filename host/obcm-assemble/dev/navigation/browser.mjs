// Run the shipping assembly worker against the pinned NG1 selection in real Chromium.
import { createRequire } from 'node:module';
import { readFileSync, createReadStream, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import os from 'node:os';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../../../..');
const requireBuilder = createRequire(resolve(root, 'builder/app/package.json'));
const requireBrowser = createRequire(resolve(root, 'apps/obc-web-demo/tests/browser/package.json'));
const { createServer } = await import(requireBuilder.resolve('vite'));
const { chromium } = requireBrowser('playwright');
const [input, output, blockArg] = process.argv.slice(2);
const readBlockBytes = blockArg === undefined ? 65536 : Number(blockArg);
if (![4096, 65536].includes(readBlockBytes)) throw new Error("Expected 4096 or 65536 read block bytes");
if (!input || !output) throw new Error('Usage: node browser.mjs INPUT_DIRECTORY OUTPUT.json [4096|65536]');
const manifest = JSON.parse(readFileSync(resolve(here, 'inputs.json')));
const files = new Map(manifest.objects.map(e => [e.sha256, resolve(input, e.sha256 + (e.band === 'terrain' ? '.obcd' : '.obcm'))]));
const wrapper = `
const memories = [];
for (const name of ['instantiate', 'instantiateStreaming']) {
  const original = WebAssembly[name];
  WebAssembly[name] = async (...args) => {
    const result = await original(...args);
    const memory = (result.instance ?? result).exports.memory;
    if (memory) memories.push(memory);
    return result;
  };
}
const send = self.postMessage.bind(self);
self.postMessage = (message, ...args) => {
  if (message.type === 'done') message.measurement = {
    wasm_capacity_bytes: Math.max(...memories.map(m => m.buffer.byteLength)),
    scratch_peak_live_bytes: globalThis.ngScratch.peak,
  };
  send(message, ...args);
};
await import('/src/lib/assemble/assemble.worker.ts');
const { initAssemble } = await import('/src/lib/assemble/bridge.ts');
await initAssemble();
send({type:'ready'});
`;
const server = await createServer({
  configFile: false, root: resolve(root, 'builder/app'), server: { host: '127.0.0.1', port: 0 },
  plugins: [{
    name: 'ng1-measurement', enforce: 'pre',
    transform(code, id) {
      if (!id.endsWith('/cells/store.ts')) return;
      // Observe logical live scratch without changing the shipping storage policy.
      for (const [from, to] of [
        ['const live = new Map<number, { slot: number; written: number }>();',
         'const live = new Map<number, { slot: number; written: number }>();\nconst metric = globalThis.ngScratch = {live:0, peak:0};'],
        ['at.written += n;', 'at.written += n; metric.live += n; metric.peak = Math.max(metric.peak, metric.live);'],
        ['live.delete(id);', 'metric.live -= at.written; live.delete(id);'],
      ]) {
        if (code.split(from).length !== 2) throw new Error('Scratch measurement source changed: ' + from);
        code = code.replace(from, to);
      }
      return code;
    },
    configureServer(vite) {
      vite.middlewares.use((req, res, next) => {
        res.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
        res.setHeader('Cross-Origin-Embedder-Policy', 'require-corp');
        if (req.url === '/ng') {
          res.setHeader('Content-Type', 'text/html'); res.end('<title>NG1 assembly measurement</title>');
        } else if (req.url === '/ng-worker.js') {
          res.setHeader('Content-Type', 'text/javascript'); res.end(wrapper);
        } else if (req.url.startsWith('/input/')) {
          const path = files.get(req.url.slice(7));
          if (!path) { res.statusCode = 404; res.end(); return; }
          createReadStream(path).pipe(res);
        } else next();
      });
    },
  }],
});
await server.listen();
let browser;
const profile = mkdtempSync(resolve(os.tmpdir(), 'obc-ng-browser-'));
try {
  browser = await chromium.launchPersistentContext(profile, { headless: true, executablePath: process.env.CHROME_BIN || '/usr/bin/google-chrome', args: ['--no-sandbox'] });
  const page = await browser.newPage();
  page.setDefaultTimeout(180000);
  page.on('pageerror', error => console.error(error));
  page.on('console', message => { if(message.text().startsWith('NG1 ')) console.log(message.text()); });
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/ng`);
  const result = await page.evaluate(async ({manifest, readBlockBytes}) => {
    const { openCellStore } = await import('/src/lib/cells/store.ts');
    const store = await openCellStore('ng1');
    if (!store) throw new Error('OPFS input unavailable');
    const sourceCells = [], terrainCells = [];
    for (const e of manifest.objects) {
      const response = await fetch('/input/' + e.sha256);
      if (!response.ok) throw new Error('Missing input ' + e.id);
      const bytes = new Uint8Array(await response.arrayBuffer());
      const digest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), b => b.toString(16).padStart(2, '0')).join('');
      if (bytes.length !== e.bytes || digest !== e.sha256) throw new Error('Input mismatch ' + e.id);
      if (e.band === 'terrain') terrainCells.push({id:e.id,sha256:e.sha256,bytes});
      else {
        await store.put(e.sha256, bytes);
        sourceCells.push({id:e.id,band:e.band,partial:!!e.partial,byteLength:e.bytes,key:e.sha256});
      }
    }
    console.log('NG1 input staging complete');
    const worker = new Worker('/ng-worker.js', { type: 'module' });
    await new Promise((ok, bad) => { worker.onmessage = e => e.data.type === 'ready' && ok(); worker.onerror = bad; });
    const messages = [];
    const estimate = await new Promise((ok,bad) => {
      worker.onmessage = e => e.data.type === 'error' ? bad(new Error(e.data.message)) : ok(e.data);
      worker.postMessage({type:'estimate',estimateId:1,onDisk:true,
        networkBandBytes:manifest.objects.filter(e=>e.band==='network').reduce((n,e)=>n+e.bytes,0),
        totalCellBytes:manifest.objects.filter(e=>e.band!=='terrain').reduce((n,e)=>n+e.bytes,0),
        terrainBytes:terrainCells.reduce((n,e)=>n+e.bytes.length,0),mergeBudgetBytes:manifest.options.merge_budget_bytes});
    });
    if (!estimate.onDisk) throw new Error('Disk-backed estimate refused');
    console.log('NG1 assembly starts');
    const start = performance.now();
    const done = await new Promise((ok,bad) => {
      worker.onmessage = e => {
        messages.push(e.data);
        if (e.data.type === 'error') bad(new Error(e.data.message));
        if (e.data.type === 'done') ok(e.data);
      };
      worker.onerror = bad;
      worker.postMessage({type:'assemble',requireDisk:true,cells:[],cellStore:'ng1',sourceCells,knownEmpty:[],
        schemaJson:JSON.stringify({schema:manifest.schema}),skinJson:JSON.stringify(manifest.skin),
        options:{readBlockBytes,mergeBudgetBytes:manifest.options.merge_budget_bytes,acceptPartial:manifest.options.accept_partial,acceptHoles:manifest.options.accept_holes},
        terrain:{postingLog2:manifest.terrain.posting_log2,cellLog2:manifest.terrain.cell_log2},terrainCells},
        terrainCells.map(e=>e.bytes.buffer));
    });
    const request_ms = performance.now()-start;
    worker.terminate();
    const outputDir = await (await navigator.storage.getDirectory()).getDirectoryHandle('obc-out');
    globalThis.ngOutput = await (await outputDir.getFileHandle('map.part')).getFile();
    const stored = messages.find(m=>m.type==='stored-map');
    if (!stored || stored.byteLength !== globalThis.ngOutput.size) throw new Error('Missing or short sunk output');
    return {request_ms,estimate,done,stored,reading:messages.find(m=>m.type==='reading'),writing:messages.find(m=>m.type==='writing'),visibility:document.visibilityState};
  }, {manifest, readBlockBytes});
  console.log(JSON.stringify({stage:'assembled',request_ms:result.request_ms,...result.done.measurement}));
  // Read back in bounded chunks after the assembly timing and memory observation.
  const hash = createHash('sha256');
  for (let at=0; at<result.stored.byteLength; at+=1024*1024) {
    const encoded = await page.evaluate(async at => {
      const data = new Uint8Array(await globalThis.ngOutput.slice(at,at+1024*1024).arrayBuffer());
      let binary=''; for(let p=0;p<data.length;p+=32768) binary+=String.fromCharCode(...data.subarray(p,p+32768));
      return btoa(binary);
    }, at);
    hash.update(Buffer.from(encoded,'base64'));
  }
  result.readback_sha256 = hash.digest('hex');
  if (result.readback_sha256 !== result.stored.sha256) throw new Error('Independent output digest mismatch');
  result.read_block_bytes = readBlockBytes;
  result.measured_at_utc = new Date().toISOString();
  result.source_commit = execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim();
  result.wasm_sha256 = createHash('sha256').update(readFileSync(resolve(root,'builder/app/src/lib/assemble/pkg/obc_web_assemble_bg.wasm'))).digest('hex');
  result.manifest_sha256 = createHash('sha256').update(readFileSync(resolve(here,'inputs.json'))).digest('hex');
  result.browser = browser.browser().version(); result.context = 'persistent, non-incognito'; result.os = `${os.type()} ${os.release()} ${os.arch()}`;
  result.note = 'Host browser evidence. Request timing excludes input download/hash, WASM initialization and independent output readback. WASM capacity excludes JS/browser memory. Logical scratch excludes removed files retained in pool slots.';
  writeFileSync(output,JSON.stringify(result,null,2)+'\n');
  console.log(JSON.stringify({request_ms:result.request_ms,...result.done.measurement,sha256:result.readback_sha256}));
} finally { await browser?.close(); await server.close(); rmSync(profile,{recursive:true,force:true}); }
