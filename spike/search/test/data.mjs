import {DatabaseSync} from 'node:sqlite';
import {performance} from 'node:perf_hooks';
import assert from 'node:assert/strict';
import {writeFile,mkdir,stat} from 'node:fs/promises';
import {search,DEFAULT_VIEW} from '../web/engine.mjs';
import {compact} from '../web/text.mjs';

const dbs=Object.fromEntries(['germany','baden-wuerttemberg'].map(name=>{
  const conn=new DatabaseSync(`data/${name}.sqlite`,{readOnly:true});
  conn.exec('PRAGMA cache_size=-32768; PRAGMA mmap_size=0;');
  return [name,{conn,all:(sql,bind=[])=>conn.prepare(sql).all(...bind)}];
}));
const cases=[
  {q:'bakery',check:r=>r.length>0&&r.every(p=>p.kind==='bakery'&&p.lon>=DEFAULT_VIEW[0]&&p.lon<=DEFAULT_VIEW[2])},
  {q:'Kandel',check:r=>r[0]?.kind==='summit'&&r.some(p=>p.source==='n1591343465'&&p.kind==='pass')&&r.filter(p=>p.kind==='street'&&p.name==='Kandel'&&p.distance<30).length===1},
  {q:'Feldberg',check:r=>r[0]?.source==='n26862857'&&r.some(p=>p.source==='r317609')},
  {q:'Feldberg (Schwarzwald)',check:r=>r[0]?.source==='r317609'},
  {q:'Kandel',view:[8.3,48.95,8.5,49.1],serverOnly:true,check:r=>r[0]?.kind==='city'},
  {q:'Freiburg',check:r=>r[0]?.name==='Freiburg im Breisgau'},
  {q:'Freibug',check:r=>r[0]?.name==='Freiburg im Breisgau'},
  {q:'Xreiburg',check:r=>r[0]?.name==='Freiburg im Breisgau'},
  ...['Munich','München','Muenchen','Munchne'].map(q=>({q,serverOnly:true,check:r=>r[0]?.name==='München'&&r[0]?.kind==='city'})),
  {q:'Deutsches Museum München',serverOnly:true,check:r=>r[0]?.name==='Deutsches Museum'&&r[0]?.kind==='museum'},
  {q:'Platz der Republik 1 Berlin',serverOnly:true,check:r=>r[0]?.precision==='house'&&r[0]?.city==='Berlin'},
  {q:'Kaiser Joseph Straße 242 Freiburg',check:r=>r[0]?.precision==='house'},
  {q:'Habsburgerstr. 10 Freiburg',check:r=>r[0]?.source==='w154330310'&&r[0]?.precision==='house'},
  {q:'Habsburgerstr 10',check:r=>r[0]?.city==='Freiburg im Breisgau'&&r[0]?.precision==='house'},
  ...['Media Markt','Media-Markt','NediaMarkt','Mdeia Mrkt','Media Markt Freiburg'].map(q=>({q,check:r=>r[0]?.source==='n809686332'})),
  ...['MediaMarkt','Media Markt'].map(q=>({q,view:[7.76,48.08,7.86,48.16],check:r=>{
    const shops=r.filter(p=>p.kind==='electronics'&&compact(p.name)==='mediamarkt');
    return shops[0]?.source==='n414000115'&&shops[1]?.source==='n809686332'&&shops[2]?.source==='w143665549'
      &&shops.every((p,i)=>!i||shops[i-1].distance<=p.distance);
  }})),
  {q:'Media Markt München',serverOnly:true,check:r=>r[0]?.city==='München'&&r[0]?.kind==='electronics'},
  {q:'Kaiser Joseph Straße 9999 Freiburg',check:r=>r[0]?.precision==='street'},
  {q:'bakeries in Munich',serverOnly:true,request:{type:'places',what:['bakery'],where:{near:'Munich',in:true}},check:r=>r.length>0&&r.every(p=>p.city==='München')},
  ...['pizza','Döner'].map(q=>({q,check:r=>r.length>0&&r.every(p=>['restaurant','fast_food','cafe','pub','bar'].includes(p.kind))&&r.some(p=>p.why.match.includes('OSM cuisine tag'))})),
  {q:'Döner in Teningen',request:{type:'places',what:['kebab'],where:{near:'Teningen',in:true}},check:r=>r.length>0&&r.every(p=>p.city==='Teningen'&&(p.cuisine.includes('kebab')||/döner|kebap|kebab/i.test(p.name)))},
];
const results=[];
for(const [name,db]of Object.entries(dbs))for(const c of cases) {
  if(name!=='germany'&&c.serverOnly)continue;
  const input={...c,view:c.view||DEFAULT_VIEW};delete input.check;
  const r=search(db,input);
  assert.ok(c.check(r.results),`${name}: ${c.q}: ${JSON.stringify(r.results.slice(0,2).map(p=>[p.name,p.kind]))}`);
  results.push({data:name,q:c.q,ms:r.elapsed,first:r.results[0]?.name});
}
console.log(`PASS ${results.length} real-data acceptance cases`);
const timing=[];
for(let n=0;n<3;n++)for(const c of cases) {
  const r=search(dbs.germany,{...c,view:c.view||DEFAULT_VIEW});timing.push(r.elapsed);
}
timing.sort((a,b)=>a-b);
const counts=Object.fromEntries(Object.entries(dbs).map(([name,db])=>[name,{
  places:db.all('SELECT count(*) n FROM places')[0].n,
  addresses:db.all('SELECT count(*) n FROM addresses')[0].n,
}]));
for(const db of Object.values(dbs))db.conn.close();
const before=performance.now();
await Promise.all(cases.slice(0,4).map(async c=>{
  const r=await fetch('http://localhost:8780/api/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({q:c.q,view:c.view||DEFAULT_VIEW})});
  assert.equal(r.status,200);
}));
const concurrentFourMs=performance.now()-before;
const server=await fetch('http://localhost:8780/api/status').then(r=>r.json());
const report={counts,firstPass:results,warm:{queries:timing.length,medianMs:timing[Math.floor(timing.length/2)],p95Ms:timing[Math.ceil(timing.length*.95)-1]},
  concurrentFourMs,serverRss:server.rss,germanyBytes:server.germanyBytes,offlineBytes:server.offlineBytes,
  downloadBytes:(await stat('data/baden-wuerttemberg.sqlite.gz')).size};
await mkdir('test-output',{recursive:true});
await writeFile('test-output/data.json',JSON.stringify(report,null,2));
console.log(JSON.stringify({...report,firstPass:undefined},null,2));
