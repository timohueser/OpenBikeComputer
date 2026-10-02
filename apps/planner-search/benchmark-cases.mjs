import {DEFAULT_VIEW,around} from './web/engine.mjs';

export function searchCases(db) {
  const queries = ['Kandel','Feldberg','Freiburg','Freibug','Xreiburg','Habsburgerstr. 10 Freiburg',
    'Kaiser Joseph Straße 9999 Freiburg','Media Markt','NediaMarkt','Mdeia Mrkt','pizza','Döner','bakery'];
  const cases = queries.map(q=>({q,view:DEFAULT_VIEW}));
  for (const p of db.all('SELECT name,lon,lat FROM places WHERE id%12007=0 AND length(name)>4 ORDER BY id')) {
    cases.push({q:p.name,view:around([p.lon,p.lat],10)});
    cases.push({q:p.name.slice(0,-1),view:around([p.lon,p.lat],10)});
  }
  return cases;
}

export function reverseCases(db) {
  return db.all('SELECT lon,lat FROM addresses WHERE rowid%3001=0 ORDER BY rowid')
    .flatMap(p=>[[p.lon,p.lat],[p.lon+.0003,p.lat-.0003]]);
}
