import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {database} from './database.mjs';
import {answerQuery} from '../query.mjs';
import {validateRequest,validateInput} from '../validation.mjs';
import {searchRuntime} from '../runtime.mjs';
import {nativeSearch} from '../native.mjs';
const {db}=database();
const input={q:'Kandel',view:[7.8,47.9,7.95,48.05],submitted:true};
const never={parse(){throw new Error('Model must not run');}};

test('shared runtime supplies one clock and explicit calendar capability for filtering and status', async()=>{
  const times=[], now=Date.parse('2026-09-28T10:00:00Z');
  const hours={assertEnvironment(){},
    openingState(_place,_filter,context){times.push(Date.parse(context.now));return 'open';},
    currentOpening(_place,time){times.push(time);return {state:'open'};}};
  assert.throws(()=>searchRuntime({db,parser:never}),/opening-hours adapters/);
  const runtime=searchRuntime({db,parser:never,hours,clock:()=>now});
  const result=await runtime.query({...input,q:'hotel',request:{type:'places',what:['hotel'],open:{now:true}}});
  assert.ok(result.results.length);assert.ok(times.length>result.results.length);
  assert.ok(times.every(time=>time===now));
  assert.deepEqual(result.results[0].hoursStatus,{state:'open'});
  hours.assertEnvironment=()=>{throw new Error('Wrong calendar zone');};
  await assert.rejects(runtime.query(input),/Wrong calendar zone/);
});

test('native JSON capabilities preserve complete replies, metadata and reverse labels', async()=>{
  const fixture=database();
  fixture.conn.prepare('INSERT INTO metadata VALUES (?,?)').run('schema','4');
  fixture.conn.prepare('INSERT INTO metadata VALUES (?,?)').run('attribution',JSON.stringify(['OSM contributors']));
  const hours={assertEnvironment(){},openingState(){return 'unknown';},currentOpening(){return undefined;}};
  let calls=0;
  const native=nativeSearch({region:'test',hours,
    all:(sql,bind)=>JSON.stringify({rows:fixture.db.all(sql,JSON.parse(bind))}),
    parse:()=>{calls++;return JSON.stringify({request:{type:'reverse'},elapsed:1});}});
  const answer=await native.request('query',{...input,q:'reverse this route',now:'2026-09-28T10:00:00Z'});
  assert.equal(calls,1);assert.equal(answer.request.type,'reverse');
  assert.equal((await native.request('status',{})).parser.ready,true);
  const points=[[7.854,48.01],[7.86,48.02]];
  assert.deepEqual(await native.request('route-request',{points,bike:'touring',goal:'shortest'}),
    {points,profile:'touring/shorter',alternatives:false});
  for (const goal of ['least_unpaved','most_climbing'])
    await assert.rejects(native.request('route-request',{points,bike:'touring',goal}), /routing package has no/);
  assert.equal(answer.region,'test');assert.deepEqual(answer.attribution,['OSM contributors']);
  fixture.conn.exec("UPDATE place_records SET website='https://bakery.example',phone='+49 123',description='Bread and coffee.' WHERE source='n1'");
  const details=await native.request('query',{...input,q:'',source:'n1'});
  assert.equal(details.results[0].website,'https://bakery.example');
  assert.equal(details.results[0].phone,'+49 123');
  assert.equal(details.results[0].description,'Bread and coffee.');
  assert.equal(calls,1);
  for(const source of ['n1 OR 1=1','poi-1',4,'w0'])assert.throws(()=>validateInput({...input,source}));
  assert.deepEqual(await native.request('reverse',{coordinate:[7.854,48.01]}),{label:'Habsburgerstraße 10, Freiburg'});
  await assert.rejects(native.request('query',{...input,region:'missing'}),/does not cover/);
  await assert.rejects(native.request('reverse',{region:'missing',coordinate:[7.854,48.01]}),/does not cover/);
  fixture.conn.close();
});

