import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {database} from './database.mjs';
import {openCells} from '../cells.mjs';
import {search} from '../web/engine.mjs';
import {reverseAddress} from '../web/reverse.mjs';

const normalize = value => {
  if(typeof value==='number')return Math.round(value*1e8)/1e8;
  if(Array.isArray(value))return value.map(normalize);
  if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).filter(([k])=>k!=='elapsed').map(([k,v])=>[k,normalize(v)]));
  return value;
};

test('cell queries preserve global limits, spelling, addresses and duplicate ownership beyond eight attachments',()=>{
  const root=mkdtempSync(path.join(os.tmpdir(),'planner-cells-'));
  const extra=Array.from({length:850},(_,i)=>[`n${100+i}`,'Hotel Summit','hotel',7.8+i%30*.001,47.95+Math.floor(i/30)*.001,'Freiburg',.1+i/10000]);
  const reference=database(extra),files=[];
  let cells;
  try {
    for(let i=0;i<12;i++) {
      const file=path.join(root,`${i}.sqlite`),part=database(extra,file);
      // Some complete records occur in adjacent cells; ids remain source ids.
      part.conn.prepare('DELETE FROM place_records WHERE id%12!=? AND id%5!=0').run(i);
      part.conn.close();files.push({file});
    }
    cells=openCells(files,{schema:3});
    const view=[7.8,47.9,7.95,48.05];
    for(const q of ['Hotel Summit','Kandel','Kandell','Kaiser Joseph Str 12 Freiburg','bakery','Habsburgerstraße 10']) {
      assert.deepEqual(normalize(search(cells,{q,view})),normalize(search(reference.db,{q,view})),q);
    }
    for(const point of [[7.851,47.991],[7.854,48.01],[11.57,48.13]]) {
      assert.deepEqual(normalize(reverseAddress(cells,point)),normalize(reverseAddress(reference.db,point)));
    }
  } finally {cells?.close();reference.conn.close();rmSync(root,{recursive:true,force:true})}
});
