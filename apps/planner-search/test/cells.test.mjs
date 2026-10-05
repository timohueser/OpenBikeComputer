import {test} from 'node:test';
import assert from 'node:assert/strict';
import {database} from './database.mjs';
import {openCells} from '../cells.mjs';
import {search} from '../web/engine.mjs';
import {reverseAddress} from '../web/reverse.mjs';
import {findPlaces,resolvePoint} from '../resolver.mjs';

const normalize = value => {
  if(typeof value==='number')return Math.round(value*1e8)/1e8;
  if(Array.isArray(value))return value.map(normalize);
  if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).filter(([k])=>k!=='elapsed').map(([k,v])=>[k,normalize(v)]));
  return value;
};

test('cell queries preserve global limits, spelling, addresses and duplicate ownership beyond eight attachments',()=>{
  const extra=Array.from({length:850},(_,i)=>[`n${100+i}`,'Hotel Summit','hotel',7.8+i%30*.001,47.95+Math.floor(i/30)*.001,'Freiburg',.1+i/10000]);
  const reference=database(extra),parts=[];
  let cells;
  try {
    for(let i=0;i<12;i++) {
      const part=database(extra);parts.push(part);
      // Some complete records occur in adjacent cells; ids remain source ids.
      part.conn.prepare('DELETE FROM place_records WHERE id%12!=? AND id%5!=0').run(i);
    }
    cells=openCells(parts.map(part=>part.file));
    const view=[7.8,47.9,7.95,48.05];
    for(const q of ['Hotel Summit','Kandel','Kandell','Kaiser Joseph Str 12 Freiburg','bakery','Habsburgerstraße 10']) {
      assert.deepEqual(normalize(search(cells,{q,view})),normalize(search(reference.db,{q,view})),q);
    }
    const hotels={type:'places',what:['hotel']},context={view,limit:100};
    assert.deepEqual(normalize(findPlaces(cells,hotels,context)),normalize(findPlaces(reference.db,hotels,context)));
    assert.deepEqual(resolvePoint(cells,{kind:'hotel'},context,[7.83,47.97]),resolvePoint(reference.db,{kind:'hotel'},context,[7.83,47.97]));
    for(const point of [[7.851,47.991],[7.854,48.01],[11.57,48.13]]) {
      assert.deepEqual(normalize(reverseAddress(cells,point)),normalize(reverseAddress(reference.db,point)));
    }
  } finally {
    cells?.close();
    for(const fixture of [reference,...parts]){fixture.db.close();fixture.conn.close();}
  }
});

test('a bounded query skips cells outside its bounds and keeps a cell that touches them',()=>{
  const near=database(),far=database();
  let cells;
  try {
    near.conn.exec('DELETE FROM place_records WHERE lon>10');
    far.conn.exec('DELETE FROM place_records WHERE lon<10');
    near.conn.prepare("INSERT INTO metadata VALUES ('bounds',?)").run(JSON.stringify([7.8,47.9,7.85,48.1]));
    far.conn.prepare("INSERT INTO metadata VALUES ('bounds',?)").run(JSON.stringify([11,48,12,49]));
    cells=openCells([near.file,far.file]);
    const sources=cells.rows({sql:'SELECT p.source FROM {c}.place_records p',bounds:[7.85,47.98,7.9,48]}).map(row=>row.source);
    assert.ok(sources.includes('n1'),'n1 lies on the east edge of the near cell');
    assert.ok(!sources.includes('r5'),'the far cell is skipped');
    assert.ok(cells.rows({sql:'SELECT p.source FROM {c}.place_records p',bounds:[11.5,48.1,11.6,48.2]}).every(row=>row.source!=='n1'));
  } finally {
    cells?.close();
    for(const fixture of [near,far]){fixture.db.close();fixture.conn.close();}
  }
});
