import sqlite3InitModule from './vendor/sqlite/index.mjs';
import {search} from './engine.mjs';

let pool,sqlite,conn;
const ready=(async()=>{
  sqlite=await sqlite3InitModule({locateFile:f=>new URL('./vendor/sqlite/'+f,import.meta.url).href});
  pool=await sqlite.installOpfsSAHPoolVfs({name:'search-spike',initialCapacity:4});
  if(pool.getFileNames().includes('/bw.sqlite')) open();
})();
function open() {
  conn=new pool.OpfsSAHPoolDb('/bw.sqlite','r');
  conn.exec('PRAGMA cache_size=-8192; PRAGMA mmap_size=0;');
}
const db={all(sql,bind=[]) {return conn.exec({sql,bind,rowMode:'object',returnValue:'resultRows'});}};
let queue=Promise.resolve();
self.onmessage=({data})=>{
  queue=queue.then(async()=>{
    try {
      await ready;
      let result;
      if(data.action==='install') {
        if(conn){conn.close();conn=null;}
        const response=await fetch('/data/baden-wuerttemberg.sqlite');
        if(!response.ok)throw new Error('Search download failed. Try again while online.');
        const reader=response.body.getReader(), total=Number(response.headers.get('X-Uncompressed-Length')||response.headers.get('Content-Length'));
        let received=0,last=0;
        await pool.importDb('/bw.sqlite',async()=>{
          const {value,done}=await reader.read();
          if(done)return undefined;
          received+=value.length;
          if(performance.now()-last>150){self.postMessage({progress:received,total});last=performance.now();}
          return value;
        });
        open();result={installed:true,bytes:received};
      } else if(data.action==='search') {
        result=conn?{...search(db,data.input),source:'offline'}:{results:[],unavailable:true};
      } else result={installed:!!conn};
      self.postMessage({id:data.id,result});
    } catch(error) {self.postMessage({id:data.id,error:error.message});}
  });
};
