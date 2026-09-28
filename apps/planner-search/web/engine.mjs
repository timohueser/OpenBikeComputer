import {norm,compact,streetNorm,editDistance,editBudget,spans} from './text.mjs';
import vocabulary from '../query/lexicon/kinds.json' with {type:'json'};
export {norm,editDistance} from './text.mjs';

export const DEFAULT_VIEW = [7.77, 47.965, 7.96, 48.06];
export const REGION_BOUNDS = [7.51, 47.53, 10.5, 49.8];
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
  bike_shop: 'bike shop bike shops fahrradladen fahrradladen',
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
const inBox = (p, b) => p.lon >= b[0] && p.lon <= b[2] && p.lat >= b[1] && p.lat <= b[3];
const bboxSQL = 'p.id IN (SELECT id FROM spatial WHERE east>=? AND north>=? AND west<=? AND south<=?)';

export function routePosition(point, route) {
  let best = {distance: Infinity, along: 0}, along = 0;
  for (let i = 1; i < route.length; i++) {
    const a = route[i-1], b = route[i], cos = Math.cos(point[1] * Math.PI / 180);
    const vx = (b[0]-a[0])*cos, vy = b[1]-a[1];
    const wx = (point[0]-a[0])*cos, wy = point[1]-a[1];
    const t = Math.max(0, Math.min(1, (vx*wx+vy*wy)/(vx*vx+vy*vy || 1)));
    const len = distance(a,b), d = distance(point,[a[0]+t*(b[0]-a[0]),a[1]+t*(b[1]-a[1])]);
    if (d < best.distance) best = {distance:d, along:along+t*len};
    along += len;
  }
  return {...best, total:along};
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
  if(['city','town','village'].includes(p.kind)&&ns.some(n=>n.startsWith(text+' ')))match=Math.max(match,95);
  if (fuzzy) match-=8*fuzzy;
  const km=distance([p.lon,p.lat],focus);
  const proximity=12/(1+km/20), importance=PROMINENT.includes(p.kind)?Math.min(1,Math.max(0,p.importance))*8:0;
  // A small outdoor preference breaks close matches; proximity can still favour a town.
  const outdoor=match>=99&&['summit','pass'].includes(p.kind)?3:0;
  return {...p, distance:km, score:match+proximity+importance+outdoor,
    why:{match,proximity,importance,outdoor,correction:fuzzy}, precision:p.kind==='street'?'street':'place'};
}

function textCandidates(db, q, view, onlyPlaces=false) {
  let exp=expression(q);
  if(!exp) return [];
  if(!norm(q).includes(' '))exp='name : '+exp;
  if(streetNorm(q)!==norm(q))exp=`(${exp}) OR (${expression(streetNorm(q))})`;
  const restriction=onlyPlaces?" AND p.kind IN ('city','town','village','hamlet','locality','district','state','suburb')":'';
  const from=`FROM terms JOIN places p ON p.id=terms.rowid WHERE terms MATCH ?${restriction}`;
  const exact=db.all(`SELECT p.* FROM names n JOIN places p ON p.id=n.place_id WHERE n.term=?${restriction}
    ORDER BY ${candidateOrder(view)} LIMIT 400`,[norm(q)]);
  const prefix=db.all(`SELECT DISTINCT p.* FROM names n JOIN places p ON p.id=n.place_id WHERE n.term>=? AND n.term<?${restriction}
    ORDER BY ${candidateOrder(view)} LIMIT 100`,[norm(q),norm(q)+'\uffff']);
  // Both branches precede ranking: a local candidate survives a common global name.
  const global=db.all(`SELECT p.* ${from} ORDER BY rank, ${candidateOrder(view)} LIMIT 400`,[exp]);
  const local=db.all(`SELECT p.* ${from} AND ${bboxSQL} ORDER BY ${candidateOrder(view)} LIMIT 800`,[exp,...view]);
  const keys=[...new Set([...spans(q),...spans(streetNorm(q))].map(p=>p.term))];
  const joined=compactCandidates(db,keys,compact(q),view,restriction);
  return [...new Map([...exact,...prefix,...global,...local,...joined].map(p=>[p.id,p])).values()];
}

