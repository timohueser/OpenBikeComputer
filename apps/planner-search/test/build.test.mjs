import {test} from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {mkdtempSync,rmSync,mkdirSync,writeFileSync,copyFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import {answerQuery} from '../query.mjs';
import {openRegion} from '../installation.mjs';
import {openCells} from '../cells.mjs';
import {search} from '../web/engine.mjs';
import {reverseAddress} from '../web/reverse.mjs';
import {openingHours} from '../hours.mjs';

const hours=openingHours('Europe/Berlin');
// Builds Photon place records into DIRECTORY/test.sqlite and opens it read-only.
function writePlaces(directory,records) {
  execFileSync('python3',['-c',`import json,sys
sys.path.insert(0,sys.argv[1])
from pathlib import Path
from writer import Writer
writer=Writer('test',Path(sys.argv[2]))
for place in json.load(sys.stdin): writer.add(place)
writer.finish({'bounds':[7,47,9,49]})`,new URL('..',import.meta.url).pathname,directory],{input:JSON.stringify(records)});
  return new DatabaseSync(join(directory,'test.sqlite'),{readOnly:true});
}

test('the POI builder retains contact aliases and descriptions for named and unnamed places',async()=>{
  const directory=mkdtempSync(join(tmpdir(),'obc-poi-details-'));
  const records=[
    {object_id:1,osm_key:'tourism',osm_value:'hotel',name:{name:'Hotel'},extra:{website:' https://hotel.example ',
      'contact:website':'https://other.example',phone:' +49 123 ','contact:phone':'+49 999',description:'Tents welcome.','description:en':'Other text'}},
    {object_id:2,osm_key:'natural',osm_value:'spring',extra:{'website':' ','contact:website':'water.example',
      'contact:phone':'+33 456','description:fr':'Petite source avec robinet.'}},
    {object_id:3,osm_key:'tourism',osm_value:'camp_site',extra:{'description:en':'Small tents only.\nAsk at the farm.','description:de':'Nur kleine Zelte.'}},
    {object_id:4,osm_key:'amenity',osm_value:'drinking_water',extra:{}},
  ].map(p=>({object_type:'N',centroid:[8,48],...p}));
  let connection,db;
  try {
    connection=writePlaces(directory,records);
    db=openCells([join(directory,'test.sqlite')]);
    assert.equal(db.metadata.schema,5);
    const rows=connection.prepare('SELECT website,phone,description FROM places ORDER BY id').all().map(row=>({...row}));
    assert.deepEqual(rows,[
      {website:'https://hotel.example',phone:'+49 123',description:'Tents welcome.'},
      {website:'water.example',phone:'+33 456',description:'Petite source avec robinet.'},
      {website:'',phone:'',description:'Small tents only.\nAsk at the farm.'},
      {website:'',phone:'',description:''},
    ]);
    const result=await answerQuery(db,{source:'n2',q:'',view:[7,47,9,49]},
      {parse(){throw new Error('An ID lookup must not run the model');}},hours);
    assert.equal(result.results.length,1);
    assert.equal(result.results[0].description,rows[1].description);
    assert.equal(result.results[0].source,'n2');
    assert.equal((await answerQuery(db,{source:'n99',q:''},{},hours)).results.length,0);
  } finally {db?.close();connection?.close();rmSync(directory,{recursive:true,force:true});}
});

test('Swiss places keep the German canton name that selects cantonal holidays',()=>{
  const directory=mkdtempSync(join(tmpdir(),'obc-cantons-'));
  const records=[
    {object_id:1,address:{state:'Graubünden/Grischun/Grigioni','state:de':'Graubünden','state:it':'Grigioni'}},
    {object_id:2,address:{state:'St. Gallen','state:de':'St. Gallen'}},
  ].map(p=>({object_type:'N',osm_key:'shop',osm_value:'bakery',name:{name:`Bakery ${p.object_id}`},centroid:[9.53,46.85],
    country_code:'ch',extra:{opening_hours:'Mo-Su 08:00-18:00; PH off'},...p}));
  let connection;
  try {
    connection=writePlaces(directory,records);
    const places=connection.prepare('SELECT * FROM places ORDER BY id').all();
    assert.deepEqual(places.map(p=>p.region),['Graubünden','Sankt Gallen']);
    const goodFriday={openDate:'2026-04-03'};
    for(const place of places)assert.equal(openingHours('Europe/Zurich').openingState(place,{},goodFriday),'closed');
  } finally {connection?.close();rmSync(directory,{recursive:true,force:true});}
});

test('independent POI and address inputs preserve search, locality ownership and disjoint numeric identities',()=>{
  const directory=mkdtempSync(join(tmpdir(),'obc-search-components-'));
  let installed,grid;
  try {
    execFileSync('python3',['-c',`import io,json,sys
sys.path.insert(0,sys.argv[1])
from pathlib import Path
from writer import Writer
from split import split_lines
common={'object_type':'N','centroid':[8,48],'country_code':'de','address':{'city':'Testville'}}
records=[
 {**common,'object_id':1,'osm_key':'place','osm_value':'city','address_type':'city','name':{'name':'Testville'}},
 {**common,'object_id':2,'osm_key':'highway','osm_value':'residential','address_type':'street','name':{'name':'Main Street'}},
 {**common,'object_id':3,'osm_key':'building','osm_value':'yes','housenumber':'12','address':{'city':'Testville','street':'Main Street'}},
 {**common,'object_id':4,'osm_key':'tourism','osm_value':'hotel','name':{'name':'Hotel View'},'housenumber':'14','address':{'city':'Testville','street':'Main Street'},'extra':{'description':'Tents welcome.'}},
 {**common,'object_id':5,'osm_key':'natural','osm_value':'spring'}]
header={'type':'NominatimDumpFile','content':{'data_timestamp':'2026-01-01T00:00:00Z'}}
outputs={c:io.BytesIO() for c in ('pois','addresses')}
counts=split_lines([json.dumps(header),json.dumps({'type':'Place','content':records})],outputs)
assert counts=={'pois':3,'addresses':3},counts
for component,output in outputs.items():
 lines=[json.loads(line) for line in output.getvalue().splitlines()]
 assert lines[0]==header
 writer=Writer('test',Path(sys.argv[2])/component,component)
 for p in lines[1]['content']: writer.add(p)
 writer.finish({'bounds':[7,47,9,49],'osm_sha256':'a'*64})
`,new URL('..',import.meta.url).pathname,directory]);
    installed=openRegion(directory,'test');
    const places=installed.db.rows({sql:'SELECT p.* FROM {c}.places p'});
    assert.equal(places.filter(p=>p.kind==='city').length,1);
    assert.equal(places.filter(p=>p.kind==='street').length,1);
    assert.equal(new Set(places.map(p=>p.id)).size,4);
    assert.ok(places.every(p=>p.country==='de'));
    for(const p of places)assert.ok(Number.isSafeInteger(p.id)&&p.id>0);
    assert.ok(places.find(p=>p.kind==='street').id>2**52);
    assert.ok(places.find(p=>p.kind==='hotel').id<2**52);
    assert.equal(search(installed.db,{q:'Hotel View',view:[7,47,9,49]}).results[0].description,'Tents welcome.');
    const reverse=reverseAddress(installed.db,[8,48]);
    assert.equal(reverse,'Main Street 12, Testville');
    const cells=[{id:'9-267-177',bounds:[7,47,9,49],files:['tiles/pois/9-267-177.sqlite','tiles/addresses/9-267-177.sqlite']}];
    for(const component of ['pois','addresses']) {
      mkdirSync(join(directory,'tiles',component),{recursive:true});
      copyFileSync(join(directory,component,'test.sqlite'),join(directory,'tiles',component,'9-267-177.sqlite'));
    }
    writeFileSync(join(directory,'test.grid.json'),JSON.stringify({format:3,cells}));
    grid=openRegion(directory,'test');
    assert.equal(grid.db.rows({sql:'SELECT p.* FROM {c}.places p'}).length,4);
    writeFileSync(join(directory,'test.grid.json'),JSON.stringify({format:3,cells:[{...cells[0],files:cells[0].files.slice(0,1)}]}));
    assert.throws(()=>openRegion(directory,'test'),/Invalid search grid files/);
    writeFileSync(join(directory,'test.grid.json'),JSON.stringify({format:3,cells:[{...cells[0],files:['../pois/test.sqlite',cells[0].files[1]]}]}));
    assert.throws(()=>openRegion(directory,'test'),/Invalid search grid files/);
    writeFileSync(join(directory,'test.grid.json'),JSON.stringify({format:3,cells}));
    const copy=new DatabaseSync(join(directory,'tiles/addresses/9-267-177.sqlite'));
    copy.prepare("UPDATE metadata SET value=? WHERE key='osm_sha256'").run(JSON.stringify('b'.repeat(64)));
    copy.close();
    assert.throws(()=>openRegion(directory,'test'),/one OSM source/);
  } finally {installed?.db.close();grid?.db.close();rmSync(directory,{recursive:true,force:true})}
});

test('street representatives, aliases and extents do not depend on source record order',()=>{
  const directory=mkdtempSync(join(tmpdir(),'obc-street-groups-'));
  const connections=[];
  try {
    execFileSync('python3',['-c',`import sys
sys.path.insert(0,sys.argv[1])
from pathlib import Path
from writer import Writer
common={'object_type':'W','country_code':'de','address':{'city':'Testville','street':'Main Street'},'osm_key':'highway','osm_value':'residential','address_type':'street'}
records=[
 {**common,'object_id':20,'centroid':[8.02,48.01],'name':{'name':'Main Street','alt_name':'Main Road'}},
 {**common,'object_id':10,'centroid':[8,48],'name':{'name':'Main Street','name:de':'Hauptstraße'}},
 {**common,'object_type':'N','object_id':1,'osm_key':'building','osm_value':'yes','address_type':'house','centroid':[8.01,48.02],'housenumber':'12'}]
for name,rows in [('forward',records),('reverse',list(reversed(records)))]:
 writer=Writer(name,Path(sys.argv[2]),'addresses')
 for row in rows: writer.add(row)
 writer.finish({'bounds':[7,47,9,49]})
`,new URL('..',import.meta.url).pathname,directory]);
    const read=name=>{
      const conn=new DatabaseSync(join(directory,`${name}.sqlite`),{readOnly:true});connections.push(conn);
      return conn.prepare('SELECT source,name,aliases,lon,lat,city,postcode,west,south,east,north FROM places').all().map(r=>({...r}));
    };
    const forward=read('forward');
    assert.deepEqual(forward,read('reverse'));
    assert.equal(forward.length,1);
    assert.equal(forward[0].aliases,'Hauptstraße;Main Road;Main Street');
    assert.equal(forward[0].lon,8);
    assert.equal(forward[0].lat,48);
    assert.equal(forward[0].east,8.02);
    assert.equal(forward[0].north,48.02);
  } finally {for(const c of connections)c.close();rmSync(directory,{recursive:true,force:true})}
});
