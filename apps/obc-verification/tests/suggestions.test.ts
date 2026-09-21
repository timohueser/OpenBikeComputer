import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RequestEvent } from '@sveltejs/kit';
import type { Actor, Requirement } from '../src/lib/types.ts';
import { api } from '../src/lib/server/api.ts';
import { store } from '../src/lib/server/store.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-verification-suggestions-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.ORIGIN = 'https://verify.example.com';
function request(path: string, method = 'GET', data?: unknown, actor?: Actor) {
  return api({ params: { path }, url: new URL(`${process.env.ORIGIN}/api/${path}`), request: new Request(`${process.env.ORIGIN}/api/${path}`, { method, headers: { 'content-type': 'application/json', origin: process.env.ORIGIN! }, body: data === undefined ? undefined : JSON.stringify(data) }), locals: { actor }, cookies: { get: () => undefined }, getClientAddress: () => '127.0.0.1' } as unknown as RequestEvent);
}
test('requirement suggestions are recorded, superseded, decided once, and never change a requirement', async () => {
  const owner: Actor = { name: 'owner', role: 'owner' };
  const agent: Actor = { name: 'agent', role: 'agent' };
  try {
    const base = store().latestRevision();
    const subject = { ...base.requirements[0], id: 'EXAMPLE-002', title: 'Return to position', statement: 'The map shall return to the position.' };
    store().saveRevision(base.id, 'owner', [...base.requirements, subject]);
    const suggest = (data: Record<string, unknown>, actor: Actor = agent) =>
      request('requirement-suggestions', 'POST', { baseRevision: store().latestRevision().id, reason: 'Nothing states this obligation.', ...data }, actor);
    const listed = async () => await (await request('requirement-suggestions', 'GET', undefined, agent)).json();
    const fresh = { title: 'Export timestamps', statement: 'Exported files shall keep the recorded time of every sample.', group: 'Ride export' };
    const change = { requirementId: 'EXAMPLE-002', title: 'Say what happens without a fix', statement: 'The map shall return to the last known position.' };
    assert.equal((await suggest({ ...fresh, baseRevision: base.id })).status, 409);
    assert.equal((await suggest({ ...change, requirementId: 'EXAMPLE-404' })).status, 404);
    assert.equal((await suggest({ ...fresh, reason: '' })).status, 400);
    const created = await (await suggest(fresh)).json();
    assert.equal(created.status, 'open');
    assert.equal(created.author, 'agent');
    assert.equal((await (await suggest(fresh)).json()).id, created.id, 'an identical open suggestion is reused');
    const first = await (await suggest(change)).json();
    const second = await (await suggest({ ...change, statement: 'The map shall return to the last known position, and show that the fix is missing.' })).json();
    assert.equal(second.supersedes, first.id);
    assert.deepEqual((await listed()).map((s: any) => s.status).sort(), ['open', 'open', 'superseded']);

    // A decision is an acknowledgment. It records who decided, and it leaves the requirements exactly as they were.
    const before = JSON.stringify(store().latestRevision());
    assert.equal((await request(`requirement-suggestions/${created.id}`, 'POST', { accept: true }, agent)).status, 403);
    const accepted = await (await request(`requirement-suggestions/${created.id}`, 'POST', { accept: true }, owner)).json();
    assert.equal(accepted.status, 'accepted');
    assert.equal(accepted.decidedBy, 'owner');
    assert.equal(JSON.stringify(store().latestRevision()), before, 'accepting writes no requirement and no revision');
    assert.equal((await request(`requirement-suggestions/${created.id}`, 'POST', { accept: false }, owner)).status, 409);

    // An edit to the statement leaves the open change suggestion behind; the owner reads it before deciding.
    const edited = store().latestRevision();
    const amended = (statement: string) => (r: Requirement): Requirement => r.id === 'EXAMPLE-002' ? { ...r, statement } : r;
    store().saveRevision(edited.id, 'owner', edited.requirements.map(amended(`${subject.statement} It shall also keep the zoom level.`)));
    const stale = (await listed()).find((s: any) => s.id === second.id);
    assert.match(stale.stale, /EXAMPLE-002 changed after this suggestion, which read r\d+\./);
    assert.equal(stale.missing, undefined);
    // A statement the owner put back is the statement the agent read, so the warning goes.
    const restored = store().latestRevision();
    store().saveRevision(restored.id, 'owner', restored.requirements.map(amended(subject.statement)));
    assert.equal((await listed()).find((s: any) => s.id === second.id).stale, undefined);

    const removed = store().latestRevision();
    store().saveRevision(removed.id, 'owner', removed.requirements.filter(r => r.id !== 'EXAMPLE-002'));
    assert.equal((await listed()).find((s: any) => s.id === second.id).missing, true);
    const dismissedFrom = JSON.stringify(store().latestRevision());
    const dismissed = await (await request(`requirement-suggestions/${second.id}`, 'POST', { accept: false, feedback: 'That belongs to SYS-044.' }, owner)).json();
    assert.equal(dismissed.status, 'dismissed');
    assert.equal(dismissed.feedback, 'That belongs to SYS-044.');
    assert.equal(JSON.stringify(store().latestRevision()), dismissedFrom, 'dismissing writes no requirement and no revision');

    // The newest suggestion stays first, although the decisions wrote the newest records.
    assert.deepEqual((await listed()).map((s: any) => s.id), [second.id, first.id, created.id]);
    store().clearHistory(store().latestRevision().id, 'owner', false);
    assert.deepEqual(await listed(), []);
  } finally { store().db.close(); rmSync(directory, { recursive: true, force: true }); }
});
