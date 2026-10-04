import {DatabaseSync} from 'node:sqlite';
import {pathToFileURL} from 'node:url';
import {search,around,distance} from './web/engine.mjs';
import {reverseAddress} from './web/reverse.mjs';

const addressSQL=`SELECT a.source,a.house,a.lon,a.lat,p.name,p.city,coalesce(p.postcode,'') postcode
  FROM addresses a JOIN places p ON p.id=a.street_id ORDER BY a.source,a.house,p.name,p.city,p.postcode,a.lon,a.lat`;
const compareKey=(a,b)=>Buffer.compare(Buffer.from(a.source),Buffer.from(b.source))||Buffer.compare(Buffer.from(a.house),Buffer.from(b.house));
function* groups(db) {
  let group=[];
  for(const row of db.iterate?.(addressSQL)??db.all(addressSQL)) {
    if(group.length&&compareKey(group[0],row)!==0) {yield group;group=[]}
    group.push(row);
  }
  if(group.length)yield group;
}
const percent=(n,total)=>total?100*n/total:null;
const metres=(a,b)=>distance([a.lon,a.lat],[b.lon,b.lat])*1000;
const fields=['name','city','postcode'];
const semantic=(a,b)=>fields.every(k=>(a[k]??'')===(b[k]??''));
const resultSummary=r=>r===null?null:Object.fromEntries(['source','name','city','postcode','precision','lon','lat'].map(k=>[k,r[k]]));

export function compareAddresses(candidate,reference,{sampleSize=500,coordinateMetres=1}={}) {
  if(!Number.isSafeInteger(sampleSize)||sampleSize<1||!Number.isFinite(coordinateMetres)||coordinateMetres<0)
    throw new Error('Use a positive sample size and a non-negative coordinate tolerance');
  const referenceIdentities=reference.all('SELECT count(*) n FROM (SELECT source,house FROM addresses GROUP BY source,house)')[0].n;
  const stride=Math.max(1,Math.ceil(referenceIdentities/sampleSize));
  let referenceRows=0,candidateRows=0,candidateIdentities=0,present=0,extra=0;
  const agreement=Object.fromEntries([...fields,'coordinate','all'].map(k=>[k,0]));
  const mismatches=[],sampled=[];
  const actual=groups(candidate);
  let current=actual.next();
  const advance=()=>{candidateRows+=current.value.length;candidateIdentities++;current=actual.next()};
  let position=0;
  for(const expected of groups(reference)) {
    const row=expected[0];
    while(!current.done&&compareKey(current.value[0],row)<0) {extra++;advance()}
    const matches=!current.done&&compareKey(current.value[0],row)===0?current.value:[];
    if(matches.length)present++;
    for(const row of expected) {
      referenceRows++;
      for(const f of fields)if(matches.some(m=>(m[f]??'')===(row[f]??'')))agreement[f]++;
      if(matches.some(m=>metres(row,m)<=coordinateMetres))agreement.coordinate++;
      if(matches.some(m=>semantic(m,row)&&metres(row,m)<=coordinateMetres))agreement.all++;
      else if(mismatches.length<12)mismatches.push({reference:row,candidate:matches.slice(0,2)});
    }
    if(position++%stride===0)sampled.push(row);
    if(matches.length)advance();
  }
  while(!current.done) {extra++;advance()}
  const forward={count:0,sameTopResult:0,sameLabel:0,referenceHouse:0,preservedHouse:0,referenceStreet:0,preservedStreetLabel:0,examples:[]};
  const streetDistances=[];
  const reverse={count:0,sameLabel:0,examples:[]};
  for(const row of sampled) {
    const queries=[`${row.name} ${row.house} ${row.city}`,`${row.name} ${row.city}`];
    if(row.postcode)queries.push(`${row.name} ${row.house} ${row.postcode}`);
    for(const q of queries) {
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
    referenceRows,candidateRows,referenceIdentities,candidateIdentities,
    duplicateRows:{reference:referenceRows-referenceIdentities,candidate:candidateRows-candidateIdentities},
    identity:{present,missing:referenceIdentities-present,extra,recallPercent:percent(present,referenceIdentities),precisionPercent:percent(present,candidateIdentities)},
    fields:{coordinateToleranceMetres:coordinateMetres,agreement,percent:Object.fromEntries(Object.entries(agreement).map(([k,n])=>[k,percent(n,referenceRows)]))},
    forward:{...forward,agreementPercent:percent(forward.sameTopResult,forward.count),labelAgreementPercent:percent(forward.sameLabel,forward.count),housePreservedPercent:percent(forward.preservedHouse,forward.referenceHouse),streetLabelPreservedPercent:percent(forward.preservedStreetLabel,forward.referenceStreet),streetDisplacementMetres:{count:streetDistances.length,median:quantile(.5),p95:quantile(.95),max:quantile(1)}},
    reverse:{...reverse,agreementPercent:percent(reverse.sameLabel,reverse.count)},mismatches};
}

export function equivalent(report) {
  return report.identity.missing===0&&report.identity.extra===0&&report.fields.agreement.all===report.referenceRows&&
    report.forward.sameTopResult===report.forward.count&&report.reverse.sameLabel===report.reverse.count;
}

function open(file) {
  const conn=new DatabaseSync(file,{readOnly:true});
  conn.exec('PRAGMA cache_size=-32768; PRAGMA mmap_size=0');
  const statements=new Map();
  return {conn,iterate(sql) {return conn.prepare(sql).iterate()},all(sql,params=[]) {
    if(!statements.has(sql))statements.set(sql,conn.prepare(sql));
    return statements.get(sql).all(...params);
  }};
}

if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href) {
  const [candidate,reference,sample='500']=process.argv.slice(2).filter(arg=>arg!=='--require-equivalent');
  if(!candidate||!reference)throw new Error('Usage: node address-parity.mjs CANDIDATE.sqlite REFERENCE.sqlite [SAMPLE_SIZE]');
  const a=open(candidate),b=open(reference);
  try {
    const identity=db=>JSON.parse(db.all("SELECT value FROM metadata WHERE key='osm_sha256'")[0]?.value??'null');
    if(!identity(a)||identity(a)!==identity(b))throw new Error('Packages must name the same OSM snapshot');
    const report={osmSha256:identity(a),...compareAddresses(a,b,{sampleSize:Number(sample)})};
    console.log(JSON.stringify({...report,equivalent:equivalent(report)},null,2));
    if(process.argv.includes('--require-equivalent')&&!equivalent(report))process.exitCode=1;
  } finally {a.conn.close();b.conn.close()}
}
