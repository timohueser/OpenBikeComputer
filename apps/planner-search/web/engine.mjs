import {norm,compact,streetNorm,editDistance,editBudget,spans} from './text.mjs';
import vocabulary from '../query/lexicon/kinds.json' with {type:'json'};
export {norm,editDistance} from './text.mjs';

export const DEFAULT_VIEW = [7.77, 47.965, 7.96, 48.06];
export const GROUPS = {
  water: ['drinking_water', 'water_point', 'spring', 'fountain'],
  sleep: ['campsite', 'hotel', 'hostel', 'guest_house', 'hut', 'shelter'],
  lodging: ['hotel', 'hostel', 'guest_house', 'motel', 'hut'],
  resupply: ['supermarket', 'convenience', 'bakery', 'butcher', 'marketplace'],
  food: ['restaurant', 'cafe', 'fast_food', 'bar', 'ice_cream'],
  bike: ['bike_shop', 'repair_station'],
  sight: ['museum', 'castle', 'viewpoint', 'summit', 'pass', 'waterfall', 'ruins'],
  pizza: ['restaurant', 'fast_food', 'cafe', 'pub', 'bar'],
  kebab: ['restaurant', 'fast_food', 'cafe', 'pub', 'bar'],
};
const CUISINES = {
  pizza: ['pizza', 'pizzas', 'pizzen', 'pizzeria', 'pizzerias', 'pizzerien'],
  kebab: ['kebab', 'kebap', 'doner', 'doner kebab', 'doner kebap', 'doener'],
};
const CATEGORY_WORDS = {
  bakery: 'bakery bakeries backerei backereien backer boulangerie boulangeries panetteria',
  supermarket: 'supermarket supermarkets supermarkt supermarkte supermarche',
  campsite: 'campsite campsites camping campground campingplatz campingplatze zelten',
  hotel: 'hotel hotels', hostel: 'hostel hostels', hut: 'hut huts hutte hutten refuge',
  pharmacy: 'pharmacy pharmacies apotheke apotheken pharmacie',
  restaurant: 'restaurant restaurants', cafe: 'cafe cafes coffee',
  water: 'water wasser eau', drinking_water: 'drinking water trinkwasser',
  bike_shop: 'fahrradladen',
  toilets: 'toilet toilets toilette toiletten wc', museum: 'museum museums museen',
  summit: 'peak peaks summit summits gipfel berg berge', pass: 'pass passes passe col',
  castle: 'castle castles burg burgen schloss schlosser',
  train_station: 'station stations bahnhof bahnhofe train station',
  sleep: 'accommodation unterkunft unterkunfte lodging',
  resupply: 'shop shops einkaufen laden lebensmittel', shelter: 'shelter shelters schutzhutte',
};

// Owner policy: geographic prominence may break close matches; business prominence is zero.
const PROMINENT = ['country','state','city','town','village','hamlet','summit','pass','volcano',
  'island','lake','waterfall','castle','ruins','monument','museum','attraction'];
const prominenceSQL = `CASE WHEN p.kind IN (${PROMINENT.map(k=>"'"+k+"'").join(',')})
  THEN MIN(1,MAX(0,COALESCE(p.importance,0)))*8 ELSE 0 END`;
function candidateOrder(view) {
  const [x,y]=center(view), scale=Math.cos(y*Math.PI/180);
  const km=`sqrt((p.lon-(${x}))*(p.lon-(${x}))*${scale*scale}+(p.lat-(${y}))*(p.lat-(${y})))*111.2`;
  return `(${prominenceSQL}+12/(1+(${km})/20)) DESC, p.source`;
}
const categoryTerms=new Map();
for(const per of Object.values(vocabulary.terms)) for(const [kind,terms] of Object.entries(per))
  for(const term of terms) if(!categoryTerms.has(norm(term))) categoryTerms.set(norm(term),kind);

export function cuisineOf(text) {
  return Object.keys(CUISINES).find(k=>CUISINES[k].includes(norm(text)))||null;
}
export function kindOf(text) {
  const q = norm(text);
  if(cuisineOf(q))return cuisineOf(q);
  for (const [kind, words] of Object.entries(CATEGORY_WORDS)) {
    if (q === norm(kind.replaceAll('_', ' ')) || words.split(' ').includes(q)) return kind;
  }
  if (['bike shop', 'bike shops', 'fahrrad geschaft'].includes(q)) return 'bike_shop';
  if (['drinking water', 'eau potable'].includes(q)) return 'drinking_water';
  return categoryTerms.get(q)||null;
}

