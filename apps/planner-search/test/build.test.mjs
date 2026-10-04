import {test} from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {mkdtempSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import {answerQuery} from '../query.mjs';

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
  let connection;
  try {
    execFileSync('python3',['-c',`import json,sys
sys.path.insert(0,sys.argv[1])
from pathlib import Path
from build import Writer
writer=Writer('test',Path(sys.argv[2]))
for place in json.load(sys.stdin): writer.add(place)
writer.finish({'bounds':[7,47,9,49]})`,new URL('..',import.meta.url).pathname,directory],{input:JSON.stringify(records)});
    connection=new DatabaseSync(join(directory,'test.sqlite'),{readOnly:true});
    const db={all:(sql,params=[])=>connection.prepare(sql).all(...params)};
    assert.equal(JSON.parse(db.all("SELECT value FROM metadata WHERE key='schema'")[0].value),4);
    const rows=db.all('SELECT website,phone,description FROM places ORDER BY id').map(row=>({...row}));
    assert.deepEqual(rows,[
      {website:'https://hotel.example',phone:'+49 123',description:'Tents welcome.'},
      {website:'water.example',phone:'+33 456',description:'Petite source avec robinet.'},
      {website:'',phone:'',description:'Small tents only.\nAsk at the farm.'},
      {website:'',phone:'',description:''},
    ]);
    const result=await answerQuery(db,{source:'n2',q:'',view:[7,47,9,49]},
      {parse(){throw new Error('An ID lookup must not run the model');}});
    assert.equal(result.results.length,1);
    assert.equal(result.results[0].description,rows[1].description);
    assert.equal(result.results[0].source,'n2');
    assert.equal((await answerQuery(db,{source:'n99',q:''},{})).results.length,0);
  } finally {connection?.close();rmSync(directory,{recursive:true,force:true});}
});
