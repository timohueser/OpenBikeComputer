import assert from 'node:assert/strict';
import {DatabaseSync} from 'node:sqlite';
import {statSync} from 'node:fs';
import {pathToFileURL} from 'node:url';
import {search,around,distance} from './web/engine.mjs';
import {reverseAddress} from './web/reverse.mjs';
import {searchCases,reverseCases} from './benchmark-cases.mjs';

const [file,reference] = process.argv.slice(2);
if (!file) throw new Error('Usage: node benchmark.mjs PACKAGE.sqlite [REFERENCE.sqlite]');
function open(file) {
  const conn = new DatabaseSync(file,{readOnly:true});
  conn.exec('PRAGMA cache_size=-32768; PRAGMA mmap_size=0; PRAGMA temp_store=FILE');
  const statements = new Map();
  const db = {all(sql,params=[]) {
    if (!statements.has(sql)) statements.set(sql,conn.prepare(sql));
    return statements.get(sql).all(...params);
  }};
  const schema = JSON.parse(db.all("SELECT value FROM metadata WHERE key='schema'")[0].value);
  return {conn,db,schema};
}
function reverse(package_,point) {
  if (package_.schema===4) return reverseAddress(package_.db,point);
  const candidates = package_.db.all(`SELECT a.house,a.lon,a.lat,p.name,p.city,a.source
    FROM address_spatial b CROSS JOIN addresses a ON a.rowid=b.id JOIN places p ON p.id=a.street_id
    WHERE b.east>=? AND b.north>=? AND b.west<=? AND b.south<=?`,around(point,.1));
  const nearest = candidates.map(a=>({...a,distance:distance(point,[a.lon,a.lat])}))
    .filter(a=>a.distance<=.1).sort((a,b)=>a.distance-b.distance||a.source.localeCompare(b.source))[0];
  return nearest?`${nearest.name} ${nearest.house}${nearest.city?`, ${nearest.city}`:''}`:null;
}
function summary(times) {
  times.sort((a,b)=>a-b);
  return {count:times.length,p50Ms:times[Math.floor(times.length*.5)],p95Ms:times[Math.ceil(times.length*.95)-1],maxMs:times.at(-1)};
}
const candidate = open(file), baseline = reference?open(reference):null;
const cases = searchCases(candidate.db), points = reverseCases(candidate.db);
if (baseline) {
  candidate.conn.prepare('ATTACH DATABASE ? AS reference').run(`${pathToFileURL(reference).href}?mode=ro`);
  for (const table of ['places','addresses']) {
    const columns = candidate.db.all(`PRAGMA table_info(${table})`).map(c=>c.name);
    const key = table==='places'?'id':'rowid';
    const expected = candidate.db.all(`SELECT count(*) n FROM reference.${table}`)[0].n;
    assert.equal(candidate.db.all(`SELECT count(*) n FROM main.${table}`)[0].n,expected,`${table} count`);
    const equal = candidate.db.all(`SELECT count(*) n FROM main.${table} a JOIN reference.${table} b ON a.${key}=b.${key}
      WHERE ${columns.map(c=>`a.${c} IS b.${c}`).join(' AND ')}`)[0].n;
    assert.equal(equal,expected,`${table} exact rows`);
  }
  for (const table of ['names','compact_names','lexicon']) for (const [a,b] of [['main','reference'],['reference','main']])
    assert.equal(candidate.db.all(`SELECT count(*) n FROM (SELECT * FROM ${a}.${table} EXCEPT SELECT * FROM ${b}.${table})`)[0].n,0,table);
}
const times = {candidate:{search:[],reverse:[]},reference:{search:[],reverse:[]}};
for (const input of cases) {
  const result = search(candidate.db,input);
  times.candidate.search.push(result.elapsed);
  if (baseline) {
    const expected = search(baseline.db,input);
    times.reference.search.push(expected.elapsed);
    delete result.elapsed; delete expected.elapsed;
    assert.deepEqual(result,expected,input.q);
  }
}
for (const point of points) {
  let start = performance.now();
  const result = reverse(candidate,point);
  times.candidate.reverse.push(performance.now()-start);
  if (baseline) {
    start = performance.now();
    const expected = reverse(baseline,point);
    times.reference.reverse.push(performance.now()-start);
    assert.equal(result,expected,JSON.stringify(point));
  }
}
console.log(JSON.stringify({file,bytes:statSync(file).size,schema:candidate.schema,
  reference:reference||null,parity:baseline?'exact place/address rows, lexical entries, search results, reverse labels':null,
  timings:Object.fromEntries(Object.entries(times).filter(([k])=>k==='candidate'||baseline)
    .map(([k,v])=>[k,Object.fromEntries(Object.entries(v).map(([q,t])=>[q,summary(t)]))])),
  processMaxRssKiB:process.resourceUsage().maxRSS},null,2));
candidate.conn.close(); baseline?.conn.close();