export function distance(a, b) {
  const rad = Math.PI / 180;
  const y = (a[1] - b[1]) * rad;
  const x = (a[0] - b[0]) * rad * Math.cos((a[1] + b[1]) / 2 * rad);
  return 6371 * Math.hypot(x, y);
}
const center = b => [(b[0] + b[2]) / 2, (b[1] + b[3]) / 2];
export function around(point, km) {
  const dy = km / 111.2, dx = dy / Math.cos(point[1] * Math.PI / 180);
  return [point[0] - dx, point[1] - dy, point[0] + dx, point[1] + dy];
}
const pointBoundsSQL = 'p.lon>=? AND p.lat>=? AND p.lon<=? AND p.lat<=?';

// `ds` holds the kilometre of each route point. A position equals a scan of every segment
// (the first nearest segment wins): a block is skipped only when its box is farther than the best.
export function routePositions(route, ds) {
  const size = Math.ceil(Math.sqrt(route.length)), starts = [], blocks = [];
  let along = 0, maxLat = 0;
  for (let i = 1; i < route.length; i++) { starts.push(along); along += ds[i]-ds[i-1]; }
  for (let first = 1; first < route.length; first += size) {
    const last = Math.min(route.length-1, first+size-1), box = [Infinity, Infinity, -Infinity, -Infinity];
    for (let i = first-1; i <= last; i++) {
      const [x, y] = route[i];
      box[0] = Math.min(box[0], x); box[1] = Math.min(box[1], y); box[2] = Math.max(box[2], x); box[3] = Math.max(box[3], y);
      maxLat = Math.max(maxLat, Math.abs(y));
    }
    // The margin covers rounding of projected points.
    blocks.push({first, last, box: [box[0]-1e-9, box[1]-1e-9, box[2]+1e-9, box[3]+1e-9]});
  }
  const total = along;
  return point => {
    const cos = Math.cos(point[1] * Math.PI / 180), least = Math.cos(Math.max(maxLat, Math.abs(point[1])) * Math.PI / 180);
    // A lower bound of `distance` from the point to any point in a box.
    const bounds = blocks.map(({box}) => 6371 * Math.PI / 180 * Math.hypot(
      Math.max(0, box[0]-point[0], point[0]-box[2]) * least, Math.max(0, box[1]-point[1], point[1]-box[3])));
    let best = {distance: Infinity, along: 0, index: Infinity};
    const scan = ({first, last}) => {
      for (let i = first; i <= last; i++) {
        const a = route[i-1], b = route[i];
        const vx = (b[0]-a[0])*cos, vy = b[1]-a[1];
        const wx = (point[0]-a[0])*cos, wy = point[1]-a[1];
        const t = Math.max(0, Math.min(1, (vx*wx+vy*wy)/(vx*vx+vy*vy || 1)));
        const d = distance(point,[a[0]+t*(b[0]-a[0]),a[1]+t*(b[1]-a[1])]);
        if (d < best.distance || (d === best.distance && i < best.index)) best = {distance:d, along:starts[i-1]+t*(ds[i]-ds[i-1]), index:i};
      }
    };
    // The nearest box first gives a tight bound for the others.
    const nearest = bounds.indexOf(Math.min(...bounds));
    if (nearest >= 0) scan(blocks[nearest]);
    blocks.forEach((block, k) => { if (k !== nearest && bounds[k] <= best.distance) scan(block); });
    return {distance: best.distance, along: best.along, total};
  };
}

function expression(q) {
  const ts = norm(q).split(' ').filter(Boolean).slice(0,16);
  return ts.map((t,i)=>`"${t}"${i===ts.length-1?'*':''}`).join(' AND ');
}

function score(p,q,focus,fuzzy=false) {
  const normalize=p.kind==='street'?streetNorm:norm;
  const ns=[p.name,...p.aliases.split(';')].filter(Boolean).map(normalize);
  const text=normalize(q),tokens=text.split(' '), key=compact(text);
  let match=ns.includes(text)?100:ns.some(n=>n.startsWith(text))?88:
    ns.some(n=>tokens.every(t=>n.split(' ').some(x=>x.startsWith(t))))?74:58;
  if(ns.some(n=>compact(n)===key))match=Math.max(match,100);
  if(ns.some(n=>compact(n).startsWith(key)))match=Math.max(match,88);
  const context=norm(p.city+' '+p.postcode).split(' ');
  if(ns.some(n=>n&&(` ${text} `).includes(` ${n} `)&&norm(text.replace(n,'')).split(' ').filter(Boolean)
    .every(t=>context.some(c=>c.startsWith(t))))) match=Math.max(match,98);
  for(const n of ns) {
    const name=compact(n);
    const remainder=key.startsWith(name)?key.slice(name.length):key.endsWith(name)?key.slice(0,-name.length):null;
    if(remainder&&context.some(c=>compact(c).startsWith(remainder)))match=Math.max(match,98);
  }
  if(['city','town','village'].includes(p.kind)&&ns.some(n=>n.startsWith(text+' ')))match=Math.max(match,98);
  if (fuzzy) match-=8*fuzzy;
  const km=distance([p.lon,p.lat],focus);
  const proximity=12/(1+km/20), importance=PROMINENT.includes(p.kind)?Math.min(1,Math.max(0,p.importance))*8:0;
  // A small outdoor preference breaks close matches; proximity can still favour a town.
  const outdoor=match>=99&&['summit','pass'].includes(p.kind)?3:0;
  return {...p, distance:km, score:match+proximity+importance+outdoor,
    why:{match,proximity,importance,outdoor,correction:fuzzy}, precision:p.kind==='street'?'street':'place'};
}

