import {test} from 'node:test';
import assert from 'node:assert/strict';
import {routeQuery} from '../routing.mjs';

test('routing adapter uses finite profiles, keeps snapped connectors explicit, and rejects incomplete legs',async t=>{
  const points=[[8,48],[8.1,48]];let sent;
  const route={geometry:[[8.001,48],[8.1,48]],legs:[{from_index:0,to_index:1}],snap_truncated:true};
  const fetch=t.mock.method(globalThis,'fetch',async(url,init)=>{
    sent=JSON.parse(init.body);assert.ok(url.endsWith('/v1/route'));
    return {ok:true,json:async()=>({routes:[route]})};
  });
  const result=await routeQuery({points,bike:'gravel',goal:'least_unpaved'});
  assert.equal(sent.profile,'gravel/smoother');assert.deepEqual(result.legs[0][0],points[0]);
  assert.equal(result.warnings.length,2);
  await assert.rejects(()=>routeQuery({points,bike:'gravel',goal:'most_climbing'}),/profile/);
  assert.equal(fetch.mock.callCount(),1);
  route.legs[0].to_index=500;
  await assert.rejects(()=>routeQuery({points,bike:'touring',goal:'balanced'}),/incomplete/);
});
