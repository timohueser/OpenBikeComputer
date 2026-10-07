import {DEFAULT_VIEW,around} from './web/engine.mjs';

export function searchCases(db) {
  const queries = ['Kandel','Feldberg','Freiburg','Freibug','Xreiburg','Habsburgerstr. 10 Freiburg',
    'Kaiser Joseph Straße 9999 Freiburg','Media Markt','NediaMarkt','Mdeia Mrkt'];
  const cases = queries.map(q=>({q,view:DEFAULT_VIEW}));
  for (const p of db.rows({sql:'SELECT id,name,lon,lat FROM {c}.places WHERE id%12007=0 AND length(name)>4',order:['id']})) {
    cases.push({q:p.name,view:around([p.lon,p.lat],10)});
    cases.push({q:p.name.slice(0,-1),view:around([p.lon,p.lat],10)});
  }
  return cases;
}

export function reverseCases(db) {
  return db.rows({sql:'SELECT rowid AS _row,lon,lat FROM {c}.addresses WHERE rowid%3001=0',order:['_row']})
    .flatMap(p=>[[p.lon,p.lat],[p.lon+.0003,p.lat-.0003]]);
}