function candidates(db, queries) {
  const rows=db.candidates?db.candidates(queries):queries.flatMap(({sql,params,options})=>db.all(sql,params,options));
  return [...new Map(rows.map(p=>[p.id,p])).values()];
}

function textCandidates(db, q, view, onlyPlaces=false) {
  let exp=expression(q);
  if(!exp) return [];
  if(!norm(q).includes(' '))exp='name : '+exp;
  if(streetNorm(q)!==norm(q))exp=`(${exp}) OR (${expression(streetNorm(q))})`;
  const restriction=onlyPlaces?" AND p.kind IN ('city','town','village','hamlet','locality','district','state','suburb')":'';
  const from=`FROM terms JOIN places p ON p.id=terms.rowid WHERE terms MATCH ?${restriction}`;
  const keys=[...new Set([...spans(q),...spans(streetNorm(q))].map(p=>p.term))];
  return candidates(db,[
    {sql:`SELECT p.* FROM names n JOIN places p ON p.id=n.place_id WHERE n.term=?${restriction}
      ORDER BY ${candidateOrder(view)} LIMIT 400`,params:[norm(q)]},
    {sql:`SELECT DISTINCT p.* FROM names n JOIN places p ON p.id=n.place_id WHERE n.term>=? AND n.term<?${restriction}
      ORDER BY ${candidateOrder(view)} LIMIT 100`,params:[norm(q),norm(q)+'\uffff']},
    // Both branches precede ranking: a local candidate survives a common global name.
    {sql:`SELECT p.* ${from} ORDER BY rank, ${candidateOrder(view)} LIMIT 400`,params:[exp]},
    {sql:`SELECT p.* ${from} AND ${pointBoundsSQL} ORDER BY ${candidateOrder(view)} LIMIT 800`,params:[exp,...view],options:{bounds:view}},
    ...compactQueries(keys,compact(q),view,restriction),
  ]);
}

function compactQueries(keys,prefix,view,restriction='') {
  const joined=`FROM compact_names n JOIN places p ON p.id=n.place_id WHERE n.term IN (${keys.map(()=>'?').join(',')})${restriction}`;
  // A common query fragment must not exhaust the budget for the complete name.
  return [
    ...keys.map(key=>({sql:`SELECT p.* FROM compact_names n JOIN places p ON p.id=n.place_id
      WHERE n.term=?${restriction} ORDER BY ${candidateOrder(view)} LIMIT 400`,params:[key]})),
    {sql:`SELECT DISTINCT p.* ${joined} AND ${pointBoundsSQL} ORDER BY ${candidateOrder(view)} LIMIT 800`,params:[...keys,...view],options:{bounds:view}},
    {sql:`SELECT DISTINCT p.* FROM compact_names n JOIN places p ON p.id=n.place_id
      WHERE n.term>=? AND n.term<?${restriction} ORDER BY ${candidateOrder(view)} LIMIT 100`,params:[prefix,prefix+'\uffff']},
  ];
}

function compactCandidates(db,keys,prefix,view,restriction='') {
  return candidates(db,compactQueries(keys,prefix,view,restriction));
}