function compactCandidates(db,keys,prefix,view,restriction='') {
  const joined=`FROM compact_names n JOIN places p ON p.id=n.place_id WHERE n.term IN (${keys.map(()=>'?').join(',')})${restriction}`;
  // A common query fragment must not exhaust the budget for the complete name.
  const compactGlobal=keys.flatMap(key=>db.all(`SELECT p.* FROM compact_names n JOIN places p ON p.id=n.place_id
    WHERE n.term=?${restriction} ORDER BY ${candidateOrder(view)} LIMIT 400`,[key]));
  const compactLocal=db.all(`SELECT DISTINCT p.* ${joined} AND ${bboxSQL} ORDER BY ${candidateOrder(view)} LIMIT 800`,[...keys,...view]);
  const compactPrefix=db.all(`SELECT DISTINCT p.* FROM compact_names n JOIN places p ON p.id=n.place_id
    WHERE n.term>=? AND n.term<?${restriction} ORDER BY ${candidateOrder(view)} LIMIT 100`,[prefix,prefix+'\uffff']);
  return [...new Map([...compactGlobal,...compactLocal,...compactPrefix].map(p=>[p.id,p])).values()];
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

function serves(p,cuisine) {
  const tagged=String(p.cuisine||'').split(';').map(norm);
  if(tagged.some(t=>cuisineOf(t)===cuisine))return 'OSM cuisine tag';
  // Business names recover useful matches where OSM has no cuisine tag.
  const name=norm(p.name+' '+p.aliases);
  if((cuisine==='pizza'?/\b(?:pizza\p{L}*|pizzeri\p{L}*)\b/u:/\b(?:doner\p{L}*|doener\p{L}*|kebab\p{L}*|kebap\p{L}*)\b/u).test(name))return 'Business name';
  return null;
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
  const req=input.request||simpleRequest(q), limit=Math.min(100,Math.max(1,input.limit||20));
  let results=[], area='All available data', note='', resolved=null;
  if(req.unsupported) return {results:[],area:'Not searched',note:req.unsupported,elapsed:0};
  if(req.type==='places') {
    const where=req.where||{}, kinds=[...new Set((req.what||[]).flatMap(k=>GROUPS[k]||[k]))];
    const cuisines=(req.what||[]).filter(k=>CUISINES[k]);
    let bounds=view, focus=center(view), route=null, range=null, radial=null;
    area='Visible map area';
    if(where.scope==='here') {
      if(!input.here) return {results:[],area:'Near me',note:'Set your location to use “near me”.',elapsed:0};
      focus=input.here; radial=req.radius||5; bounds=around(focus,radial); area=`Within ${radial} km of your location`;
    }
    if(where.near) {
      const places=named(db,where.near,view,true);
      if(!places.length) return {results:[],area:where.near,note:'Place not found in this data package.',unresolved:true,elapsed:performance.now()-started};
      resolved=places[0]; focus=[resolved.lon,resolved.lat];
      if(where.in) bounds=[resolved.west,resolved.south,resolved.east,resolved.north];
      else { radial=req.radius||5; bounds=around(focus,radial); }
      area=`${where.in?'In':'Near'} ${resolved.name}`;
    }
    if(where.scope==='route' || where.day || where.part) {
      const plan=input.plan;
      if(!plan?.coordinates?.length) return {results:[],area:'Route',note:'Load a route to use this search.',elapsed:0};
      route=plan.coordinates;
      if(where.day) {
        const end=plan.days?.[where.day-1];
        if(end===undefined) return {results:[],area:'Route',note:`The plan has no day ${where.day}.`,elapsed:0};
        const start=where.day===1?0:plan.days[where.day-2];
        route=route.slice(start,end+1);
        if(where.part==='end' || where.part==='start') {
          focus=where.part==='end'?route.at(-1):route[0]; radial=req.radius||3;
          bounds=around(focus,radial); route=null; area=`Within ${radial} km of Day ${where.day} ${where.part}`;
        } else area=`Along Day ${where.day}`;
      } else area='Along the route';
      if(route) {
        const xs=route.map(p=>p[0]),ys=route.map(p=>p[1]);
        const pad=around(center([Math.min(...xs),Math.min(...ys),Math.max(...xs),Math.max(...ys)]),req.radius||1);
        const dx=(pad[2]-pad[0])/2,dy=(pad[3]-pad[1])/2;
        bounds=[Math.min(...xs)-dx,Math.min(...ys)-dy,Math.max(...xs)+dx,Math.max(...ys)+dy];
        if(where.part==='first_half') {range=[0,.5];area='First half of the route';}
        if(where.part==='last_half') {range=[.5,1];area='Last half of the route';}
      }
    }
    const rows=db.all(`SELECT p.* FROM places p WHERE p.kind IN (${kinds.map(()=>'?').join(',')})
      AND ${bboxSQL}`,[...kinds,...bounds]);
    results=rows.filter(p=>inBox(p,bounds)&&(!cuisines.length||cuisines.some(c=>serves(p,c)))).map(p=>{
      const km=distance([p.lon,p.lat],focus);
      const position=route?routePosition([p.lon,p.lat],route):null;
      return {...p,distance:km,position,score:position?-position.along:-km,
        why:{category:p.kind,...(cuisines.length?{cuisine:cuisines,match:cuisines.map(c=>serves(p,c)).filter(Boolean)}:{}),
          order:position?'Position along route':'Distance from search centre'},precision:'place'};
    }).filter(p=>(!radial||p.distance<=radial)&&(!route||(p.position.distance<=(req.radius||1)&&
      (!range||(p.position.along>=range[0]*p.position.total&&p.position.along<=range[1]*p.position.total)))));
    if(where.in && resolved) {
      // Address membership avoids including neighbouring municipalities inside a city's rectangle.
      const cityNames=[resolved.name,...resolved.aliases.split(';')].map(norm);
      results=results.filter(p=>cityNames.includes(norm(p.city)));
      note='City membership comes from OSM address context.';
    }
    resolved={...resolved,bounds};
  } else {
    const text=req.name||q;
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
  }
  results=distinct(rank(results));
  const total=results.length;
  return {results:results.slice(0,limit),hasMore:total>limit,matched:total,area,note,resolved,
    request:req,elapsed:performance.now()-started};
}
