import {existsSync,readFileSync,statSync} from 'node:fs';
import path from 'node:path';
import {openCells} from './cells.mjs';

const componentFiles = id => [`tiles/pois/${id}.sqlite`,`tiles/addresses/${id}.sqlite`];

/** A release installs a split search grid; a local bake installs the region's component files. */
export function openRegion(data,region) {
  const gridFile=path.join(data,`${region}.grid.json`);
  let files;
  if(existsSync(gridFile)) {
    const grid=JSON.parse(readFileSync(gridFile,'utf8'));
    if(grid.format!==3||!Array.isArray(grid.cells)||!grid.cells.length||
      new Set(grid.cells.map(c=>c.id)).size!==grid.cells.length||grid.cells.some(c=>!/^9-[0-9]+-[0-9]+$/.test(c.id)))
      throw new Error('Invalid search grid.');
    if(grid.cells.some(c=>JSON.stringify(c.files)!==JSON.stringify(componentFiles(c.id))))throw new Error('Invalid search grid files.');
    files=grid.cells.flatMap(c=>c.files);
  } else {
    files=['pois','addresses'].map(component=>`${component}/${region}.sqlite`).filter(name=>existsSync(path.join(data,name)));
    if(!files.length)files=[`${region}.sqlite`].filter(name=>existsSync(path.join(data,name)));
    if(!files.length)return null;
  }
  files=files.map(name=>path.join(data,name));
  return {db:openCells(files),bytes:files.reduce((n,file)=>n+statSync(file).size,0)};
}
