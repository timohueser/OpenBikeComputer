import assert from 'node:assert/strict';
import {DatabaseSync} from 'node:sqlite';
import {statSync} from 'node:fs';
import {pathToFileURL} from 'node:url';
import {search} from './web/engine.mjs';
import {openCells} from './cells.mjs';
import {reverseAddress} from './web/reverse.mjs';
import {searchCases,reverseCases} from './benchmark-cases.mjs';

const [file,reference] = process.argv.slice(2);
if (!file) throw new Error('Usage: node benchmark.mjs PACKAGE.sqlite [REFERENCE.sqlite]');
function open(file) {
  const conn = new DatabaseSync(file,{readOnly:true});
  return {conn,db:openCells([file])};
}
function summary(times) {
  times.sort((a,b)=>a-b);
  return {count:times.length,p50Ms:times[Math.floor(times.length*.5)],p95Ms:times[Math.ceil(times.length*.95)-1],maxMs:times.at(-1)};
}
const candidate = open(file), baseline = reference?open(reference):null;
const cases = searchCases(candidate.db), points = reverseCases(candidate.db);
if (baseline) {
  const count = sql => candidate.conn.prepare(sql).get().n;
  candidate.conn.prepare('ATTACH DATABASE ? AS reference').run(`${pathToFileURL(reference).href}?mode=ro`);
  for (const table of ['places','addresses']) {
    const columns = candidate.conn.prepare(`PRAGMA table_info(${table})`).all().map(c=>c.name);
    const key = table==='places'?'id':'rowid';
    const expected = count(`SELECT count(*) n FROM reference.${table}`);
    assert.equal(count(`SELECT count(*) n FROM main.${table}`),expected,`${table} count`);
    const equal = count(`SELECT count(*) n FROM main.${table} a JOIN reference.${table} b ON a.${key}=b.${key}
      WHERE ${columns.map(c=>`a.${c} IS b.${c}`).join(' AND ')}`);
    assert.equal(equal,expected,`${table} exact rows`);
  }
  for (const table of ['names','compact_names','lexicon']) for (const [a,b] of [['main','reference'],['reference','main']])
    assert.equal(count(`SELECT count(*) n FROM (SELECT * FROM ${a}.${table} EXCEPT SELECT * FROM ${b}.${table})`),0,table);
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
  const result = reverseAddress(candidate.db,point);
  times.candidate.reverse.push(performance.now()-start);
  if (baseline) {
    start = performance.now();
    const expected = reverseAddress(baseline.db,point);
    times.reference.reverse.push(performance.now()-start);
    assert.equal(result,expected,JSON.stringify(point));
  }
}
console.log(JSON.stringify({file,bytes:statSync(file).size,
  reference:reference||null,parity:baseline?'exact place/address rows, lexical entries, search results, reverse labels':null,
  timings:Object.fromEntries(Object.entries(times).filter(([k])=>k==='candidate'||baseline)
    .map(([k,v])=>[k,Object.fromEntries(Object.entries(v).map(([q,t])=>[q,summary(t)]))])),
  processMaxRssKiB:process.resourceUsage().maxRSS},null,2));
for (const package_ of [candidate,baseline].filter(Boolean)) { package_.db.close(); package_.conn.close(); }
