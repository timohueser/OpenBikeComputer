import http from 'node:http';
import {DatabaseSync} from 'node:sqlite';
import {createReadStream, statSync, existsSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
import {search} from './web/engine.mjs';

const root=path.dirname(fileURLToPath(import.meta.url));
const dbPath=path.join(root,'data/germany.sqlite');
if(!existsSync(dbPath)) throw new Error('Build Germany data first. See README.md.');
const conn=new DatabaseSync(dbPath,{readOnly:true});
conn.exec('PRAGMA cache_size=-32768; PRAGMA mmap_size=0; PRAGMA query_only=ON;');
const cache=new Map();
const db={all(sql,bind=[]) {
  if(!cache.has(sql)) { if(cache.size>100)cache.clear(); cache.set(sql,conn.prepare(sql)); }
  return cache.get(sql).all(...bind);
}};
const metadata=Object.fromEntries(db.all('SELECT * FROM metadata').map(r=>[r.key,JSON.parse(r.value)]));
const mime={'.html':'text/html','.css':'text/css','.js':'text/javascript','.mjs':'text/javascript',
  '.json':'application/json','.geojson':'application/geo+json','.wasm':'application/wasm','.svg':'image/svg+xml',
  '.woff2':'font/woff2','.png':'image/png'};

const server=http.createServer(async(req,res)=>{
  const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'credentialless'};
  const json=(status,value)=>{res.writeHead(status,{...headers,'Content-Type':'application/json'});res.end(JSON.stringify(value));};
  try {
    const url=new URL(req.url,'http://localhost');
    if(url.pathname==='/api/search' && req.method==='POST') {
      let body='';
      for await(const chunk of req) {body+=chunk; if(body.length>200000) {json(413,{error:'Request too large'});return;}}
      const input=JSON.parse(body);
      if(input.view && (input.view.length!==4 || !input.view.every(Number.isFinite))) throw new Error('Invalid map bounds');
      const out=search(db,input);
      json(200,{...out,source:'server'});return;
    }
    if(url.pathname==='/api/status') {
      const offline=path.join(root,'data/baden-wuerttemberg.sqlite');
      json(200,{metadata,germanyBytes:statSync(dbPath).size,offlineBytes:existsSync(offline)?statSync(offline).size:0,
        downloadBytes:existsSync(offline+'.gz')?statSync(offline+'.gz').size:statSync(offline).size,
        rss:process.memoryUsage().rss,node:process.version});return;
    }
    let file;
    if(url.pathname==='/data/baden-wuerttemberg.sqlite') file=path.join(root,url.pathname);
    else {
      file=path.resolve(root,'web','.'+(url.pathname==='/'?'/index.html':decodeURIComponent(url.pathname)));
      if(!file.startsWith(path.join(root,'web')+path.sep)) {res.writeHead(403);res.end();return;}
    }
    if(!existsSync(file)||!statSync(file).isFile()) {res.writeHead(404);res.end('Not found');return;}
    if(url.pathname==='/data/baden-wuerttemberg.sqlite' && existsSync(file+'.gz')) {
      headers['Content-Encoding']='gzip';headers['X-Uncompressed-Length']=String(statSync(file).size);file+='.gz';
    }
    res.writeHead(200,{...headers,'Content-Type':mime[path.extname(file)]||'application/octet-stream',
      'Content-Length':statSync(file).size,'Cache-Control':'no-cache'});
    createReadStream(file).pipe(res);
  } catch(error) { json(400,{error:error.message}); }
});
const port=Number(process.env.PORT||8780);
server.listen(port,'127.0.0.1',()=>console.log(`Search playground: http://localhost:${port}`));
