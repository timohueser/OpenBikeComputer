import {test} from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import {spawn} from 'node:child_process';
import {mkdtempSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {fileURLToPath} from 'node:url';
import {setTimeout as delay} from 'node:timers/promises';

test('disconnected clients retain admission slots until routing finishes',{timeout:10000},async t=>{
  const data=mkdtempSync(tmpdir()+'/planner-admission-');
  // The server exits without a running query runtime.
  const worker=data+'/worker';
  writeFileSync(worker,`#!${process.execPath}\nconsole.log(JSON.stringify({ready:true})); process.stdin.resume();\n`,{mode:0o755});
  const waiting=[];
  let allAdmitted;
  const admitted=new Promise(resolve=>{allAdmitted=resolve;});
  const router=http.createServer((req,res)=>{
    req.resume();
    req.on('end',()=>{
      waiting.push(res);
      if(waiting.length===16)allAdmitted();
    });
  });
  await new Promise(resolve=>router.listen(0,'127.0.0.1',resolve));
  const server=spawn(process.execPath,[fileURLToPath(new URL('../server.mjs',import.meta.url))],{
    env:{...process.env,OBC_SEARCH_PORT:'0',OBC_SEARCH_DATA:data,OBC_SEARCH_REGIONS:'test',
      OBC_SEARCH_PYTHON:worker,OBC_QUERY_ROUTER:`http://127.0.0.1:${router.address().port}`},
    stdio:['ignore','pipe','ignore'],
  });
  t.after(()=>{
    server.kill('SIGTERM');
    router.closeAllConnections();
    router.close();
    rmSync(data,{recursive:true,force:true});
  });
  const base=await new Promise((resolve,reject)=>{
    let output='';
    server.on('error',reject);
    server.stdout.on('data',chunk=>{
      output+=chunk.toString();
      const url=output.match(/http:\/\/127\.0\.0\.1:\d+\n/);
      if(url)resolve(url[0].trim());
    });
  });
  const clients=Array.from({length:16},()=>{
    const request=http.request(base+'/api/planner-search/route',{method:'POST',headers:{'Content-Type':'application/json'}});
    request.on('error',()=>{});
    request.end(JSON.stringify({bike:'touring',goal:'balanced',points:[[7.85,48],[7.86,48]]}));
    return request;
  });
  t.after(()=>clients.forEach(client=>client.destroy()));
  await admitted;
  clients.forEach(client=>client.destroy());
  await delay(50);
  assert.equal((await fetch(base+'/api/planner-search/status')).status,503);
  waiting.forEach(res=>{res.writeHead(503,{'Content-Type':'application/json'});res.end('{"message":"Finished"}');});
  let status;
  do{
    await delay(10);
    status=(await fetch(base+'/api/planner-search/status')).status;
  }while(status===503);
  assert.equal(status,200);
});
