import {test} from 'node:test';
import assert from 'node:assert/strict';
import {database as fixture} from './database.mjs';
function database() {
  const {db,conn}=fixture();
  return {conn,db:{...db,all:(sql,params=[])=>conn.prepare(sql).all(...params)}};
}
import {compareAddresses,equivalent} from '../address-parity.mjs';

test('parity measures missing houses and lookup regressions from the reference population',()=>{
  const candidate=database(),reference=database();
  try {
    assert.equal(compareAddresses(candidate.db,reference.db).fields.percent.all,100);
    assert.ok(equivalent(compareAddresses(candidate.db,reference.db)));
    candidate.conn.exec("DELETE FROM addresses WHERE source='w123'");
    const result=compareAddresses(candidate.db,reference.db);
    assert.equal(result.identity.missing,1);
    assert.ok(!equivalent(result));
    assert.equal(result.identity.recallPercent,75);
    assert.ok(result.forward.housePreservedPercent<100);
    assert.ok(result.reverse.agreementPercent<100);
    assert.equal(result.mismatches[0].reference.source,'w123');
  } finally {candidate.conn.close();reference.conn.close()}
});

test('parity separates identity overlap from renamed streets, postcodes and duplicate rows',()=>{
  const candidate=database(),reference=database();
  try {
    candidate.conn.exec("UPDATE place_records SET name='Wrong street' WHERE id=10; UPDATE place_contexts SET postcode='99999' WHERE id=10; INSERT INTO addresses SELECT * FROM addresses WHERE source='w123'");
    const result=compareAddresses(candidate.db,reference.db);
    assert.equal(result.identity.recallPercent,100);
    assert.equal(result.duplicateRows.candidate,1);
    assert.equal(result.fields.percent.name,75);
    assert.equal(result.fields.percent.postcode,75);
    assert.equal(result.fields.percent.all,75);
  } finally {candidate.conn.close();reference.conn.close()}
});
