import {REGION_BOUNDS,rank,simpleRequest,distinct} from './engine.mjs';

const $=id=>document.getElementById(id), escape=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const size=n=>`${(n/1e6).toFixed(0)} MB`;
function client(url,progress) {
  const worker=new Worker(url,{type:'module'}),pending=new Map();let id=0;
  worker.onmessage=({data})=>{
    if(data.progress!==undefined){progress?.(data);return;}
    const p=pending.get(data.id);if(!p)return;pending.delete(data.id);
    data.error?p.reject(new Error(data.error)):p.resolve(data.result);
  };
  worker.onerror=e=>{for(const p of pending.values())p.reject(new Error(e.message));pending.clear();};
  return (action,input={})=>new Promise((resolve,reject)=>{pending.set(++id,{resolve,reject});worker.postMessage({id,action,...input});});
}
const offline=client('./db-worker.mjs',({progress,total})=>{
  $('progress').value=progress/total;$('download-copy').textContent=`Downloading search data: ${size(progress)} of ${size(total)}`;
});
const parser=client('./parser-worker.mjs');
let installed=false,here=null,plan=null,limit=20,version=0,lastOutput=null,routeLayers=[],markers=[],currentResults=[];
let controller,debounce,suppressMove=false;
const map=L.map('map',{zoomControl:false}).setView([47.995,7.849],13);
L.control.zoom({position:'topright'}).addTo(map);
const tiles=L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png',{maxZoom:18,attribution:'© <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'}).addTo(map);
map.attributionControl.addAttribution('Search: OSM / Nominatim / Photon');
let outlines;
fetch('/regions.geojson').then(r=>r.json()).then(data=>{
  outlines=L.geoJSON(data,{style:f=>({color:f.properties.name==='Baden-Württemberg'?'#4a6b2f':'#7c8268',weight:1,fillColor:'#dbe4cf',fillOpacity:.12}),
    onEachFeature:(f,l)=>l.bindTooltip(f.properties.name)}).addTo(map);
  outlines.bringToBack();
}).catch(()=>{});

function view() {const b=map.getBounds();return [b.getWest(),b.getSouth(),b.getEast(),b.getNorth()];}
function connected() {return $('online').checked&&navigator.onLine;}
function connection() {
  if(connected()){if(!map.hasLayer(tiles))tiles.addTo(map);}
  else if(map.hasLayer(tiles))map.removeLayer(tiles);
  $('map-note').textContent=connected()?'Categories search the visible map. Place names search all available data.':'Offline search · Detailed map tiles need a connection in this spike.';
}
function offlineState() {
  $('offline-state').textContent=installed?'Ready on this browser':'Not downloaded';
  $('download').textContent=installed?'Download again':'Download Baden-Württemberg';
  if(installed){$('download-copy').textContent='Baden-Württemberg is stored here. Switch off the Germany server to try offline search.';$('offline-details').open=false;}
}
offline('status').then(r=>{installed=r.installed;offlineState();}).catch(e=>{$('storage-note').textContent=`Offline storage: ${e.message}`;});
let serverStatus;
fetch('/api/status').then(r=>r.json()).then(r=>{
  serverStatus=r;
  if(!installed)$('download-copy').textContent=`Baden-Württemberg: ${size(r.downloadBytes)} download, ${size(r.offlineBytes)} installed, plus the query model. Germany stays on the server.`;
}).catch(()=>{$('online').checked=false;connection();});
if('serviceWorker'in navigator) {
  await navigator.serviceWorker.register('/sw.js');
  await navigator.serviceWorker.ready;
}
$('download').onclick=async()=>{
  $('download').disabled=true;$('progress').hidden=false;
  try {
    await navigator.storage?.persist?.();
    const sw=navigator.serviceWorker;
    if(sw&&!sw.controller)await new Promise(resolve=>sw.addEventListener('controllerchange',resolve,{once:true}));
    await offline('install');
    $('download-copy').textContent='Loading the offline query model…';
    const model=await parser('load');
    if(!model.loaded)throw new Error(model.notice||'Query model could not load.');
    installed=true;offlineState();$('status').textContent='Offline search is ready. Try switching off the Germany server.';
    if($('query').value)await runSearch();
  }catch(e){$('download-copy').textContent=e.message;}
  finally{$('download').disabled=false;$('progress').hidden=true;}
};

