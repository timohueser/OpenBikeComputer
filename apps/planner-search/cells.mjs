import {DatabaseSync} from 'node:sqlite';

const qualified = (sql, i) => sql.replace(/\b(FROM|JOIN)\s+(place_records|places|names|compact_names|terms|spatial|addresses)\b/gi, `$1 c${i}.$2`);
const overlaps = (a,b) => !a || !b || a[0]<=b[2] && a[2]>=b[0] && a[1]<=b[3] && a[3]>=b[1];
const narrow = sql => sql.replace('p.*','p.id,p.kind,p.importance,p.lon,p.lat,p.source')
  .replace('JOIN places p','JOIN place_records p').replace('FROM places p','FROM place_records p');

function connection(database) {
  const statements = new Map();
  return {database, all(sql,params=[]) {
    if (!statements.has(sql)) {
      if (statements.size>=128) statements.delete(statements.keys().next().value);
      statements.set(sql,database.prepare(sql));
    }
    return statements.get(sql).all(...params);
  }};
}

/** Immutable cells retain their own indexes. Bounded SQL unions preserve global candidate limits. */
export function openCells(files,metadata) {
  if (!files.length) throw new Error('No search cells are installed.');
  const merge=connection(new DatabaseSync(':memory:')), groups=[],plans=new Map();
  let closed=false;
  const close=()=>{if(closed)return;closed=true;for(const g of groups)g.database.close();merge.database.close()};
  try {
    merge.database.exec(`PRAGMA cache_size=-16384; PRAGMA temp_store=MEMORY;
      CREATE TABLE lexicon(term TEXT UNIQUE);
      CREATE VIRTUAL TABLE fuzzy USING fts5(term,content='lexicon',detail=none,tokenize='trigram');`);
    const insert=merge.database.prepare('INSERT OR IGNORE INTO lexicon(term) VALUES(?)');
    merge.database.exec('BEGIN');
    for(let start=0;start<files.length;start+=8) {
      const group=connection(new DatabaseSync(':memory:'));group.indices=[];groups.push(group);
      group.database.exec('PRAGMA cache_size=-256; PRAGMA temp_store=MEMORY');
      for(let i=start;i<Math.min(files.length,start+8);i++) {
        group.database.prepare(`ATTACH DATABASE ? AS c${i}`).run(files[i].file);
        group.database.exec(`PRAGMA c${i}.cache_size=-${Math.max(128,Math.floor(16384/files.length))}; PRAGMA c${i}.mmap_size=0`);
        group.indices.push(i);
        for(const {term} of group.database.prepare(`SELECT term FROM c${i}.lexicon`).iterate())insert.run(term);
      }
      group.database.exec('PRAGMA query_only=ON');
    }
    merge.database.exec(`COMMIT; INSERT INTO fuzzy(fuzzy) VALUES('rebuild'); INSERT INTO fuzzy(fuzzy) VALUES('optimize');`);
  } catch(error) {close();throw error}

  function plan(sql) {
    if(plans.has(sql))return plans.get(sql);
    const order=sql.match(/ORDER BY([\s\S]+?)(?:LIMIT \d+)?$/i)?.[1].trim();
    const limit=Number(sql.match(/LIMIT (\d+)\s*$/i)?.[1]);
    const result={order,limit,branches:files.map((_,i)=>qualified(sql,i))};
    if(plans.size>=128)plans.delete(plans.keys().next().value);plans.set(sql,result);return result;
  }
  function statement(sql,params,options,group) {
    const p=plan(sql),indices=group.indices.filter(i=>overlaps(files[i].bounds,options.bounds));
    if(!indices.length)return null;
    const parts=indices.map(i=>`SELECT * FROM (${p.branches[i]})`),bindings=indices.flatMap(()=>params);
    if(parts.length===1)return[parts[0],bindings];
    let merged=`SELECT DISTINCT p.* FROM (${parts.join(' UNION ALL ')}) p`;
    if(p.order) {merged+=' ORDER BY '+p.order;const n=(p.order.match(/\?/g)||[]).length;if(n)bindings.push(...params.slice(-n))}
    if(p.limit)merged+=' LIMIT '+p.limit;
    return[merged,bindings];
  }
  function combine(rows,order,limit,params=[]) {
    const first=rows.find(r=>r.length)?.[0];if(!first)return[];
    const fields=Object.keys(first).map(key=>`json_extract(value,'$."${key.replaceAll('"','\\"').replaceAll("'","''")}"') AS "${key.replaceAll('"','""')}"`).join(',');
    const parts=rows.map(()=>`SELECT ${fields} FROM json_each(?)`),bindings=rows.map(r=>JSON.stringify(r));
    let sql=`SELECT DISTINCT p.* FROM (${parts.join(' UNION ALL ')}) p`;
    if(order){sql+=' ORDER BY '+order;const n=(order.match(/\?/g)||[]).length;if(n)bindings.push(...params.slice(-n))}
    if(limit)sql+=' LIMIT '+limit;
    return merge.all(sql,bindings);
  }
  function places(ids) {
    if(!ids.length)return[];
    const encoded=JSON.stringify(ids);
    const rows=groups.flatMap(group=>group.all(group.indices.map(i=>`SELECT p.* FROM c${i}.places p WHERE p.id IN (SELECT value FROM json_each(?))`).join(' UNION '),group.indices.map(()=>encoded)));
    return [...new Map(rows.map(row=>[row.id,row])).values()];
  }
  function candidateIds(queries) {
    const narrowed=queries.map(q=>({...q,sql:narrow(q.sql).replace('ORDER BY rank, ','ORDER BY ')}));
    const order=plan(narrowed[0].sql).order;
    if(!order||order.includes('?')||narrowed.some(q=>plan(q.sql).order!==order||!plan(q.sql).limit)) {
      return [...new Set(narrowed.flatMap(q=>db.all(q.sql,q.params,q.options).map(r=>r.id)))];
    }
    const rows=groups.map(group=>{
      const parts=[],params=[];
      for(const [branch,q] of narrowed.entries()) {
        const selected=statement(q.sql,q.params,q.options||{},group);
        if(selected){parts.push(`SELECT ${branch} AS branch,${plan(q.sql).limit} AS cap,p.* FROM (${selected[0]}) p`);params.push(...selected[1])}
      }
      return parts.length?group.all(parts.join(' UNION ALL '),params):[];
    });
    const fields=['branch','cap','id','kind','importance','lon','lat','source'].map(k=>`json_extract(value,'$.${k}') AS ${k}`).join(',');
    const parts=rows.map(()=>`SELECT ${fields} FROM json_each(?)`);
    const sql=`WITH candidates AS MATERIALIZED (SELECT DISTINCT * FROM (${parts.join(' UNION ALL ')}))
      SELECT DISTINCT id FROM (SELECT id,cap,ROW_NUMBER() OVER (PARTITION BY branch ORDER BY ${order}) AS position FROM candidates p) WHERE position<=cap`;
    return merge.all(sql,rows.map(r=>JSON.stringify(r))).map(row=>row.id);
  }
  const db={close,all(sql,params=[],options={}) {
    if(sql.includes('FROM metadata'))return Object.entries(metadata).map(([key,value])=>({key,value:JSON.stringify(value)}));
    if(sql.includes('FROM fuzzy'))return merge.all(sql,params);
    if(groups.length>1&&sql.includes('SELECT p.* FROM spatial')) {
      const ids=db.all(narrow(sql),params,options).map(row=>row.id);
      const records=new Map(places(ids).map(row=>[row.id,row]));
      return ids.map(id=>records.get(id));
    }
    if(sql.includes('FROM terms '))sql=sql.replace('ORDER BY rank, ','ORDER BY ');
    const selected=groups.map(g=>{const q=statement(sql,params,options,g);return q&&[g,...q]}).filter(Boolean);
    if(!selected.length)return[];
    const rows=selected.map(([g,q,p])=>g.all(q,p));
    if(rows.length===1)return rows[0];
    const {order,limit}=plan(sql);
    if(!order&&!limit&&sql.includes('p.*'))return [...new Map(rows.flatMap(group=>group.map(row=>[row.id,row]))).values()];
    return combine(rows,order,limit,params);
  },candidates(queries) {
    if(groups.length===1) {
      const group=groups[0],parts=[],params=[];
      for(const q of queries){const sql=narrow(q.sql).replace('ORDER BY rank, ','ORDER BY ');const selected=statement(sql,q.params,q.options||{},group);if(selected){parts.push(`SELECT id FROM (${selected[0]})`);params.push(...selected[1])}}
      if(!parts.length)return[];
      const wanted=`WITH wanted AS MATERIALIZED (${parts.join(' UNION ')}) `;
      return group.all(wanted+group.indices.map(i=>`SELECT p.* FROM c${i}.places p WHERE p.id IN (SELECT id FROM wanted)`).join(' UNION '),params);
    }
    const ids=candidateIds(queries);
    return places(ids);
  }};
  return db;
}
