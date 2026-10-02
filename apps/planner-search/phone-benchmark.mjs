import {search} from './web/engine.mjs';
import {reverseAddress} from './web/reverse.mjs';
import {searchCases,reverseCases} from './benchmark-cases.mjs';

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value==='object') return Object.fromEntries(Object.keys(value).sort().map(k=>[k,canonical(value[k])]));
  return value;
}

export function run(all, digest) {
  const db = {all(sql,params=[]) {
    const result = JSON.parse(all(sql,JSON.stringify(params)));
    if (result.error) throw new Error(result.error);
    return result.rows;
  }};
  const samples = [];
  for (const input of searchCases(db)) {
    const result = search(db,input), elapsedMs = result.elapsed;
    delete result.elapsed;
    samples.push({kind:'search',input,elapsedMs,result,sha256:digest(JSON.stringify(canonical(result)))});
  }
  for (const coordinate of reverseCases(db)) {
    const started = performance.now(), result = reverseAddress(db,coordinate);
    samples.push({kind:'reverse',input:coordinate,elapsedMs:performance.now()-started,result,sha256:digest(JSON.stringify(result))});
  }
  return {schema:3,scope:'Shared lexical search and reverse lookup; excludes smart parsing and opening hours',samples};
}
