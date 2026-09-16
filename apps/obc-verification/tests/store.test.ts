import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Store } from '../src/lib/server/store.ts';

test('revisions reject stale saves and never alter historical content', () => {
  const directory = mkdtempSync(join(tmpdir(), 'obc-verification-'));
  const db = new Store(directory);
  try {
    const first = db.latestRevision(); assert.equal(first.requirements.length, 1); assert.equal(first.requirements[0].active, false);
    const changed = structuredClone(first.requirements); changed[0].statement = 'Human written requirement.'; changed[0].group = 'Navigation';
    const next = db.saveRevision(first.id, 'owner', changed);
    assert.equal(next.id, first.id + 1);
    const regrouped = structuredClone(next.requirements); regrouped[0].group = 'Bluetooth';
    db.saveRevision(next.id, 'owner', regrouped);
    assert.equal(db.revision(first.id).requirements[0].group, undefined);
    assert.equal(db.revision(next.id).requirements[0].group, 'Navigation');
    assert.equal(next.requirements[0].group, 'Navigation');
    assert.throws(() => db.saveRevision(first.id, 'other owner', changed), /changed/);
    assert.notEqual(db.revision(first.id).requirements[0].statement, changed[0].statement);
    db.put('example', 'id', { value: 1 }); db.put('example', 'id', { value: 2 });
    assert.deepEqual(db.get('example', 'id'), { value: 2 });
    assert.equal(db.db.prepare('SELECT COUNT(*) AS count FROM records').get()?.count, 2);
  } finally { db.db.close(); rmSync(directory, { recursive: true, force: true }); }
});
