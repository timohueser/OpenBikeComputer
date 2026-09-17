import { after, test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RequestEvent } from '@sveltejs/kit';
import type { Actor, LinkProposal, ProposalReview, Requirement } from '../src/lib/types.ts';
import { api } from '../src/lib/server/api.ts';
import { store } from '../src/lib/server/store.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-proposals-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.ORIGIN = 'https://verify.example.com';
const owner: Actor = { name: 'owner', role: 'owner' };
const agent: Actor = { name: 'agent', role: 'agent' };
const linked = { id: 'existing', kind: 'automated' as const, title: 'Existing test', caseId: 'old', inputs: [] };
after(() => { store().db.close(); rmSync(directory, { recursive: true, force: true }); });

function request(path: string, method = 'GET', data?: unknown, actor = agent) {
  return api({ params: { path }, request: new Request(`${process.env.ORIGIN}/api/${path}`, {
    method, headers: { 'content-type': 'application/json', origin: process.env.ORIGIN! },
    body: data === undefined ? undefined : JSON.stringify(data)
  }), locals: { actor } } as unknown as RequestEvent);
}
function setup() {
  store().put('catalog', 'current', { sourceSha: 'a'.repeat(40), updatedAt: '', cases: ['a', 'b', 'old'].map(id => ({ id, suite: 'suite', name: `Test ${id}` })) });
  const requirement: Requirement = { id: 'SYS-001', title: 'A requirement', statement: 'A measurable promise.', active: true, tests: [linked] };
  return store().saveRevision(store().latestRevision().id, owner.name, [requirement, { ...requirement, id: 'SYS-002', tests: [] }]);
}
async function propose(baseRevision: number, caseId: string, action = 'add', requirementId = 'SYS-001') {
  const response = await request('proposals', 'POST', { baseRevision, requirementId, caseId, action, reason: 'Checks the required behavior.' });
  assert.equal(response.status, 201, await response.clone().text());
  return await response.json() as LinkProposal;
}
async function review(id: string) {
  const response = await request('proposals');
  assert.equal(response.status, 200);
  return (await response.json() as ProposalReview[]).find(p => p.id === id)!;
}
const decide = (id: string, accept = true) => request(`proposals/${id}`, 'POST', { accept }, owner);

test('a proposal batch survives other link approvals and unrelated edits without changing its base revision', async () => {
  const base = setup();
  const proposals = [await propose(base.id, 'a'), await propose(base.id, 'b'), await propose(base.id, 'old', 'remove'), await propose(base.id, 'a', 'add', 'SYS-002')];
  const edited = structuredClone(base.requirements);
  edited.push({ ...edited[0], id: 'SYS-003', statement: 'An unrelated edit.' });
  edited[0].tests.push({ id: 'manual', kind: 'manual', title: 'Manual check', steps: 'Check.', expected: 'Works.', inputs: [] });
  store().saveRevision(base.id, owner.name, edited);
  for (const proposal of proposals) {
    const item = await review(proposal.id);
    assert.equal(item.conflict, undefined);
    assert.equal(item.requirement?.title, 'A requirement');
    assert.equal(item.test?.name, `Test ${proposal.caseId}`);
    const response = await decide(proposal.id);
    assert.equal(response.status, 200, await response.clone().text());
    const result = await response.json();
    assert.equal(result.status, 'accepted');
    assert.equal(result.baseRevision, base.id);
    assert.deepEqual(result.revision, store().latestRevision());
  }
  const current = store().latestRevision();
  assert.deepEqual(current.requirements[0].tests.map(t => t.caseId ?? t.id), ['manual', 'a', 'b']);
  assert.equal(current.requirements[1].tests[0].caseId, 'a');
  assert.equal(current.requirements[2].statement, 'An unrelated edit.');
  assert.deepEqual(store().revision(base.id), base);
  assert.equal((await decide(proposals[0].id)).status, 409);
  assert.deepEqual(store().latestRevision(), current);
});

test('conflicting definitions, links, and catalogue changes block approval but allow rejection', async () => {
  const scenarios: { name: string; action?: string; change: (requirements: Requirement[]) => void }[] = [
    { name: 'statement', change: r => { r[0].statement = 'A different promise.'; } },
    { name: 'title', change: r => { r[0].title = 'A different title'; } },
    { name: 'group', change: r => { r[0].group = 'Navigation'; } },
    { name: 'active', change: r => { r[0].active = false; } },
    { name: 'definition label', change: r => { r[0].todo = true; } },
    { name: 'implementation label', change: r => { r[0].implementationNeeded = true; } },
    { name: 'deleted requirement', change: r => { r.splice(0, 1); } },
    { name: 'already linked', change: r => { r[0].tests.push({ ...linked, id: 'added', caseId: 'a' }); } },
    { name: 'missing catalogue test', change: () => { store().put('catalog', 'current', { sourceSha: '', updatedAt: '', cases: [] }); } },
    { name: 'removed link', action: 'remove', change: r => { r[0].tests = []; } },
    { name: 'replaced link', action: 'remove', change: r => { r[0].tests[0].id = 'replacement'; } }
  ];
  for (const scenario of scenarios) {
    const base = setup();
    const proposal = await propose(base.id, scenario.action === 'remove' ? 'old' : 'a', scenario.action);
    const edited = structuredClone(base.requirements);
    scenario.change(edited);
    const current = store().saveRevision(base.id, owner.name, edited);
    const item = await review(proposal.id);
    assert(item.conflict, scenario.name);
    const response = await decide(proposal.id);
    assert.equal(response.status, 409, scenario.name);
    assert.equal((await response.json()).error, item.conflict);
    assert.deepEqual(store().latestRevision(), current);
    assert.equal(store().get<LinkProposal>('proposal', proposal.id).status, 'pending');
    assert.equal((await decide(proposal.id, false)).status, 200);
    assert.deepEqual(store().latestRevision(), current);
    assert.equal((await review(proposal.id)).status, 'rejected');
  }
});

test('submission retries reuse a pending proposal and invalid link requests are rejected', async () => {
  const base = setup();
  const proposal = await propose(base.id, 'a');
  const payload = { baseRevision: base.id, requirementId: 'SYS-001', caseId: 'a', action: 'add', reason: 'Checks the required behavior.' };
  const retry = await request('proposals', 'POST', payload);
  assert.equal(retry.status, 200);
  assert.equal((await retry.json()).id, proposal.id);
  assert.equal((await request('proposals', 'POST', { ...payload, caseId: 'old' })).status, 409);
  assert.equal((await request('proposals', 'POST', { ...payload, action: 'remove' })).status, 409);
  assert.equal((await request('proposals', 'POST', { ...payload, caseId: 'unknown' })).status, 400);
  assert.equal((await decide(proposal.id)).status, 200);
  assert.equal((await request('proposals', 'POST', payload)).status, 409);
});

test('saving an approved link and its decision is atomic', async () => {
  const base = setup();
  const proposal = await propose(base.id, 'a');
  store().db.exec(`CREATE TEMP TRIGGER refuse_decision BEFORE INSERT ON records
    WHEN NEW.kind = 'proposal' AND json_extract(NEW.body, '$.status') = 'accepted'
    BEGIN SELECT RAISE(ABORT, 'Cannot save decision'); END`);
  try {
    assert.throws(() => store().decideProposal(proposal.id, owner.name, true), /Cannot save decision/);
    assert.deepEqual(store().latestRevision(), base);
    assert.equal(store().get<LinkProposal>('proposal', proposal.id).status, 'pending');
  } finally { store().db.exec('DROP TRIGGER refuse_decision'); }
});
