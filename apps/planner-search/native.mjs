import {routeRequest} from './routing.mjs';
import {searchRuntime} from './runtime.mjs';

const response = text => {
  const value = JSON.parse(text);
  if (value.error) throw new Error(value.error);
  return value;
};

/** Native capabilities exchange JSON; all query algorithms stay in the shared runtime. */
export function nativeSearch({all,batch,parse,hours,region,bytes,bounds}) {
  const db = {all:(sql,params=[],options={})=>response(all(sql,JSON.stringify(params),JSON.stringify(options))).rows};
  if(batch)db.candidates=queries=>response(batch(JSON.stringify(queries))).rows;
  const metadata = Object.fromEntries(db.all('SELECT * FROM metadata').map(row=>[row.key,JSON.parse(row.value)]));
  if (metadata.schema !== 4) throw new Error('Rebuild incompatible search data.');
  if (bounds) { metadata.bounds = bounds; delete metadata.counts; }
  const runtime = searchRuntime({db,parser:{parse:text=>response(parse(text))},hours,region,attribution:metadata.attribution});
  return {
    async request(method,input) {
      if (method === 'status') return {parser:{ready:true,message:''},regions:[{id:region,metadata,bytes}]};
      if (method === 'route-request') return routeRequest(input);
      if (input?.region && input.region !== region) throw new Error('Search does not cover this region.');
      if (method === 'query') return runtime.query(input);
      if (method === 'reverse') return runtime.reverse(input.coordinate);
      throw new Error('Unknown planner search method.');
    },
  };
}