test('exact names and typed chip edits bypass the model, sentences invoke it',async()=>{
  const exact=await answerQuery(db,input,never);
  assert.equal(exact.notice,'');assert.equal(exact.request.type,'place');
  const edited=await answerQuery(db,{...input,q:'a different typed sentence',request:{type:'place',name:'Kandel'}},never);
  assert.equal(edited.notice,'');assert.equal(edited.results[0].name,'Kandel');
  let text;
  const parsed=await answerQuery(db,{...input,q:'reverse the route'}, {async parse(q){text=q;return {request:{type:'reverse'},elapsed:12};}});
  assert.equal(text,'reverse the route');assert.equal(parsed.request.type,'reverse');
  assert.equal(parsed.type,'unresolved');assert.match(parsed.note,/route first/);
});
test('out-of-domain and unavailable-model results stay visible and cannot edit the plan',async t=>{
  const out=await answerQuery(db,{...input,q:'write me a poem'}, {async parse(){return {request:{type:'none',ignored:['poem']},elapsed:1};}});
  assert.equal(out.type,'places');assert.match(out.notice,/not understood/);assert.deepEqual(out.request.ignored,['poem']);
  const offline=await answerQuery(db,{...input,q:'show water after day two'}, {async parse(){throw new Error('Runtime stopped.');}});
  assert.match(offline.notice,/Runtime stopped/);assert.equal(offline.canRetry,true);assert.equal(offline.type,'places');
  const log=t.mock.method(console,'error',()=>{});
  const drifted=await answerQuery(db,{...input,q:'show water after day two'}, {async parse(){return {request:{type:'places',what:['moon_base']},elapsed:1};}});
  assert.match(drifted.notice,/unsupported request/);assert.equal(drifted.canRetry,false);assert.equal(drifted.type,'places');
  assert.equal(log.mock.callCount(),1);
});
test('the API accepts every archived decoder request in all four evaluation languages',()=>{
  for(const language of ['en','de','fr','it']) {
    const rows=readFileSync(new URL(`../query/testset/${language}.jsonl`,import.meta.url),'utf8').trim().split('\n').map(JSON.parse);
    for(const row of rows)validateRequest(row.request);
  }
});

test('plain categories inherit pointing and cuisine filters stay visible and removable',async()=>{
  const category=await answerQuery(db,{...input,q:'hotel',pointing:{anchor:[11.57,48.13]}},never);
  assert.equal(category.results[0].city,'München');
  const pizza=await answerQuery(db,{...input,q:'pizza'},never);
  assert.equal(pizza.request.cuisine,'pizza');assert.ok(pizza.results.some(p=>p.name==='La Luna'));
  const request={...pizza.request};delete request.cuisine;
  const anyFood=await answerQuery(db,{...input,q:'pizza',request},never);
  assert.ok(anyFood.results.some(p=>p.name==='Asia Wok'));assert.equal(anyFood.request.cuisine,undefined);
  const doner=await answerQuery(db,{...input,q:'Döner'},never);
  assert.deepEqual(new Set(doner.results.map(p=>p.source)),new Set(['n17','n19']));
});

test('literal names and bilingual categories keep an explicit locality without the model', async () => {
  const {db} = database([
    ['r27','Teningen','town',7.81,48.13,'Teningen',.2],
    ['n28','Lidl','supermarket',7.815,48.129,'Teningen',0],
    ['n29','ALDI Süd','supermarket',7.845,48.117,'Emmendingen',0],
    ['n30','ALDI','supermarket',9.18,48.77,'Stuttgart',0],
    ['n31','Berggasthaus Kandelhof','restaurant',8.018,48.063,'Waldkirch',0],
  ]);
  for (const q of ['Lidl in Teningen', 'Lidl Teningen', 'Aldi in Teningen']) {
    const r = await answerQuery(db, {...input, q}, never);
    assert.equal(r.notice, '');
    assert.equal(r.request.type, 'place');
    assert.ok(r.results.length);
    assert.ok(r.results.every(p => p.distance < 5));
    assert.match(r.area, /Teningen/);
    assert.ok(r.results.every(p => p.city !== 'Stuttgart'));
  }
  for (const q of ['Shop Teningen', 'Shop in Teningen', 'Supermarkt Teningen', 'shops in Teningen']) {
    const r = await answerQuery(db, {...input, q}, never);
    assert.equal(r.notice, '');
    assert.ok(r.results.some(p => p.name === 'Lidl'));
    assert.ok(r.results.every(p => p.kind === 'supermarket' && p.distance < 5));
  }
  const inn = await answerQuery(db, {...input, q:'Berggasthaus Kandel'}, never);
  assert.equal(inn.request.type, 'place');
  assert.equal(inn.results[0].name, 'Berggasthaus Kandelhof');
  assert.equal(inn.notice, '');
});

test('category searches include the shared device service subtypes', async () => {
  const {db,conn}=database([
    ['n90','Tap','water_tap',7.85,47.99,'Freiburg',.1],
    ['n91','Caravan','caravan_site',7.85,47.99,'Freiburg',.1],
    ['n92','Motel','motel',7.85,47.99,'Freiburg',.1],
  ]);
  assert.ok((await answerQuery(db,{...input,q:'water'},never)).results.some(p=>p.source==='n90'));
  assert.ok((await answerQuery(db,{...input,q:'camping'},never)).results.some(p=>p.source==='n91'));
  assert.ok((await answerQuery(db,{...input,q:'accommodation'},never)).results.some(p=>p.source==='n92'));
  conn.close();
});
