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
    const changed = structuredClone(first.requirements); changed[0].statement = 'Human written requirement.'; changed[0].group = 'Navigation'; changed[0].todo = true;
    const next = db.saveRevision(first.id, 'owner', changed);
    assert.equal(next.id, first.id + 1);
    const regrouped = structuredClone(next.requirements); regrouped[0].group = 'Bluetooth'; delete regrouped[0].todo;
    db.saveRevision(next.id, 'owner', regrouped);
    assert.equal(db.revision(first.id).requirements[0].group, undefined);
    assert.equal(db.revision(next.id).requirements[0].group, 'Navigation');
    assert.equal(next.requirements[0].group, 'Navigation');
    assert.equal(db.revision(next.id).requirements[0].todo, true);
    assert.equal(db.latestRevision().requirements[0].todo, undefined);
    assert.throws(() => db.saveRevision(first.id, 'other owner', changed), /changed/);
    assert.notEqual(db.revision(first.id).requirements[0].statement, changed[0].statement);
    db.put('example', 'id', { value: 1 }); db.put('example', 'id', { value: 2 });
    assert.deepEqual(db.get('example', 'id'), { value: 2 });
    assert.equal(db.db.prepare('SELECT COUNT(*) AS count FROM records').get()?.count, 2);
  } finally { db.db.close(); rmSync(directory, { recursive: true, force: true }); }
});

test('history cleanup keeps release references, rejects stale writers, and leaves an empty reset empty after restart', () => {
  const directory = mkdtempSync(join(tmpdir(), 'obc-verification-reset-'));
  const db = new Store(directory);
  try {
    const first = db.latestRevision();
    const second = db.saveRevision(first.id, 'owner', [{ ...first.requirements[0], group: 'Navigation' }]);
    const latest = db.saveRevision(second.id, 'owner', [{ ...second.requirements[0], title: 'Reviewed requirement' }]);
    const candidate = { revision: first, status: 'failed' };
    const publication = { revision: second, status: 'published' };
    db.put('candidate', 'candidate', candidate); db.put('publication', 'release', publication);
    db.put('proposal', 'proposal', { baseRevision: latest.id });
    db.put('file', 'file', { name: 'input.gpx' }); db.put('catalog', 'current', { cases: [] });
    db.db.prepare('INSERT INTO github_users(id,login,admin) VALUES(?,?,?)').run('123', 'admin', 1);
    assert.deepEqual(db.historySummary(), { baseRevision: latest.id, revisionCount: 3, protectedRevisionCount: 2, requirementCount: 1, testCount: 0 });
    const kept = db.clearHistory(latest.id, 'admin', false);
    assert.deepEqual(kept.requirements, latest.requirements);
    assert.deepEqual(db.revisions().map(r => r.id), [kept.id, second.id, first.id]);
    assert.throws(() => db.revision(latest.id), /not found/);
    assert.equal(db.proposals().length, 0);
    assert.throws(() => db.clearHistory(latest.id, 'admin', true), /changed/);
    assert.throws(() => db.saveRevision(latest.id, 'stale editor', latest.requirements), /changed/);
    const empty = db.clearHistory(kept.id, 'admin', true);
    assert(empty.id > kept.id); assert.deepEqual(empty.requirements, []);
    assert.deepEqual(db.get('candidate', 'candidate'), candidate);
    assert.deepEqual(db.get('publication', 'release'), publication);
    assert.deepEqual(db.get('file', 'file'), { name: 'input.gpx' });
    assert.deepEqual(db.get('catalog', 'current'), { cases: [] });
    assert.equal(db.githubUser('123')?.admin, true);
    const reopened = new Store(directory);
    assert.deepEqual(reopened.latestRevision(), empty); reopened.db.close();
  } finally { db.db.close(); rmSync(directory, { recursive: true, force: true }); }
});
