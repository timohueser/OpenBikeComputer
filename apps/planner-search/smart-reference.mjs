import {spawn} from 'node:child_process';
import {createHash} from 'node:crypto';
import {createReadStream,readFileSync,writeFileSync,mkdtempSync,symlinkSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {setTimeout as delay} from 'node:timers/promises';
import {lengths} from './web/geography.mjs';
import {DEFAULT_VIEW} from './web/engine.mjs';

function canonicalReply(value) {
  if (Array.isArray(value)) return value.map(canonicalReply);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort()
    .filter(key=>!['elapsed','parserMs'].includes(key)).map(key=>[key,canonicalReply(value[key])]));
  return value;
}

const [database,model,python,output] = process.argv.slice(2).map(value=>path.resolve(value));
if (!output) throw new Error('Usage: node smart-reference.mjs DATABASE.sqlite MODEL PYTHON OUTPUT.json');
const temporary = mkdtempSync(path.join(tmpdir(),'planner-reference-'));
symlinkSync(database,path.join(temporary,'baden-wuerttemberg.sqlite'));
symlinkSync(model,path.join(temporary,'model'));
const server = spawn(process.execPath,[new URL('./server.mjs',import.meta.url).pathname],{
  env:{...process.env,OBC_SEARCH_DATA:temporary,OBC_SEARCH_REGIONS:'baden-wuerttemberg',
    OBC_SEARCH_PYTHON:python,OBC_SEARCH_PORT:'0'},stdio:['ignore','pipe','inherit'],
});
try {
  const endpoint = await new Promise((resolve,reject)=>{
    server.on('error',reject);
    server.on('exit',code=>reject(new Error(`Reference server exited: ${code}`)));
    createInterface({input:server.stdout}).on('line',line=>{
      const url = line.match(/http:\/\/127\.0\.0\.1:\d+/)?.[0];
      if (url) resolve(url);
    });
  });
  let ready = false;
  for (let n=0; n<200; n++) {
    if ((await (await fetch(`${endpoint}/api/planner-search/status`)).json()).parser.ready) { ready=true;break; }
    await delay(100);
  }
  if (!ready) throw new Error('Reference parser did not initialize.');
  const xml = readFileSync(new URL('../../fixtures/sources/route-import/komoot-schwarzwald.gpx',import.meta.url),'utf8');
  const coordinates = [...xml.matchAll(/<trkpt lat="([^"]+)" lon="([^"]+)"/g)].map(row=>[Number(row[2]),Number(row[1])]);
  const distances = lengths(coordinates), total = distances.at(-1);
  const plan = {coordinates,points:[],days:[1,2,3].map(number=>({number,from:(number-1)*total/3,to:number*total/3}))};
  const context = {view:DEFAULT_VIEW,here:coordinates[0],plan,startDate:'2026-09-28',now:'2026-09-28T10:58:00Z',submitted:true,limit:20};
  const cases = ['en','de','fr','it'].flatMap(language=>readFileSync(new URL(`./query/testset/${language}.jsonl`,import.meta.url),'utf8')
    .trim().split('\n').map(line=>({kind:'query',input:{q:JSON.parse(line).text}})));
  for (const request of [
    {type:'places',what:['food'],open:{now:true}},
    {type:'places',what:['bakery'],open:{weekday:'mon'}},
    {type:'places',what:['hotel'],open:{day:1},where:{day:1,part:'end'}},
    {type:'places',what:['water'],where:{day:'every'}},
    {type:'route',from:{plan:'start'},to:{name:'Habsburgerstr. 10 Freiburg'}},
    {type:'stretches',what:'gap:water'}, {type:'stretches',what:'steep'},
    {type:'split',days:3}, {type:'reverse'}, {type:'join',day:1},
  ]) cases.push({kind:'query',input:{q:'edited request',request}});
  for (const request of [
    {type:'places',what:['water'],where:{along:{ref:'start',at:{value:2,unit:'h'}}}},
    {type:'stretches',what:'steep'},
  ]) cases.push({kind:'query',input:{q:'typed plan attributes',request,plan:{...plan,
    hours:distances.map(km=>km/15),segments:[{kind:'steep',from:0,to:Math.min(5,total),gradient:12}]}}});
  for (const coordinate of [[7.85434,48.01003],[7.849,47.995],[8.018,48.063],[9.18,48.77]]) cases.push({kind:'reverse',coordinate});
  const samples = [];
  for (const item of cases) {
    const input = item.kind === 'reverse' ? {coordinate:item.coordinate} : {...context,...item.input};
    const started = performance.now();
    const reply = await fetch(`${endpoint}/api/planner-search/${item.kind === 'reverse' ? 'reverse' : 'query'}`,{
      method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(input),
    });
    const result = canonicalReply(await reply.json());
    if (!reply.ok) throw new Error(JSON.stringify({input,result}));
    samples.push({kind:item.kind,elapsedMs:performance.now()-started,result,
      sha256:createHash('sha256').update(JSON.stringify(result)).digest('hex')});
    if (samples.length%50===0) console.log(`Reference replies: ${samples.length}/${cases.length}`);
  }
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(database)) hash.update(chunk);
  const modelHashes = {};
  for (const name of ['model.int8.onnx','tokenizer.json','labels.json','tokenizer_config.json']) {
    const digest = createHash('sha256');
    for await (const chunk of createReadStream(path.join(model,name))) digest.update(chunk);
    modelHashes[name]=digest.digest('hex');
  }
  writeFileSync(output,JSON.stringify({region:'baden-wuerttemberg',context,cases,samples,
    contextScope:'Captured GPX geometry; two typed cases explicitly provide synthetic riding-time and segment attributes',
    databaseSha256:hash.digest('hex'),model:modelHashes}));
  console.log(JSON.stringify({samples:samples.length,output}));
} finally {
  if (server.pid && server.exitCode === null && server.signalCode === null) {
    const closed = once(server,'exit');
    server.kill();
    await closed;
  }
  rmSync(temporary,{recursive:true});
}