function merge(local,remote,count=limit) {
  const byId=new Map();
  for(const [out,source]of [[remote,'Server'],[local,'Offline']])for(const p of out?.results||[]) {
    const old=byId.get(p.source);byId.set(p.source,{...(old||{}),...p,served:old?'Offline + server':source});
  }
  const ranked=distinct(rank([...byId.values()]));
  return {...(remote||local),results:ranked.slice(0,count),hasMore:!!(local?.hasMore||remote?.hasMore||ranked.length>count),
    timings:{local:local?.elapsed,server:remote?.elapsed},localAvailable:!local?.unavailable};
}

async function runSearch(suggest=false) {
  const q=$('query').value.trim();if(!q)return;
  const mine=++version,start=performance.now();controller?.abort();controller=new AbortController();
  $('status').textContent='Searching…';
  try {
    const request=suggest?simpleRequest(q):await parser('parse',{q});
    if(mine!==version)return;
    const input={q,view:view(),here,plan,request,limit:suggest?6:limit};
    let local,remote,remoteError;
    const localJob=offline('search',{input}).then(out=>{
      local=out;
    if(mine===version&&out.results?.length)render({...merge(local,null,input.limit),request,searchMs:performance.now()-start,pending:connected()});
    }).catch(e=>{local={results:[],unavailable:true,note:e.message};});
    const remoteJob=connected()?fetch('/api/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(input),signal:controller.signal})
      .then(async r=>{const out=await r.json();if(!r.ok)throw new Error(out.error);remote=out;})
      .catch(e=>{if(e.name!=='AbortError')remoteError=e.message;}):Promise.resolve();
    await Promise.all([localJob,remoteJob]);
    if(mine!==version)return;
    const out={...merge(local,remote,input.limit),request,searchMs:performance.now()-start,remoteError,suggest};
    if(!connected()||remoteError) {
      const b=view(),bw=REGION_BOUNDS;
      const partlyOutside=b[0]<bw[0]||b[1]<bw[1]||b[2]>bw[2]||b[3]>bw[3];
      if(!installed)out.note='No offline search data is installed. Connect and download Baden-Württemberg.';
      else if(out.unresolved||partlyOutside)out.note='Offline coverage is Baden-Württemberg only. Places outside it need the Germany server.';
      else if(request.type==='place')out.note='Searching downloaded Baden-Württemberg. Other regions need a connection.';
    }
    const where=request.where||{};
    if(!suggest&&out.results?.length&&out.resolved?.bounds&&(where.near||where.day||where.scope==='route'||where.scope==='here')) {
      const b=out.resolved.bounds;suppressMove=true;
      map.fitBounds([[b[1],b[0]],[b[3],b[2]]],{padding:[35,70],animate:false});suppressMove=false;
    }
    render(out);lastOutput=out;
  }catch(e){if(mine===version)$('status').textContent=`Search failed: ${e.message}`;}
}

function render(out) {
  currentResults=out.results||[];
  document.body.classList.toggle('has-results',currentResults.length>0);
  $('scope').textContent=out.area||'Downloaded Baden-Württemberg';
  let text=currentResults.length?`${currentResults.length}${out.hasMore?'+':''} ${out.suggest?'suggestions':'results'} · ${Math.round(out.searchMs)} ms`:'No matching places found.';
  if(out.pending)text+=' · Checking Germany…';
  if(out.note)text+=' '+out.note;
  if(out.remoteError)text+=' Server unavailable; showing downloaded data.';
  if(out.request?.notice)text+=' '+out.request.notice;
  if(out.request?.ignored?.length)text+=' Not understood: '+out.request.ignored.join(', ');
  $('status').textContent=text;
  $('more').hidden=!out.hasMore||out.suggest||limit>=100;
  $('results').innerHTML=currentResults.map((p,i)=>{
    const km=p.position?`${p.position.along.toFixed(1)} km along route · ${p.position.distance.toFixed(1)} km from line`:
      `${p.distance<1?Math.round(p.distance*1000)+' m':p.distance.toFixed(1)+' km'} straight line`;
    return `<button class="result" data-result="${i}"><span class="result-num">${i+1}</span><span><strong>${escape(p.name)}</strong><span class="description">${escape(p.kind.replaceAll('_',' '))} · ${escape([p.city,p.postcode].filter(Boolean).join(' '))}${p.precision==='street'?' · Street only':''}</span><span class="distance">${escape(km)}<span class="source">${escape(p.served)}</span></span></span></button>`;
  }).join('');
  drawMarkers();
  $('inspect').textContent=JSON.stringify({query:$('query').value,request:out.request,area:out.area,
    timingMs:out.timings,firstResults:currentResults.slice(0,5).map(p=>({name:p.name,score:p.score,why:p.why,source:p.source})),
    serverBytes:serverStatus?.germanyBytes,offlineBytes:serverStatus?.offlineBytes},null,2);
}
function drawMarkers() {
  for(const m of new Set(markers))map.removeLayer(m);markers=[];
  const groups=[];
  currentResults.forEach((p,i)=>{
    const xy=map.latLngToContainerPoint([p.lat,p.lon]);
    const group=groups.find(g=>g.xy.distanceTo(xy)<36);
    if(group)group.items.push({p,i});else groups.push({xy,items:[{p,i}]});
  });
  for(const g of groups) {
    const items=g.items,cluster=items.length>1;
    const lat=items.reduce((s,x)=>s+x.p.lat,0)/items.length,lon=items.reduce((s,x)=>s+x.p.lon,0)/items.length;
    const marker=L.marker([lat,lon],{icon:L.divIcon({className:cluster?'pin cluster':'pin',
      html:String(cluster?items.length:items[0].i+1),iconSize:cluster?[34,34]:[28,28]})}).addTo(map);
    if(cluster)marker.bindTooltip(`${items.length} results · Zoom in`).on('click',()=>map.fitBounds(items.map(x=>[x.p.lat,x.p.lon]),{padding:[70,70],maxZoom:18}));
    else marker.on('click',()=>select(items[0].i,false));
    for(const x of items)markers[x.i]=marker;
  }
}
map.on('zoomend',drawMarkers);
function select(i,pan=true) {
  const p=currentResults[i];if(!p)return;
  document.querySelectorAll('.result').forEach((e,j)=>e.classList.toggle('selected',i===j));
  if(pan){suppressMove=true;map.setView([p.lat,p.lon],Math.max(map.getZoom(),14),{animate:false});suppressMove=false;}
  const source=/^[nwr]\d+$/.test(p.source)?`<a href="https://www.openstreetmap.org/${({n:'node',w:'way',r:'relation'})[p.source[0]]}/${p.source.slice(1)}" target="_blank" rel="noopener">View OSM record</a>`:'';
  markers[i].bindPopup(`<strong>${escape(p.name)}</strong><br>${escape(p.city)}<br>${p.lat.toFixed(5)}, ${p.lon.toFixed(5)}<br>${source}`).openPopup();
  $('inspect').textContent=JSON.stringify({name:p.name,score:p.score,why:p.why,precision:p.precision,source:p.source},null,2);
}
$('results').onclick=e=>{const b=e.target.closest('[data-result]');if(b)select(Number(b.dataset.result));};
$('search-form').onsubmit=e=>{e.preventDefault();clearTimeout(debounce);limit=20;$('query').blur();runSearch();};
$('query').oninput=()=>{
  clearTimeout(debounce);++version;controller?.abort();
  if($('query').value.trim().length>=2)debounce=setTimeout(()=>runSearch(true),300);
  else if(!$('query').value.trim()) {
    currentResults=[];lastOutput=null;drawMarkers();$('results').replaceChildren();$('more').hidden=true;
    $('status').textContent='Type a name or try an example.';$('scope').textContent='Search the map, or find a place anywhere in Germany.';
    document.body.classList.remove('has-results');
  }
};
$('query').onfocus=()=>document.body.classList.add('typing');
$('query').onblur=()=>{document.body.classList.remove('typing');map.invalidateSize();};
document.querySelectorAll('[data-query]').forEach(b=>b.onclick=()=>{$('query').value=b.dataset.query;limit=20;runSearch();});
$('more').onclick=()=>{limit=Math.min(limit+20,100);runSearch();};
$('online').onchange=()=>{connection();if($('query').value)runSearch();};
window.addEventListener('online',connection);window.addEventListener('offline',()=>{connection();if($('query').value)runSearch();});
const views={freiburg:[[47.995,7.849],13],munich:[[48.137,11.576],13],germany:[[51,10.3],6]};
document.querySelectorAll('[data-view]').forEach(b=>b.onclick=()=>map.setView(...views[b.dataset.view]));
map.on('moveend',()=>{if(!suppressMove&&lastOutput?.request?.type==='places'&&lastOutput.request.where?.scope==='view'&&!lastOutput.request.where?.near)runSearch();});
$('locate').onclick=()=>{
  if(!navigator.geolocation){$('status').textContent='Location is unavailable in this browser.';return;}
  navigator.geolocation.getCurrentPosition(p=>{here=[p.coords.longitude,p.coords.latitude];map.setView([here[1],here[0]],14);$('status').textContent='Location set. “Near me” now uses this position.';},
    e=>{$('status').textContent=`Could not get your location: ${e.message}`;});
};
function setPlan(data) {
  plan=data;for(const l of routeLayers)map.removeLayer(l);routeLayers=[];
  const colors=['#cc2a93','#2f6fb5','#3b8a3f'];let from=0;
  plan.days.forEach((end,i)=>{
    const coords=plan.coordinates.slice(from,end+1).map(p=>[p[1],p[0]]);
    routeLayers.push(L.polyline(coords,{color:colors[i%3],weight:4}).addTo(map));
    routeLayers.push(L.marker(coords.at(-1),{icon:L.divIcon({className:'day-marker',html:`${i+1}`,iconSize:[25,25]})}).bindTooltip(`End of Day ${i+1}`).addTo(map));from=end;
  });
  map.fitBounds(L.latLngBounds(plan.coordinates.map(p=>[p[1],p[0]])),{padding:[40,60]});
  $('clear-route').hidden=false;$('sample-route').textContent='Search Day 3 end';
  $('status').textContent='Route loaded with three illustrative days. Try “bakeries at the end of day three”.';
}
$('sample-route').onclick=async()=>{
  if(plan){$('query').value='bakeries at the end of day three';runSearch();}
  else setPlan(await fetch('/route.json').then(r=>r.json()));
};
$('open-gpx').onclick=()=>$('gpx').click();
$('gpx').onchange=async e=>{
  try{
    const xml=new DOMParser().parseFromString(await e.target.files[0].text(),'application/xml');
    const points=[...xml.querySelectorAll('trkpt,rtept')].map(p=>[Number(p.getAttribute('lon')),Number(p.getAttribute('lat'))]);
    if(points.length<2||!points.every(p=>p.every(Number.isFinite)))throw new Error('The file has no usable track.');
    const stride=Math.max(1,Math.ceil(points.length/2500)),coordinates=points.filter((_,i)=>i%stride===0||i===points.length-1);
    setPlan({coordinates,days:[Math.floor(coordinates.length/3),Math.floor(coordinates.length*2/3),coordinates.length-1]});
  }catch(e){$('status').textContent=`Could not load GPX: ${e.message}`;}
};
$('clear-route').onclick=()=>{plan=null;routeLayers.forEach(l=>map.removeLayer(l));routeLayers=[];$('clear-route').hidden=true;$('sample-route').textContent='Load sample route';};
// Read-only handles for repeatable browser verification.
window.searchSpike={runSearch,offline,parser,map,get results(){return currentResults;},get output(){return lastOutput;}};