export function named(db,q,view,onlyPlaces=false) {
  const results=textCandidates(db,q,view,onlyPlaces).map(p=>score(p,q,center(view)));
  if(/[aou]e/i.test(q)) {
    const alt=norm(q).replace(/ae/g,'a').replace(/oe/g,'o').replace(/ue/g,'u');
    results.push(...textCandidates(db,alt,view,onlyPlaces).map(p=>score(p,alt,center(view))));
  }
  if(!results.some(p=>p.why.match>=96)) {
    const parts=spans(q).filter(p=>editBudget(p.term));
    const key=compact(q), grams=[...new Set(Array.from({length:Math.max(0,key.length-2)},(_,i)=>key.slice(i,i+3)))];
    if(grams.length) {
      // Trigrams retrieve candidates only; edit distance decides whether a correction is allowed.
      const lengths=[...new Set(parts.flatMap(p=>Array.from({length:2*editBudget(p.term)+1},(_,i)=>p.term.length-editBudget(p.term)+i)))];
      const terms=lengths.length?db.all(`SELECT l.term, ${grams.map(()=>'(instr(l.term,?)>0)').join('+')} hits
        FROM fuzzy JOIN lexicon l ON l.rowid=fuzzy.rowid WHERE fuzzy MATCH ?
        AND length(l.term) IN (${lengths.map(()=>'?').join(',')}) ORDER BY hits DESC,rank LIMIT 256`,
        [...grams,grams.map(t=>`"${t}"`).join(' OR '),...lengths]):[];
      const corrections=[];
      for(const c of terms)for(const part of parts) {
        const budget=editBudget(part.term);
        if(Math.abs(c.term.length-part.term.length)>budget)continue;
        const edits=editDistance(c.term,part.term);
        if(edits>0&&edits<=budget)corrections.push({term:c.term,text:part.replace(c.term),edits});
      }
      const seen=new Set();
      for(const c of corrections.sort((a,b)=>a.edits-b.edits).filter(c=>{
        if(seen.has(c.text))return false;seen.add(c.text);return true;
      }).slice(0,8)) {
        const restriction=onlyPlaces?" AND p.kind IN ('city','town','village','hamlet','locality','district','state','suburb')":'';
        results.push(...compactCandidates(db,[c.term],c.term,view,restriction).map(p=>score(p,c.text,center(view),c.edits)));
      }
    }
  }
  return rank(results.filter(p=>p.why.match>=66));
}

function order(a,b) { return b.score-a.score || a.source.localeCompare(b.source); }

export function rank(results) { return [...results].sort(order); }

export function servesCuisine(p,cuisine) {
  if(String(p.cuisine||'').split(';').some(t=>cuisineOf(t)===cuisine))return true;
  // Business names recover useful matches where OSM has no cuisine tag.
  return (cuisine==='pizza'?/\bpizz(?:a|eri)/:/\b(?:doner|doener|kebab|kebap)/).test(norm(p.name+' '+p.aliases));
}

export function distinct(results) {
  const seen=new Set(), nearby=new Map();
  return results.filter(p=>{
    if(seen.has(p.source))return false;
    seen.add(p.source);
    const key=compact(p.kind==='street'?streetNorm(p.name):p.name)+'|'+p.kind, others=nearby.get(key)||[];
    // One street can have separate address groups across postcodes and municipal borders.
    const radius=p.kind==='street'?1:.02;
    if(others.some(o=>distance([o.lon,o.lat],[p.lon,p.lat])<radius))return false;
    others.push(p);nearby.set(key,others);return true;
  });
}

export function simpleRequest(q) {
  const nearby=norm(q).match(/^(.+?) (?:near me|around me|bei mir|in meiner nahe)$/);
  const kind=kindOf(nearby?.[1]||q);
  return kind?{type:'places',what:[kind],...(nearby?{where:{scope:'here'}}:{})}:
    {type:'place',name:q};
}

export function search(db,input) {
  const started=performance.now();
  const q=String(input.q||'').trim().slice(0,240), view=input.view||DEFAULT_VIEW;
  if(!q) return {results:[],area:'Map view',elapsed:0};
  const text=input.name||q, limit=Math.min(100,Math.max(1,input.limit||20));
  let results=[], note='';
  const number=[...text.matchAll(/\b\d{1,4}[a-z]?(?:[-/]\d+[a-z]?)?\b/gi)].at(-1);
  if(number) {
    const streetQuery=(text.slice(0,number.index)+' '+text.slice(number.index+number[0].length)).trim();
    const streets=named(db,streetQuery,view).filter(p=>p.kind==='street');
    for(const s of streets) {
      const homes=db.all('SELECT * FROM addresses WHERE street_id=? AND house=?',[s.id,norm(number[0])]);
      for(const h of homes) {
        const km=distance([h.lon,h.lat],center(view)), proximity=12/(1+km/20);
        const ranked={...s,distance:km,score:s.score-s.why.proximity+proximity,why:{...s.why,proximity}};
        results.push({...ranked,...h,name:`${s.name} ${number[0]}`,kind:'address',
          precision:'house',score:ranked.score+25,why:{...ranked.why,house:'Exact mapped house number'}});
      }
    }
    if(!results.length && streets.length) {
      results=streets.map(s=>({...s,precision:'street',why:{...s.why,house:'House number not mapped; street only'}}));
      note='House number not found. These are street-level results.';
    }
  }
  if(!results.length) results=named(db,text,view);
  if(input.withinKm!==undefined) results=results.filter(p=>distance([p.lon,p.lat],center(view))<=input.withinKm);
  results=distinct(rank(results));
  const total=results.length;
  return {results:results.slice(0,limit),hasMore:total>limit,matched:total,area:'All available data',note,
    elapsed:performance.now()-started};
}
