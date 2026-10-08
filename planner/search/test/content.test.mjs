import {test} from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {mkdtempSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {openCells} from '../cells.mjs';
import {answerQuery} from '../query.mjs';
import {openingHours} from '../hours.mjs';
import {validateInput} from '../validation.mjs';

const attribution={source_url:'https://en.wikipedia.org/?oldid=12',revision:'12',license_url:'https://creativecommons.org/licenses/by-sa/4.0/',original_notices:{large:'excluded'}};
const article={default_language:'de',variants:[{language:'en',text_pages:['A short article.'],attribution}],
  photo:{online_url:'https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Castle.jpg/500px-Castle.jpg',
    credit:['Castle','A photographer','CC BY-SA 4.0','Commons'],attribution,
    file_identity:{filename:'Castle.jpg',page_id:5},page_revision:12,file_revision:{timestamp:'2025-01-01T00:00:00Z',sha1:'abc'},
    path:'/device/photo.bin',bytes:51840,sha256:'device-only'}};
const never={parse(){throw new Error('Exact place names do not need inference');}};
const hours=openingHours('Europe/Berlin');

test('curated search retains articles and credits, deduplicates explicit OSM identities, and keeps distinct summit positions',async()=>{
  const directory=mkdtempSync(join(tmpdir(),'obc-place-content-'));
  let db;
  try {
    writeFileSync(join(directory,'landmarks.json'),JSON.stringify({aliases:{Q9:'Q1'},wikipedia_aliases:{'de:Alte Burg':'Q5','de:Burg am See':'Q5'},records:[
      {...article,qid:'Q1',name:'Castle',category:2,latitude:48,longitude:8},
      {...article,qid:'Q2',name:'Unmapped Castle',category:2,latitude:48.1,longitude:8.1},
      {...article,qid:'Q5',name:'Wikipedia Castle',category:2,latitude:48.4,longitude:8.4},
      {...article,qid:'Q3',name:'Outside Castle',category:2,latitude:60,longitude:8},
    ]}));
    writeFileSync(join(directory,'peaks.json'),JSON.stringify({records:[{...article,id:'Q4',name:'Peak'}],associations:[
      {node_id:3,article_id:'Q4',latitude:48.2,longitude:8.2},
      {node_id:4,article_id:'Q4',latitude:48.3,longitude:8.3},
    ]}));
    execFileSync('python3',['-c',`import sys
from pathlib import Path
sys.path.insert(0,sys.argv[1])
from writer import Writer
from content import add
w=Writer('test',Path(sys.argv[2]),'pois')
for identity,kind,qid,wiki in [(1,'castle','Q1',''),(2,'attraction','Q9',''),(3,'peak','',''),(4,'peak','',''),
                              (5,'castle','','de:Alte_Burg'),(6,'castle','','https://de.wikipedia.org/wiki/Burg%20am%20See'),
                              (7,'castle','','de:Other castle')]:
 w.add({'object_type':'N','object_id':identity,'osm_key':'natural' if kind=='peak' else 'tourism','osm_value':kind,
        'centroid':[8.03001,48.03001] if identity==4 else [8+identity/100,48+identity/100],'name':{'name':'Castle' if qid or wiki else 'Peak'},'extra':{'wikidata':qid,'wikipedia':wiki}})
add(w,[Path(sys.argv[2])/'landmarks.json'],[Path(sys.argv[2])/'peaks.json'],[7,47,9,49])
w.finish({'bounds':[7,47,9,49],'osm_sha256':'a'*64})
# The native offline shards carry the compact content, not photo payloads.
sys.path.insert(0,sys.argv[3])
from tools.planner_grid_search import search_lookup,search_shard
import sqlite3,json
source=Path(sys.argv[2])/'test.sqlite';lookup=Path(sys.argv[2])/'lookup.sqlite'
search_lookup(source,lookup)
with sqlite3.connect(source) as db: meta={k:json.loads(v) for k,v in db.execute('SELECT key,value FROM metadata')}
search_shard(source,lookup,Path(sys.argv[2])/'shard.sqlite',[7,47,9,49],meta)
`,new URL('..',import.meta.url).pathname,directory,new URL('../../..',import.meta.url).pathname]);
    db=openCells([join(directory,'shard.sqlite')]);
    const castles=await answerQuery(db,{q:'Castle',view:[7,47,9,49]},never,hours);
    assert.equal(castles.results.filter(p=>p.landmark_id==='Q1').length,1);
    assert.equal(castles.results.filter(p=>p.landmark_id==='Q5').length,1);
    assert.ok(!castles.results.some(p=>p.source==='Q5'));
    assert.ok(castles.results.some(p=>p.source==='n7'&&!p.content));
    assert.equal((await answerQuery(db,{q:'Wikipedia Castle',view:[7,47,9,49]},never,hours)).results[0].landmark_id,'Q5');
    assert.ok(castles.results.some(p=>p.source==='Q2'));
    assert.ok(!castles.results.some(p=>p.source==='Q3'));
    const selected=await answerQuery(db,{q:'',source:'Q2',view:[7,47,9,49]},never,hours);
    validateInput({q:'',source:'Q2',view:[7,47,9,49]});
    assert.deepEqual(selected.results[0].content.variants[0].text_pages,['A short article.']);
    assert.deepEqual(selected.results[0].content.photo.credit,article.photo.credit);
    assert.ok(!JSON.stringify(selected).includes('device-only'));
    assert.ok(!JSON.stringify(selected).includes('original_notices'));
    for(const id of [3,4]) {
      const peak=(await answerQuery(db,{q:'',source:`n${id}`,view:[7,47,9,49]},never,hours)).results[0];
      assert.equal(peak.lon,id===4?8.03001:8+id/100);
      assert.equal(peak.lat,id===4?48.03001:48+id/100);
      assert.equal(peak.content.variants[0].text_pages[0],'A short article.');
    }
    assert.equal((await answerQuery(db,{q:'Peak',view:[7,47,9,49]},never,hours)).results.length,2);
  } finally {db?.close();rmSync(directory,{recursive:true,force:true});}
});
