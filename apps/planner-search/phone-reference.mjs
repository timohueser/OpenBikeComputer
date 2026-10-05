import {DatabaseSync} from 'node:sqlite';
import {createHash} from 'node:crypto';
import {writeFileSync,createReadStream} from 'node:fs';
import {pathToFileURL} from 'node:url';
import {run} from './phone-benchmark.mjs';
import {run as runHours} from './phone-hours.mjs';

const [file,output,mode] = process.argv.slice(2);
if (!file || !output) throw new Error('Usage: node phone-reference.mjs PACKAGE.sqlite OUTPUT.json');
const db = new DatabaseSync(':memory:');
db.prepare('ATTACH DATABASE ? AS c0').run(`${pathToFileURL(file).href}?mode=ro`);
db.exec('PRAGMA c0.cache_size=-32768; PRAGMA c0.mmap_size=0');
const statements = new Map();
// The same JSON exchange as the phone's native cell adapter.
const native = {groups: () => JSON.stringify([['c0']]), run: (_group, sql, params) => {
  if (!statements.has(sql)) {
    if (statements.size>=100) statements.clear();
    statements.set(sql,db.prepare(sql));
  }
  return JSON.stringify({rows:statements.get(sql).all(...JSON.parse(params))});
}};
try {
  const report = (mode==='--hours' ? runHours : run)(native,text=>createHash('sha256').update(text).digest('hex'));
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  report.databaseSha256 = hash.digest('hex');
  writeFileSync(output,JSON.stringify(report));
  console.log(JSON.stringify({samples:report.samples.length,output}));
} finally { db.close(); }
