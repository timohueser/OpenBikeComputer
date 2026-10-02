import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {routeLine,routeQuery} from '../routing.mjs';

test('route line decodes the shared answer vector exactly',()=>{
  const vector=JSON.parse(readFileSync(new URL('../../../specs/vectors/route-answer.json',import.meta.url),'utf8'));
  assert.deepEqual(routeLine(vector.answer.routes[0]),vector.route.geometry);
});

test('routing adapter uses finite profiles, keeps snapped connectors explicit, and rejects incomplete legs',async t=>{
  const points=[[8,48],[8.1,48]];let sent;
  const route={coordinates_udeg:[8001000,48000000,99000,0],legs:[{from_index:0,to_index:1}],snap_truncated:true};
  const fetch=t.mock.method(globalThis,'fetch',async(url,init)=>{
    sent=JSON.parse(init.body);assert.ok(url.endsWith('/v1/route'));
    return {ok:true,json:async()=>({routes:[route]})};
  });
  const result=await routeQuery({points,bike:'gravel',goal:'least_climbing'});
  assert.equal(sent.profile,'gravel/less-climbing');assert.deepEqual(result.legs[0][0],points[0]);
  assert.equal(result.warnings.length,2);
  await assert.rejects(()=>routeQuery({points,bike:'gravel',goal:'most_climbing'}),/profile/);
  await assert.rejects(()=>routeQuery({points,bike:'gravel',goal:'least_unpaved'}),/least unpaved/);
  assert.equal(fetch.mock.callCount(),1);
  route.legs[0].to_index=500;
  await assert.rejects(()=>routeQuery({points,bike:'touring',goal:'balanced'}),/incomplete/);
});
