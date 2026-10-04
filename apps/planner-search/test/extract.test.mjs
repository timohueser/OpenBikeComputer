import {test} from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {mkdtempSync,readFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import {database} from './database.mjs';
import {reverseAddress} from '../web/reverse.mjs';

test('bbox extraction retains external street and administrative dependencies without changing its source', () => {
  const dir = mkdtempSync(join(tmpdir(),'obc-search-extract-'));
  try {
    const source = join(dir,'source.sqlite'), destination = join(dir,'selected.sqlite');
    const {conn} = database([['r27','Locality','city',7.9,48.2,'Locality',.5,'Locality alias']],source);
    conn.exec(`UPDATE place_records SET west=7.8,south=48,east=8,north=48.3 WHERE source='r27';
      INSERT INTO metadata VALUES ('schema','4'),('bounds','[7,47,12,50]');`);
    conn.close();
    const before = createHash('sha256').update(readFileSync(source)).digest('hex');
    const command = new URL('../extract.py',import.meta.url).pathname;
    execFileSync('python3',[command,source,destination,'--bounds=7.8535,48.0295,7.8545,48.0305']);
    assert.equal(createHash('sha256').update(readFileSync(source)).digest('hex'),before);
    const output = new DatabaseSync(destination,{readOnly:true});
    try {
      const db = {all:(sql,params=[])=>output.prepare(sql).all(...params)};
      assert.deepEqual(db.all('SELECT source FROM places ORDER BY id').map(p=>p.source),['s14','r27']);
      assert.deepEqual(db.all('SELECT source FROM addresses').map(p=>p.source),['w141']);
      assert.equal(reverseAddress(db,[7.854,48.03]),'Habsburgerstraße 10, Freiburg');
      assert.equal(db.all("SELECT count(*) n FROM names WHERE term='locality alias'")[0].n,1);
      output.prepare('ATTACH DATABASE ? AS original').run(source);
      assert.equal(db.all('SELECT count(*) n FROM (SELECT * FROM places EXCEPT SELECT * FROM original.places)')[0].n,0);
      assert.equal(db.all('SELECT count(*) n FROM (SELECT * FROM addresses EXCEPT SELECT * FROM original.addresses)')[0].n,0);
      assert.equal(JSON.parse(db.all("SELECT value FROM metadata WHERE key='source_search_sha256'")[0].value),before);
      assert.throws(()=>execFileSync('python3',[command,source,join(dir,'invalid.sqlite'),'--bounds=6,47,8,48'],{stdio:'pipe'}),/does not cover/);
    } finally { output.close(); }
    const missingCoverage = new DatabaseSync(source);
    missingCoverage.exec("DELETE FROM metadata WHERE key='bounds'");
    missingCoverage.close();
    assert.throws(()=>execFileSync('python3',[command,source,join(dir,'unknown.sqlite'),'--bounds=7.8,48,7.9,48.1'],{stdio:'pipe'}),/Rebuild it with --bounds/);
  } finally { rmSync(dir,{recursive:true,force:true}); }
});
