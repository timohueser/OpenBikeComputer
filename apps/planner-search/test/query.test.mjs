import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {database} from './database.mjs';
import {answerQuery} from '../query.mjs';
import {validateRequest} from '../validation.mjs';
const {db}=database();
const input={q:'Kandel',view:[7.8,47.9,7.95,48.05],submitted:true};
const never={parse(){throw new Error('Model must not run');}};

test('exact names and typed chip edits bypass the model, sentences invoke it',async()=>{
  const exact=await answerQuery(db,input,never);
  assert.equal(exact.notice,'');assert.equal(exact.request.type,'place');
  const edited=await answerQuery(db,{...input,q:'a different typed sentence',request:{type:'place',name:'Kandel'}},never);
  assert.equal(edited.notice,'');assert.equal(edited.results[0].name,'Kandel');
  let text;
  const parsed=await answerQuery(db,{...input,q:'reverse the route'}, {async parse(q){text=q;return {request:{type:'reverse'},elapsed:12};}});
  assert.equal(text,'reverse the route');assert.equal(parsed.request.type,'reverse');
  assert.equal(parsed.type,'unresolved');assert.match(parsed.note,/route first/);
});
test('out-of-domain and unavailable-model results stay visible and cannot edit the plan',async()=>{
  const out=await answerQuery(db,{...input,q:'write me a poem'}, {async parse(){return {request:{type:'none',ignored:['poem']},elapsed:1};}});
  assert.equal(out.type,'places');assert.match(out.notice,/not understood/);assert.deepEqual(out.request.ignored,['poem']);
  const offline=await answerQuery(db,{...input,q:'show water after day two'}, {async parse(){throw new Error('Runtime stopped.');}});
  assert.match(offline.notice,/Runtime stopped/);assert.equal(offline.canRetry,true);assert.equal(offline.type,'places');
});
test('the API accepts every archived decoder request in all four evaluation languages',()=>{
  for(const language of ['en','de','fr','it']) {
    const rows=readFileSync(new URL(`../query/testset/${language}.jsonl`,import.meta.url),'utf8').trim().split('\n').map(JSON.parse);
    for(const row of rows)validateRequest(row.request);
  }
});

test('plain categories inherit pointing and cuisine filters stay visible and removable',async()=>{
  const category=await answerQuery(db,{...input,q:'hotel',pointing:{anchor:[11.57,48.13]}},never);
  assert.equal(category.results[0].city,'München');
  const pizza=await answerQuery(db,{...input,q:'pizza'},never);
  assert.equal(pizza.request.cuisine,'pizza');assert.ok(pizza.results.some(p=>p.name==='La Luna'));
  const request={...pizza.request};delete request.cuisine;
  const anyFood=await answerQuery(db,{...input,q:'pizza',request},never);
  assert.ok(anyFood.results.some(p=>p.name==='Asia Wok'));assert.equal(anyFood.request.cuisine,undefined);
});
