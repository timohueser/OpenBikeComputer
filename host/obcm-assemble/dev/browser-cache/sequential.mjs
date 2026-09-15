// Diagnostic only: apply sequential.patch and build the WASM before running this file.
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const requireBuilder = createRequire(resolve(root, 'builder/app/package.json'));
const requireBrowser = createRequire(resolve(root, 'apps/obc-web-demo/tests/browser/package.json'));
const { createServer } = await import(requireBuilder.resolve('vite'));
const { chromium } = requireBrowser('playwright');
const [output, profileParent] = process.argv.slice(2);
if (!output || !profileParent) throw new Error('Usage: node sequential.mjs OUTPUT.json PROFILE_PARENT');
const worker = `
import init, { sequential_cache_scan } from '/src/lib/assemble/pkg/obc_web_assemble.js';
await init();
const root = await navigator.storage.getDirectory();
const file = await root.getFileHandle('sequential', {create:true});
const handle = await file.createSyncAccessHandle();
const len = 32 * 1024 * 1024;
const block = Uint8Array.from({length:256*1024}, (_,i)=>i&255);
for(let at=0;at<len;at+=block.length) if(handle.write(block,{at})!==block.length) throw new Error('short write');
handle.flush();
const results = [];
for (const width of [17, 512, 256*1024]) {
  for (const cacheBlock of [65536,4096,4096,65536,65536,4096]) {
    let calls=0, bytes=0;
    const read = (_slot,at,into) => { calls++; bytes+=into.length; return handle.read(into,{at})===into.length; };
    const start=performance.now();
    const checksum=sequential_cache_scan(read,cacheBlock,width,len);
    const elapsed_ms=performance.now()-start;
    if(checksum!==len/256*32640 || bytes!==len) throw new Error('sequential content or traffic mismatch');
    results.push({width,cache_block_bytes:cacheBlock,elapsed_ms,calls,bytes,checksum});
  }
}
handle.close();
postMessage({source_bytes:len,results});
`;
const server = await createServer({configFile:false,root:resolve(root,'builder/app'),server:{host:'127.0.0.1',port:0},plugins:[{
  name:'sequential-cache-diagnostic',configureServer(vite){vite.middlewares.use((req,res,next)=>{
    if(req.url==='/sequential-worker.js'){res.setHeader('Content-Type','text/javascript');res.end(worker);}
    else if(req.url==='/sequential'){res.setHeader('Content-Type','text/html');res.end('<title>Sequential cache diagnostic</title>');}
    else next();
  });}
}]});
await server.listen();
const profile=mkdtempSync(resolve(profileParent,'obc-sequential-'));
let browser;
try {
  browser=await chromium.launchPersistentContext(profile,{headless:true,executablePath:process.env.CHROME_BIN||'/usr/bin/google-chrome',args:['--no-sandbox']});
  const page=await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/sequential`);
  const result=await page.evaluate(()=>new Promise((ok,bad)=>{
    const worker=new Worker('/sequential-worker.js',{type:'module'});
    worker.onmessage=e=>{worker.terminate();ok(e.data);};worker.onerror=bad;
  }));
  result.source_commit=execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim();
  result.wasm_sha256=createHash('sha256').update(readFileSync(resolve(root,'builder/app/src/lib/assemble/pkg/obc_web_assemble_bg.wasm'))).digest('hex');
  result.patch_sha256=createHash('sha256').update(readFileSync(resolve(root,'host/obcm-assemble/dev/browser-cache/sequential.patch'))).digest('hex');
  result.profile_filesystem=execFileSync('findmnt',['-T',profile,'-n','-o','FSTYPE,SOURCE'],{encoding:'utf8'}).trim();
  result.browser=browser.browser().version();result.measured_at_utc=new Date().toISOString();
  result.note='Synthetic sequential reads through the shipping BlockCache and JsReads shim plus a diagnostic-only export. Fresh persistent OPFS file, initialized and flushed before timing. Warm host filesystem; no physical device throughput claim.';
  writeFileSync(output,JSON.stringify(result,null,2)+'\n');
} finally {await browser?.close();await server.close();rmSync(profile,{recursive:true,force:true});}
