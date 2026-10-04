import {DatabaseSync} from 'node:sqlite';
import {pathToFileURL} from 'node:url';
import {search,around,distance} from './web/engine.mjs';
import {reverseAddress} from './web/reverse.mjs';

const addressSQL=`SELECT a.source,a.house,a.lon,a.lat,p.name,p.city,coalesce(p.postcode,'') postcode
  FROM addresses a JOIN places p ON p.id=a.street_id ORDER BY a.source,a.house,p.name,p.city,p.postcode,a.lon,a.lat`;
const key=a=>JSON.stringify([a.source,a.house]);
function groups(rows) {
  const result=new Map();
  for(const row of rows) {
    const k=key(row);
    if(!result.has(k))result.set(k,[]);
    result.get(k).push(row);
  }
  return result;
}
const percent=(n,total)=>total?100*n/total:null;
const metres=(a,b)=>distance([a.lon,a.lat],[b.lon,b.lat])*1000;
const fields=['name','city','postcode'];
const semantic=(a,b)=>fields.every(k=>(a[k]??'')===(b[k]??''));
const resultSummary=r=>r===null?null:Object.fromEntries(['source','name','city','postcode','precision','lon','lat'].map(k=>[k,r[k]]));

export function compareAddresses(candidate,reference,{sampleSize=500,coordinateMetres=1}={}) {
  if(!Number.isSafeInteger(sampleSize)||sampleSize<1||!Number.isFinite(coordinateMetres)||coordinateMetres<0)
    throw new Error('Use a positive sample size and a non-negative coordinate tolerance');
  const actual=candidate.all(addressSQL),expected=reference.all(addressSQL);
  const a=groups(actual),b=groups(expected);
  const present=[...b.keys()].filter(k=>a.has(k)).length;
  const agreement=Object.fromEntries([...fields,'coordinate','all'].map(k=>[k,0]));
  const mismatches=[];
  for(const row of expected) {
    const matches=a.get(key(row))??[];
    for(const f of fields)if(matches.some(m=>(m[f]??'')===(row[f]??'')))agreement[f]++;
    if(matches.some(m=>metres(row,m)<=coordinateMetres))agreement.coordinate++;
    if(matches.some(m=>semantic(m,row)&&metres(row,m)<=coordinateMetres))agreement.all++;
    else if(mismatches.length<12)mismatches.push({reference:row,candidate:matches.slice(0,2)});
  }
  const unique=[...b.values()].map(rows=>rows[0]);
  const sampled=unique.filter((_,i)=>i%Math.max(1,Math.ceil(unique.length/sampleSize))===0);
  const forward={count:0,sameTopResult:0,sameLabel:0,referenceHouse:0,preservedHouse:0,referenceStreet:0,preservedStreetLabel:0,examples:[]};
  const streetDistances=[];
  const reverse={count:0,sameLabel:0,examples:[]};
  for(const row of sampled) {
    for(const q of [`${row.name} ${row.house} ${row.city}`,`${row.name} ${row.city}`]) {
      const input={q,view:around([row.lon,row.lat],10),limit:5};
      const expected=search(reference,input).results[0]??null;
      const actual=search(candidate,input).results[0]??null;
      const sameLabel=expected===null?actual===null:actual!==null&&expected.name===actual.name&&
        expected.city===actual.city&&expected.precision===actual.precision;
      const same=sameLabel&&(expected===null||metres(expected,actual)<=coordinateMetres);
      forward.count++;
      if(same)forward.sameTopResult++;
      if(sameLabel)forward.sameLabel++;
      if(expected?.precision==='house') {
        forward.referenceHouse++;
        if(same)forward.preservedHouse++;
      }
      if(expected?.precision==='street') {
        forward.referenceStreet++;
        if(sameLabel) {
          forward.preservedStreetLabel++;
          streetDistances.push(metres(expected,actual));
        }
      }
      if(!same&&forward.examples.length<8)forward.examples.push({q,reference:resultSummary(expected),candidate:resultSummary(actual)});
    }
    for(const point of [[row.lon,row.lat],[row.lon+.0003,row.lat-.0003]]) {
      const expected=reverseAddress(reference,point),actual=reverseAddress(candidate,point);
      reverse.count++;
      if(expected===actual)reverse.sameLabel++;
      else if(reverse.examples.length<8)reverse.examples.push({point,reference:expected,candidate:actual});
    }
  }
  streetDistances.sort((a,b)=>a-b);
  const quantile=q=>streetDistances.length?streetDistances[Math.floor((streetDistances.length-1)*q)]:null;
  return {scope:'addresses and streets; POI/locality search is not evaluated',
    referenceRows:expected.length,candidateRows:actual.length,referenceIdentities:b.size,candidateIdentities:a.size,
    duplicateRows:{reference:expected.length-b.size,candidate:actual.length-a.size},
    identity:{present,missing:b.size-present,extra:a.size-present,recallPercent:percent(present,b.size),precisionPercent:percent(present,a.size)},
    fields:{coordinateToleranceMetres:coordinateMetres,agreement,percent:Object.fromEntries(Object.entries(agreement).map(([k,n])=>[k,percent(n,expected.length)]))},
    forward:{...forward,agreementPercent:percent(forward.sameTopResult,forward.count),labelAgreementPercent:percent(forward.sameLabel,forward.count),housePreservedPercent:percent(forward.preservedHouse,forward.referenceHouse),streetLabelPreservedPercent:percent(forward.preservedStreetLabel,forward.referenceStreet),streetDisplacementMetres:{count:streetDistances.length,median:quantile(.5),p95:quantile(.95),max:quantile(1)}},
    reverse:{...reverse,agreementPercent:percent(reverse.sameLabel,reverse.count)},mismatches};
}

function open(file) {
  const conn=new DatabaseSync(file,{readOnly:true});
  conn.exec('PRAGMA cache_size=-32768; PRAGMA mmap_size=0');
  const statements=new Map();
  return {conn,all(sql,params=[]) {
    if(!statements.has(sql))statements.set(sql,conn.prepare(sql));
    return statements.get(sql).all(...params);
  }};
}

if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href) {
  const [candidate,reference,sample='500']=process.argv.slice(2);
  if(!candidate||!reference)throw new Error('Usage: node address-parity.mjs CANDIDATE.sqlite REFERENCE.sqlite [SAMPLE_SIZE]');
  const a=open(candidate),b=open(reference);
  try {
    const identity=db=>JSON.parse(db.all("SELECT value FROM metadata WHERE key='osm_sha256'")[0]?.value??'null');
    if(!identity(a)||identity(a)!==identity(b))throw new Error('Packages must name the same OSM snapshot');
    console.log(JSON.stringify({osmSha256:identity(a),...compareAddresses(a,b,{sampleSize:Number(sample)})},null,2));
  } finally {a.conn.close();b.conn.close()}
}
