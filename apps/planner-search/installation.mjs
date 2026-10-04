import {DatabaseSync} from 'node:sqlite';
import {existsSync,readFileSync,statSync} from 'node:fs';
import path from 'node:path';
import {openCells} from './cells.mjs';

const boundsEqual=(a,b)=>JSON.stringify(a)===JSON.stringify(b);
const validBounds=b=>Array.isArray(b)&&b.length===4&&b.every(Number.isFinite)&&b[0]<b[2]&&b[1]<b[3];

function readMetadata(file) {
  if(!existsSync(file))throw new Error(`Missing search database: ${file}`);
  const db=new DatabaseSync(file,{readOnly:true});
  try {
    db.prepare('SELECT rowid FROM addresses INDEXED BY address_cells LIMIT 0');
    return Object.fromEntries(db.prepare('SELECT * FROM metadata').all().map(r=>[r.key,JSON.parse(r.value)]));
  } finally {db.close()}
}

/** Components retain their indexes; the query runtime sees a single logical database. */
export function openRegion(data,region) {
  const gridFile=path.join(data,`${region}.grid.json`);
  let files,metadata;
  if(existsSync(gridFile)) {
    const grid=JSON.parse(readFileSync(gridFile,'utf8'));
    if(![2,3].includes(grid.format)||grid.metadata?.schema!==4||!Array.isArray(grid.cells)||
      !grid.cells.length||new Set(grid.cells.map(c=>c.id)).size!==grid.cells.length||
      grid.cells.some(c=>!/^9-[0-9]+-[0-9]+$/.test(c.id)||!validBounds(c.bounds)))throw new Error('Invalid search grid.');
    files=grid.cells.flatMap(c=>{
      const names=grid.format===2?[`tiles/${c.id}.sqlite`]:c.files;
      if(!Array.isArray(names)||!names.length||(grid.format===3&&names.length!==2)||new Set(names).size!==names.length||names.some(name=>
        typeof name!=='string'||!(grid.format===2?name===`tiles/${c.id}.sqlite`:
          [`tiles/pois/${c.id}.sqlite`,`tiles/addresses/${c.id}.sqlite`].includes(name))))throw new Error('Invalid search grid files.');
      return names.map(name=>({file:path.join(data,name),bounds:c.bounds,
        component:grid.format===3?name.split('/')[1]:'all'}));
    });
    metadata=grid.metadata;
  } else {
    const split=['pois','addresses'].map(component=>({file:path.join(data,component,`${region}.sqlite`),component}));
    if(split.some(c=>existsSync(c.file)))files=split;
    else {
      const file=path.join(data,`${region}.sqlite`);
      if(!existsSync(file))return null;
      files=[{file,component:'all'}];
    }
    metadata=readMetadata(files[0].file);
    files=files.map(f=>({...f,bounds:metadata.bounds}));
  }
  const counts={};
  if(files.some(f=>f.component!=='all')&&(!validBounds(metadata.bounds)||
    !/^[a-f0-9]{64}$/.test(metadata.osm_sha256||'')))throw new Error('Search components need source identity and coverage.');
  for(const file of files) {
    const m=readMetadata(file.file);
    if(m.schema!==4||m.osm_sha256!==metadata.osm_sha256||!boundsEqual(m.bounds,file.bounds)||
      (file.component!=='all'&&m.component!==file.component))throw new Error('Search components have incompatible provenance or coverage.');
    for(const [kind,count] of Object.entries(m.counts||{}))counts[kind]=(counts[kind]||0)+count;
  }
  if(!existsSync(gridFile))metadata={...metadata,component:files.length===1?files[0].component:'all',counts};
  const db=openCells(files,metadata);
  return {db,metadata,bytes:files.reduce((n,c)=>n+statSync(c.file).size,0),close:db.close};
}
