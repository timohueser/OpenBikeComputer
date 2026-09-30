import {DatabaseSync} from 'node:sqlite';
import {norm} from '../web/engine.mjs';
import {streetNorm,compact} from '../web/text.mjs';
export function database(extra = []) {
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
  ...extra,
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
conn.exec('CREATE VIRTUAL TABLE address_spatial USING rtree(id,west,east,south,north); INSERT INTO address_spatial SELECT rowid,lon,lon,lat,lat FROM addresses');

return {db,conn,records};
}
