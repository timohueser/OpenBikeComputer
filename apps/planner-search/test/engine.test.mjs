import {test} from 'node:test';
import assert from 'node:assert/strict';
import {search,norm,simpleRequest,rank,routePositions,distance} from '../web/engine.mjs';
import {streetNorm,compact} from '../web/text.mjs';
import {lengths} from '../web/geography.mjs';

import {database} from './database.mjs';
const {db,conn,records}=database();
const view=[7.8,47.9,7.95,48.05];

test('distant named results survive local matches; ambiguity uses proximity',()=>{
  assert.equal(search(db,{q:'Kandel',view}).results[0].source,'n6');
  assert.equal(search(db,{q:'Kandel',view:[8.1,49,8.3,49.2]}).results[0].source,'r7');
  assert.equal(search(db,{q:'Hotel Krone',view}).results.length,2);
  assert.equal(search(db,{q:'Munich',view}).results[0].source,'r5');
});
test('a short city name outranks a nearby district with default geographic importance',()=>{
  const {db,conn}=database([
    ['r30','Freiburg im Breisgau','city',7.8494,47.9961,'',.24],
    ['r31','Freiburg','locality',7.7713,47.9958,'',.067],
  ]);
  for(const q of ['Freiburg','Freibug','Xreiburg'])
    assert.equal(search(db,{q,view}).results[0].source,'r30',q);
  conn.close();
});
test('free-form addresses retain house precision and label fallback',()=>{
  const house=search(db,{q:'12 Kaiser Joseph Str Freiburg',view});
  assert.equal(house.results[0].source,'w123');
  assert.equal(house.results[0].precision,'house');
  const missing=search(db,{q:'Kaiser Joseph Str 999 Freiburg',view});
  assert.equal(missing.results[0].precision,'street');
  assert.match(missing.note,/House number not found/);
});
test('SQL-looking input stays data',()=>{
  assert.doesNotThrow(()=>search(db,{q:'" OR 1=1; DROP TABLE places --',view}));
  assert.equal(db.all('SELECT count(*) n FROM places')[0].n,records.length);
});
test('ordinary place names and categories do not depend on word count',()=>{
  assert.equal(simpleRequest('bakery').type,'places');
  assert.equal(simpleRequest('Statue of Liberty').type,'place');
  assert.equal(simpleRequest('Lidl').type,'place');
  assert.deepEqual(simpleRequest('Döner near me').where,{scope:'here'});
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
test('route positions equal a scan of every segment and read only nearby segments',()=>{
  const route=Array.from({length:20000},(_,i)=>[7+i*1e-4,48+.01*Math.sin(i/50)]),ds=lengths(route);
  const scan=point=>{
    let best={distance:Infinity,along:0},along=0;
    for(let i=1;i<route.length;i++) {
      const a=route[i-1],b=route[i],cos=Math.cos(point[1]*Math.PI/180),len=ds[i]-ds[i-1];
      const vx=(b[0]-a[0])*cos,vy=b[1]-a[1],wx=(point[0]-a[0])*cos,wy=point[1]-a[1];
      const t=Math.max(0,Math.min(1,(vx*wx+vy*wy)/(vx*vx+vy*vy||1)));
      const d=distance(point,[a[0]+t*(b[0]-a[0]),a[1]+t*(b[1]-a[1])]);
      if(d<best.distance)best={distance:d,along:along+t*len};
      along+=len;
    }
    return {...best,total:along};
  };
  let reads=0;
  const position=routePositions(new Proxy(route,{get(target,key) {
    if(/^\d+$/.test(String(key)))reads++;
    return target[key];
  }}),ds);
  const near=Array.from({length:300},(_,i)=>{
    const [x,y]=route[i*6151%route.length];
    return [x+(i%7-3)*.002,y+(i%5-2)*.004];
  });
  reads=0;
  for(const point of near)assert.deepEqual(position(point),scan(point));
  assert.ok(reads<near.length*route.length/10,`${reads} route reads`);
  for(const point of near.slice(0,50).map(([x,y],i)=>[x+(i%3-1)*.3,y+(i%4-1.5)*.2]))
    assert.deepEqual(position(point),scan(point));
});
