import {test} from 'node:test';
import assert from 'node:assert/strict';
import {routeQuery} from '../routing.mjs';

test('routing adapter uses finite profiles and sends the route answer on unchanged',async t=>{
  const points=[[8,48],[8.1,48]],answer={routes:[{coordinates_udeg:[8001000,48000000,99000,0]}]};let sent;
  const fetch=t.mock.method(globalThis,'fetch',async(url,init)=>{
    sent=JSON.parse(init.body);assert.ok(url.endsWith('/v1/route'));
    return {ok:true,json:async()=>answer};
  });
  assert.equal(await routeQuery({points,bike:'gravel',goal:'least_climbing'}),answer);
  assert.equal(sent.profile,'gravel/less-climbing');
  await assert.rejects(()=>routeQuery({points,bike:'gravel',goal:'most_climbing'}),/profile/);
  await assert.rejects(()=>routeQuery({points,bike:'gravel',goal:'least_unpaved'}),/least unpaved/);
  assert.equal(fetch.mock.callCount(),1);
});
