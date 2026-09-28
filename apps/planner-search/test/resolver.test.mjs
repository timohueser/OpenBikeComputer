import {test} from 'node:test';
import assert from 'node:assert/strict';
import {database} from './database.mjs';
import {resolve,findPlaces} from '../resolver.mjs';
import {lengths,at,alongRange} from '../web/geography.mjs';
import {openingState} from '../hours.mjs';
import {validateInput} from '../validation.mjs';
const {db,conn}=database();
conn.exec('ALTER TABLE places ADD COLUMN opening_hours TEXT');
conn.exec("UPDATE places SET opening_hours='Mo-Sa 08:00-18:00; Su 08:00-12:00' WHERE source='n1'");
const line=[[7.82,47.99],[7.85,47.99],[7.87,47.99],[7.9,47.99]],ds=lengths(line),total=ds.at(-1);
const context={q:'bakery',view:[7.8,47.9,7.95,48.05],plan:{coordinates:line,days:[{number:1,from:0,to:total/2},{number:2,from:total/2,to:total}],points:[]},limit:20};

test('explicit words win over pointing; day ends and middle use actual line distance',()=>{
  const out=findPlaces(db,{type:'places',what:['bakery'],where:{day:1,part:'end'},radius:{value:1,unit:'km'}},{...context,pointing:{anchor:[11.576,48.14]}});
  assert.ok(out.results.length);assert.ok(out.results.every(p=>p.city==='Freiburg'));
  assert.match(out.area,/Day 1 end/);
  const middle=findPlaces(db,{type:'places',what:['bakery'],where:{day:2,part:'middle'}},context);
  assert.match(middle.area,/Day 2 middle/);
});
test('before, after and mixed route intervals constrain the complete route position',()=>{
  const out=findPlaces(db,{type:'places',what:['bakery'],where:{after:{along:{ref:'km',at:{value:ds[1],unit:'km'}}},before:{along:{ref:'km',at:{value:ds[2],unit:'km'}}}}},context);
  assert.deepEqual(new Set(out.results.map(p=>p.source)),new Set(['n1','n2']));
  assert.throws(()=>findPlaces(db,{type:'places',what:['bakery'],where:{day:8}},context),/no Day 8/);
  assert.throws(()=>findPlaces(db,{type:'places',what:['bakery'],where:{scope:'here'}},context),/location/);
});
test('an empty viewport widens visibly, an explicit interval does not',()=>{
  const out=findPlaces(db,{type:'places',what:['bakery']},{...context,view:[7.79,47.98,7.80,47.99]});
  assert.ok(out.results.length);assert.match(out.note,/widened/);
  const explicit=findPlaces(db,{type:'places',what:['bakery'],where:{anchor:[0,0]}},context);
  assert.equal(explicit.results.length,0);assert.doesNotMatch(explicit.note,/widened/);
});
test('opening filters exclude unknown tags and require a dated trip for day filters',()=>{
  const out=findPlaces(db,{type:'places',what:['bakery'],open:{weekday:'sun'}},context);
  assert.deepEqual(out.results.map(p=>p.source),['n1']);assert.match(out.note,/unknown opening/);
  assert.throws(()=>findPlaces(db,{type:'places',what:['bakery'],open:{day:1}},context),/start date/);
  assert.equal(openingState({opening_hours:'Mo-Su 08:00-18:00; PH off',lat:48,lon:8,region:'Baden-Württemberg'},{weekday:'sun'},{}),'unknown');
  assert.equal(openingState({opening_hours:'24/7',lat:48,lon:8},{now:true},{now:'2026-09-28T06:00:00Z'}),'open');
});
test('route and plan mutations return reviewable commands without changing context',()=>{
  const before=JSON.stringify(context);
  const r=resolve(db,{type:'route',from:{plan:'start'},to:{name:'Kandel'}},context);
  assert.equal(r.type,'change');assert.equal(r.changes[0].points.length,2);
  assert.equal(resolve(db,{type:'reverse'},context).changes[0].op,'reverse');
  assert.equal(resolve(db,{type:'join',day:1},context).changes[0].day,1);
  assert.equal(JSON.stringify(context),before);
});
test('route gaps are computed from mapped positions and missing attribute data stays unknown',()=>{
  const out=resolve(db,{type:'stretches',what:'gap:bakery'},context);
  assert.ok(out.stretches.length>0);assert.match(out.note,/Missing map data/);
  assert.throws(()=>resolve(db,{type:'stretches',what:'unpaved'},context),/no verified/);
});
test('time positions require a monotone time profile, and end offsets run backwards',()=>{
  assert.throws(()=>alongRange({ref:'start',at:{value:1,unit:'h'}},[0,total],context),/Riding-time/);
  const range=alongRange({ref:'end',to:{value:1,unit:'km'}},[0,total],context);
  assert.deepEqual(range,[total-1,total]);assert.deepEqual(at(line,0),line[0]);assert.deepEqual(at(line,total),line.at(-1));
});
test('malformed or unbounded API data is rejected before retrieval',()=>{
  validateInput(context);
  for(const extra of [{view:[0,0,1,Infinity]},{region:'../../data'},{request:{type:'places',what:[]}},{plan:{...context.plan,coordinates:[[NaN,0]]}},{request:{type:'route',to:{kind:'unknown-kind'}}}])assert.throws(()=>validateInput({...context,...extra}));
});

test('every day ends resolve separately and riding-time splits use the supplied profile',()=>{
  const every=findPlaces(db,{type:'places',what:['bakery'],where:{day:'every',part:'end'}},context);
  assert.ok(every.results.length);assert.match(every.area,/every day end/);
  const timed={...context,plan:{...context.plan,hours:[0,1,3,4]}};
  const split=resolve(db,{type:'split',per_day:{value:2,unit:'h'}},timed).changes[0];
  assert.equal(split.count,2);assert.equal(split.boundaries.length,1);
  assert.ok(Math.abs(split.boundaries[0]-(ds[1]+ds[2])/2)<1e-9);
  assert.throws(()=>alongRange({ref:'end',at:{value:10,unit:'h'}},[0,total],timed),/beyond/);
});

test('repeated stops bound work before resolving duplicate destinations', () => {
  assert.throws(() => resolve(db, {type:'add_point', point:{name:'Kandel'}, every:{value:0.001,unit:'km'}}, context), /50 stop intervals/);
});
