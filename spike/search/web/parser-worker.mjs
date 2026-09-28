import * as ort from './vendor/ort/ort.wasm.min.mjs';
import {Tokenizer} from './vendor/tokenizers/tokenizers.min.mjs';
import {kindOf,norm,simpleRequest} from './engine.mjs';

let session,tok,labels,lexicon,loading;
async function load() {
  if(!loading) loading=(async()=>{
    ort.env.wasm.wasmPaths=new URL('./vendor/ort/',import.meta.url).href;
    ort.env.wasm.numThreads=1;
    const [tj,tc,ls,ks]=await Promise.all(['vendor/model/tokenizer.json','vendor/model/tokenizer_config.json','labels.json','kinds.json']
      .map(p=>fetch(p).then(r=>{if(!r.ok)throw new Error('Download the query model while online first.');return r.json();})));
    tok=new Tokenizer(tj,tc);labels=ls;lexicon=new Map();
    for(const lang of Object.values(ks.terms))for(const [kind,terms]of Object.entries(lang))
      for(const term of terms) if(!lexicon.has(norm(term)))lexicon.set(norm(term),kind);
    session=await ort.InferenceSession.create('./vendor/model/model.int8.onnx',{executionProviders:['wasm']});
  })();
  return loading;
}
const argmax=a=>a.reduce((best,x,i)=>x>a[best]?i:best,0);
function decode(q,intent,spans) {
  const get=k=>spans.filter(s=>s.slot===k).map(s=>s.text);
  const category=get('WHAT').map(s=>kindOf(s)||lexicon.get(norm(s))).filter(Boolean);
  if(intent==='place') return {type:'place',name:[get('NAME')[0]||q,...get('NEAR')].join(' '),via:'mmBERT',spans};
  if(intent!=='places') return {...simpleRequest(q),via:'text fallback',spans,
    notice:intent==='none'?'Interpreting this as a place name.':'This spike searches places; it does not change the route.'};
  if(!category.length)return {...simpleRequest(q),via:'text fallback',spans};
  const where={scope:'view'}, text=norm(q), day=get('DAY').join(' '), along=get('ALONG').join(' ');
  const near=get('NEAR')[0];
  if(near) {where.near=near;where.in=/\b(in|dans|a|à)\s/i.test(q);}
  if(get('HERE').length || /\b(near me|around me|bei mir|in meiner nahe)\b/.test(text))where.scope='here';
  if(get('SCOPE').some(s=>/route|strecke|tour|itiner|parcours/i.test(s)))where.scope='route';
  const routeHalf=/first half|erste[nr]? halfte|premiere moitie|last half|zweite[nr]? halfte|second half/.test(text);
  if(day && !routeHalf) {
    const number=day.match(/\d+/)?.[0]||({one:1,two:2,three:3,four:4,eins:1,zwei:2,drei:3,vier:4}[norm(day).split(' ').at(-1)]);
    if(!number)return {unsupported:'Use a numbered day in this prototype.',spans};
    where.day=Number(number);
    if(/end|ende|fin|fine/i.test(day))where.part='end';
    else if(/start|anfang|debut|début|inizio/i.test(day))where.part='start';
  }
  if(/first half|erste[nr]? halfte|premiere moitie/.test(text)){where.part='first_half';where.scope='route';}
  else if(/last half|zweite[nr]? halfte|second half/.test(text)){where.part='last_half';where.scope='route';}
  else if(along)return {unsupported:'This spike supports route halves and day ends; numeric route intervals are not connected yet.',spans};
  if(get('OPEN').length||get('BEFORE').length||get('AFTER').length)
    return {unsupported:'Opening hours and before/after filters are not connected in this search spike.',spans};
  const ignored=get('IGNORED');
  const r=get('RADIUS')[0];
  let radius=r?Number(r.match(/[\d.,]+/)?.[0]?.replace(',','.')):undefined;
  if(r&&/\b(m|meters?|metres?)\b/i.test(r))radius/=1000;
  return {type:'places',what:[...new Set(category)],where,radius,via:'mmBERT',spans,ignored};
}

async function parse(q) {
  const simple=simpleRequest(q);
  if(simple.type==='places')return simple;
  // Structured wording invokes the tagger; names and addresses remain directly searchable.
  if(!/\b(in|near|along|end|day|route|first|last|within|around|am|ende|tag|entlang|nahe|bei|halfte|half|dans|pres|jour|fin|vicino|giorno|meta)\b/i.test(norm(q)))return simple;
  if(q.length>80)return {...simple,notice:'Smart queries support up to 80 characters; using ordinary text search.'};
  await load();
  const start=performance.now(), enc=tok.encode(q), ids=enc.ids;
  if(ids.length>64)return {...simple,notice:'Query is too long for the parser; using text search.'};
  const out=await session.run({input_ids:new ort.Tensor('int64',BigInt64Array.from(ids,BigInt),[1,ids.length]),
    attention_mask:new ort.Tensor('int64',new BigInt64Array(ids.length).fill(1n),[1,ids.length])});
  const intent=labels.INTENTS[argmax(Array.from(out.intent_logits.data))];
  // Metaspace tokens retain the input spelling. Fail visibly if byte fallback prevents alignment.
  let offset=0,first=true,tokenSpans=[];
  for(let i=0;i<enc.tokens.length;i++) {
    let t=enc.tokens[i];
    if(t==='<bos>'||t==='<eos>')continue;
    t=t.replaceAll('▁',' ');
    if(first&&t.startsWith(' ')){t=t.slice(1);first=false;}else first=false;
    tokenSpans.push({start:offset,end:offset+t.length,index:i,text:t});offset+=t.length;
  }
  if(tokenSpans.map(t=>t.text).join('')!==q)return {...simple,notice:'Parser could not align this spelling; using text search.'};
  const words=[...q.matchAll(/\d+(?:[.,]\d+)?|[\p{L}]+(?:[-'’][\p{L}]+)*|\S/gu)];
  let spans=[],prev='O';
  for(const w of words) {
    const token=tokenSpans.find(t=>Math.max(t.start,w.index)<Math.min(t.end,w.index+w[0].length));
    const index=token?.index, width=labels.LABELS.length;
    const lab=index===undefined?'O':labels.LABELS[argmax(Array.from(out.tag_logits.data.slice(index*width,(index+1)*width)))];
    if(lab!=='O') {
      const slot=lab.slice(2),end=w.index+w[0].length;
      if(lab.startsWith('I-')&&prev.slice(2)===slot) {
        spans.at(-1).end=end;spans.at(-1).text=q.slice(spans.at(-1).start,end);
      } else spans.push({slot,start:w.index,end,text:w[0]});
    }
    prev=lab;
  }
  return {...decode(q,intent,spans),modelMs:performance.now()-start,intent};
}
let queue=Promise.resolve();
self.onmessage=({data})=>{queue=queue.then(async()=>{
  try{self.postMessage({id:data.id,result:data.action==='load'?(await load(),{loaded:true}):await parse(data.q)});}
  catch(e){self.postMessage({id:data.id,result:{...simpleRequest(data.q||''),notice:e.message}});}
});};
