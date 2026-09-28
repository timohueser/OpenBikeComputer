const CACHE='obc-search-spike-v1';
const SHELL=['/','/index.html','/app.mjs','/style.css','/engine.mjs','/text.mjs','/address-terms.json','/db-worker.mjs','/parser-worker.mjs',
  '/labels.json','/kinds.json','/regions.geojson','/route.json','/vendor/atkinson.woff2',
  '/vendor/leaflet/leaflet.js','/vendor/leaflet/leaflet.css',
  '/vendor/sqlite/index.mjs','/vendor/sqlite/sqlite3.wasm',
  '/vendor/tokenizers/tokenizers.min.mjs','/vendor/ort/ort.wasm.min.mjs',
  '/vendor/ort/ort-wasm-simd-threaded.mjs','/vendor/ort/ort-wasm-simd-threaded.wasm'];
self.addEventListener('install',event=>event.waitUntil(caches.open(CACHE).then(c=>c.addAll(SHELL)).then(()=>self.skipWaiting())));
self.addEventListener('activate',event=>event.waitUntil(self.clients.claim()));
self.addEventListener('fetch',event=>{
  const url=new URL(event.request.url);
  if(event.request.method!=='GET'||url.origin!==self.location.origin||url.pathname.startsWith('/api/')||url.pathname.startsWith('/data/'))return;
  event.respondWith(caches.open(CACHE).then(async cache=>{
    const saved=await cache.match(event.request);
    if(saved&&url.pathname.startsWith('/vendor/'))return saved;
    try {
      const response=await fetch(event.request);
      if(response.ok)await cache.put(event.request,response.clone());
      return response;
    } catch(error) {if(saved)return saved;throw error;}
  }));
});
