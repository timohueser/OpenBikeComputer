import {test} from 'node:test';
import assert from 'node:assert/strict';
import {search,norm,simpleRequest,rank} from '../web/engine.mjs';
import {streetNorm,compact} from '../web/text.mjs';

import {database} from './database.mjs';
const {db,conn,records}=database();
const view=[7.8,47.9,7.95,48.05];

test('a category uses tags and stays in the viewport',()=>{
  const out=search(db,{q:'bakery',view});
  assert.deepEqual(new Set(out.results.map(p=>p.source)),new Set(['n1','n2']));
  assert.ok(out.results.every(p=>p.kind==='bakery'));
  assert.ok(out.results[0].distance<=out.results[1].distance);
});
test('a named city overrides the viewport and resolves an alias',()=>{
  const out=search(db,{q:'bakeries in Munich',view,request:{type:'places',what:['bakery'],where:{near:'Munich',in:true}}});
  assert.deepEqual(out.results.map(p=>p.source),['n4']);
});
test('distant named results survive local matches; ambiguity uses proximity',()=>{
  assert.equal(search(db,{q:'Kandel',view}).results[0].source,'n6');
  assert.equal(search(db,{q:'Kandel',view:[8.1,49,8.3,49.2]}).results[0].source,'r7');
  assert.equal(search(db,{q:'Hotel Krone',view}).results.length,2);
  assert.equal(search(db,{q:'Munich',view}).results[0].source,'r5');
});
test('free-form addresses retain house precision and label fallback',()=>{
  const house=search(db,{q:'12 Kaiser Joseph Str Freiburg',view});
  assert.equal(house.results[0].source,'w123');
  assert.equal(house.results[0].precision,'house');
  const missing=search(db,{q:'Kaiser Joseph Str 999 Freiburg',view});
  assert.equal(missing.results[0].precision,'street');
  assert.match(missing.note,/House number not found/);
});
test('route halves filter by route distance; missing day does not broaden',()=>{
  const plan={coordinates:[[7.84,47.99],[7.85,47.99],[7.9,47.99]],days:[1,2]};
  const request={type:'places',what:['bakery'],where:{scope:'route',part:'first_half'},radius:.2};
  const out=search(db,{q:'bakeries in first half',view,plan,request});
  assert.equal(out.results.length,2);
  assert.ok(out.results.every(p=>p.position.along<=p.position.total/2));
  const absent=search(db,{q:'bakeries day three',view,plan,request:{...request,where:{day:3,part:'end'}}});
  assert.equal(absent.results.length,0);assert.match(absent.note,/no day 3/);
});
test('near me needs location and SQL-looking input stays data',()=>{
  const out=search(db,{q:'bakery near me',view,request:{type:'places',what:['bakery'],where:{scope:'here'}}});
  assert.match(out.note,/Set your location/);
  assert.doesNotThrow(()=>search(db,{q:'" OR 1=1; DROP TABLE places --',view}));
  assert.equal(db.all('SELECT count(*) n FROM places')[0].n,records.length);
});
test('ordinary place names and categories do not depend on word count',()=>{
  assert.equal(simpleRequest('bakery').type,'places');
  assert.equal(simpleRequest('Statue of Liberty').type,'place');
  assert.equal(simpleRequest('Lidl').type,'place');
});
test('street abbreviations match in both directions; addresses follow the map or explicit city',()=>{
  for(const q of ['Habsburgerstr 10','Habsburgerstr. 10','Habsburgerstraße 10']) {
    assert.equal(search(db,{q,view}).results[0].source,'w14',q);
    assert.equal(search(db,{q,view:[11.5,48.1,11.6,48.2]}).results[0].source,'w15',q);
    assert.equal(search(db,{q:q+' München',view}).results[0].source,'w15',q);
  }
  assert.equal(search(db,{q:'Habsburgerstr 10',view:[7.85,48.025,7.86,48.035]}).results[0].source,'w141');
});
test('nearby street groups collapse but a summit and pass stay distinct',()=>{
  const r=search(db,{q:'Kandel',view}).results;
  assert.equal(r.filter(p=>p.kind==='street').length,1);
  assert.ok(r.some(p=>p.kind==='pass'));
  assert.ok(r.some(p=>p.kind==='summit'));
  assert.ok(r.some(p=>p.kind==='city'));
});
test('food searches use cuisine tags or food business names, within their geographic scope',()=>{
  assert.deepEqual(simpleRequest('Döner near me').where,{scope:'here'});
  assert.deepEqual(new Set(search(db,{q:'pizza',view}).results.map(p=>p.source)),new Set(['n16','n19']));
  for(const q of ['Döner','Doener','kebap'])
    assert.deepEqual(new Set(search(db,{q,view}).results.map(p=>p.source)),new Set(['n17','n19']));
  assert.deepEqual(search(db,{q:'pizza in Munich',view,request:{type:'places',what:['pizza'],where:{near:'Munich',in:true}}}).results.map(p=>p.source),['n21']);
});
test('spacing and punctuation work across arbitrary names, including explicit cities',()=>{
  for(const q of ['Media Markt','Media-Markt','MediaMarkt Freiburg','Media Markt Freiburg'])
    assert.equal(search(db,{q,view}).results[0]?.source,'n22',q);
  assert.equal(search(db,{q:'MediaMarkt München',view}).results[0]?.source,'n23');
  for(const q of ['Silber Fuchs','Silber-Fuchs','SilberFuchs'])
    assert.equal(search(db,{q,view}).results[0]?.source,'n24',q);
  assert.equal(search(db,{q:'Rathauspl. Freiburg',view}).results[0]?.source,'s26');
});
test('spelling retrieval tolerates first-letter errors and two edits without a prefix lock',()=>{
  for(const [q,source] of [['NediaMarkt','n22'],['Mdeia Mrkt','n22'],['Silbr Fxchs','n24']]) {
    const r=search(db,{q,view}).results;
    assert.equal(r[0]?.source,source,q);
    assert.ok(r[0].why.correction,q);
  }
  assert.equal(search(db,{q:'zzzxq',view}).results.length,0);
});
test('equivalent named destinations use distance despite unequal importance and spelling',()=>{
  for(const [q,expected] of [['Hotel Krone','n8'],['MediaMarkt','n22'],['Media Markt','n22'],['NediaMarkt','n22']]) {
    const all=search(db,{q,view}).results;
    assert.equal(all[0]?.source,expected,q);
    assert.equal(search(db,{q,view,limit:1}).results[0]?.source,expected,q);
    assert.equal(rank([...all].reverse())[0]?.source,expected,q);
    assert.equal(all[0].why.importance,0);
  }
  assert.equal(search(db,{q:'Hotel Krone München',view}).results[0]?.source,'n9');
  const near={source:'near',name:'Example',kind:'hotel',precision:'place',score:101,distance:1,why:{match:100}};
  const far={...near,source:'far',score:120,distance:200};
  const other={...near,source:'other',name:'Different',score:110};
  assert.deepEqual(rank([other,near,far]).map(p=>p.source),['far','other','near']);
});
