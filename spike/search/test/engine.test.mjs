import {test} from 'node:test';
import assert from 'node:assert/strict';
import {DatabaseSync} from 'node:sqlite';
import {search,norm,simpleRequest,rank} from '../web/engine.mjs';
import {streetNorm,compact} from '../web/text.mjs';

const conn=new DatabaseSync(':memory:');
conn.exec(`CREATE TABLE places(id INTEGER PRIMARY KEY,source TEXT,name TEXT,aliases TEXT,kind TEXT,lon REAL,lat REAL,
  city TEXT,postcode TEXT,importance REAL,west REAL,south REAL,east REAL,north REAL,region TEXT,context TEXT,cuisine TEXT);
  CREATE TABLE addresses(street_id INTEGER,house TEXT,lon REAL,lat REAL,source TEXT);
  CREATE TABLE names(term TEXT,place_id INTEGER,PRIMARY KEY(term,place_id)) WITHOUT ROWID;
  CREATE TABLE compact_names(term TEXT,place_id INTEGER,PRIMARY KEY(term,place_id)) WITHOUT ROWID;
  CREATE TABLE lexicon(term TEXT);
  CREATE VIRTUAL TABLE spatial USING rtree(id,west,east,south,north);
  CREATE VIRTUAL TABLE terms USING fts5(name,context,content='',detail=column,prefix='3');
  CREATE VIRTUAL TABLE fuzzy USING fts5(term,content='lexicon',detail=none,tokenize='trigram');`);
const db={all:(sql,params=[])=>conn.prepare(sql).all(...params)};
const view=[7.8,47.9,7.95,48.05];
const records=[
  ['n1','Bäckerei Müller','bakery',7.85,47.99,'Freiburg',.1],
  ['n2','Brotzeit','bakery',7.86,47.99,'Freiburg',.1],
  ['n3','Bakery Design Studio','office',7.8501,47.99,'Freiburg',.9],
  ['n4','Bäckerei München','bakery',11.576,48.14,'München',.1],
  ['r5','München','city',11.576,48.137,'München',.8,'Munich'],
  ['n6','Kandel','summit',8.012,48.062,'Waldkirch',.5],
  ['r7','Kandel','city',8.195,49.08,'Kandel',.5],
  ['n8','Hotel Krone','hotel',7.86,47.99,'Freiburg',.2],
  ['n9','Hotel Krone','hotel',11.57,48.13,'München',.95],
  ['s10','Kaiser-Joseph-Straße','street',7.85,47.99,'Freiburg',.05],
  ['n11','Kandel','pass',8.016,48.065,'Simonswald',.5],
  ['s12','Kandel','street',8.016,48.065,'Simonswald',.05],
  ['s13','Kandel','street',8.015,48.066,'Waldkirch',.05],
  ['s14','Habsburgerstraße','street',7.854,48.01,'Freiburg',.05],
  ['s15','Habsburgerstr.','street',11.57,48.13,'München',.05],
  ['n16','La Luna','restaurant',7.85,47.99,'Freiburg',.1,'','italian;pizza'],
  ['n17','Express Döner','fast_food',7.86,47.99,'Freiburg',.1],
  ['n18','Asia Wok','fast_food',7.855,47.99,'Freiburg',.1,'','asian'],
  ['n19','Orient Grill','fast_food',7.86,47.98,'Freiburg',.1,'','kebab;pizza'],
  ['n20','Pizza Werbeagentur','office',7.85,47.99,'Freiburg',.1],
  ['n21','Pizzeria Roma','restaurant',11.576,48.14,'München',.1],
  ['n22','MediaMarkt','electronics',7.85,47.99,'Freiburg',.1],
  ['n23','Media Markt','electronics',11.57,48.13,'München',.95],
  ['n24','SilberFuchs','shop',7.86,47.99,'Freiburg',.1],
  ['n25','Silberfuchsmuseum','museum',7.86,47.99,'Freiburg',.1],
  ['s26','Rathausplatz','street',7.85,47.99,'Freiburg',.05],
];
records.forEach(([source,name,kind,lon,lat,city,importance,aliases='',cuisine=''],i)=>{
  const bounds=kind==='city'?[lon-.1,lat-.1,lon+.1,lat+.1]:[lon,lat,lon,lat];
  conn.prepare('INSERT INTO places VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)').run(i+1,source,name,aliases,kind,lon,lat,city,'',importance,...bounds,'',city,cuisine);
  conn.prepare('INSERT INTO spatial VALUES (?,?,?,?,?)').run(i+1,lon,lon,lat,lat);
  const forms=new Set([name,...aliases.split(';')].filter(Boolean).flatMap(n=>kind==='street'?[norm(n),streetNorm(n)]:[norm(n)]));
  conn.prepare('INSERT INTO terms(rowid,name,context) VALUES (?,?,?)').run(i+1,[...forms].join(' '),norm(city));
  for(const t of forms) {
    conn.prepare('INSERT INTO names VALUES (?,?)').run(t,i+1);
    conn.prepare('INSERT OR IGNORE INTO compact_names VALUES (?,?)').run(compact(t),i+1);
  }
});
conn.exec("INSERT INTO lexicon SELECT DISTINCT term FROM compact_names; INSERT INTO fuzzy(fuzzy) VALUES('rebuild')");
conn.exec("INSERT INTO addresses VALUES (10,'12',7.851,47.991,'w123')");
conn.exec("INSERT INTO addresses VALUES (14,'10',7.854,48.01,'w14'),(14,'10',7.854,48.03,'w141'),(15,'10',11.57,48.13,'w15')");

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
    assert.match(all[0].why.order,/Distance among equivalent/);
  }
  assert.equal(search(db,{q:'Hotel Krone München',view}).results[0]?.source,'n9');
  const near={source:'near',name:'Example',kind:'hotel',precision:'place',score:101,distance:1,why:{match:100}};
  const far={...near,source:'far',score:120,distance:200};
  const other={...near,source:'other',name:'Different',score:110};
  assert.deepEqual(rank([other,near,far]).map(p=>p.source),['near','other','far']);
});
