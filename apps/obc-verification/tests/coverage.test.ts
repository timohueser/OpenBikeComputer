import { after, test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RequestEvent } from '@sveltejs/kit';
import type { Actor, Candidate, CoveragePlan, CoverageProposal, CoverageProposalReview } from '../src/lib/types.ts';
import { api } from '../src/lib/server/api.ts';
import { Store, store } from '../src/lib/server/store.ts';
import { readiness } from '../src/lib/server/domain.ts';
import { coverageSummary, coverageChanges } from '../src/lib/coverage.ts';
import { report } from '../src/lib/server/report.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-coverage-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.ORIGIN = 'https://verify.example.com';
const owner: Actor = { name: 'owner', role: 'owner' };
const agent: Actor = { name: 'agent', role: 'agent', agentToken: { id: 'token', name: 'Coverage agent', issuedBy: { name: 'owner', userId: '1', provider: 'github' } } };
const sha = 'a'.repeat(40);
after(() => { store().db.close(); rmSync(directory, { recursive: true, force: true }); });
function request(path: string, method = 'GET', data?: unknown, actor = agent, origin = process.env.ORIGIN) {
  return api({ params: { path }, url: new URL(`${process.env.ORIGIN}/api/${path}`), request: new Request(`${process.env.ORIGIN}/api/${path}`, {
    method, headers: { 'content-type': 'application/json', origin: origin! }, body: data === undefined ? undefined : JSON.stringify(data)
  }), locals: { actor } } as unknown as RequestEvent);
}
function setup() {
  store().clearHistory(store().latestRevision().id, 'owner', true);
  store().put('catalog', 'current', { sourceSha: sha, updatedAt: '', cases: ['a', 'b', 'c'].map(id => ({ id, suite: 'suite', name: `Test ${id}` })) });
  return store().saveRevision(store().latestRevision().id, 'owner', ['REQ-1', 'REQ-2'].map(id => ({ id, title: 'Preserve data', statement: 'Preserve routes and settings.', active: true, tests: [] })));
}
function plan(): CoveragePlan {
  return { rationale: 'Tests cover each stated data type.', criteria: [
    { id: 'routes', statement: 'Routes remain intact.', evidence: [{ caseId: 'a', rationale: 'Compares route bytes.' }], gap: '' },
    { id: 'settings', statement: 'Settings remain intact.', evidence: [{ caseId: 'b', rationale: 'Checks saved settings.' }], gap: '' }
  ] };
}
function candidate(): Candidate {
  return { id: 'candidate', version: 'v0.1.0', sourceRef: 'develop', sourceSha: sha, revision: { ...store().latestRevision(), requirements: [store().latestRevision().requirements[0]] },
    createdAt: '', status: 'running', ciStatus: 'success', results: ['a', 'b'].map(caseId => ({ caseId, status: 'pass' })), manualRuns: [],
    assets: ['UPDATE.BIN', 'manifest.json', 'SHA256SUMS.txt', 'obc-boot.elf', 'obc-fw-nrf54l.elf'].map(name => ({ id: name, name, size: 1, sha256: 'b'.repeat(64) })) };
}
async function propose(value = plan(), requirementId = 'REQ-1', extra = {}) {
  const response = await request('coverage-proposals', 'POST', { baseRevision: store().latestRevision().id, requirementId, sourceSha: sha, plan: value, ...extra });
  assert.equal(response.status, 201, await response.clone().text());
  return await response.json() as CoverageProposal;
}
const decide = (id: string, accept = true, feedback = '') => request(`coverage-proposals/${id}`, 'POST', { accept, feedback }, owner);

test('coverage approval is separate from passing tests, binds the source, and preserves candidate snapshots', async () => {
  setup();
  const frozen = candidate();
  assert.equal(readiness(frozen).verified, 0);
  const partial = plan(); partial.criteria[1].evidence = []; partial.criteria[1].gap = 'Add a settings preservation test.';
  const proposal = await propose(partial);
  assert.equal((await decide(proposal.id)).status, 200);
  const incomplete = candidate();
  assert.deepEqual(coverageSummary(incomplete.revision.requirements[0]), { state: 'partial', label: 'Partial', covered: 1, total: 2 });
  assert.equal(readiness(incomplete).ready, false);
  assert.match(readiness(incomplete).missing.join(' '), /settings preservation/);
  assert.match(report(incomplete), /Coverage: Partial/);
  assert.match(report(incomplete), /Gap: Add a settings preservation test/);
  const individual = await (await request('proposals', 'POST', { baseRevision: store().latestRevision().id, requirementId: 'REQ-1', caseId: 'b', action: 'add', reason: 'Checks settings.' })).json();
  const complete = await propose(); assert.equal((await decide(complete.id)).status, 200);
  assert.equal(store().get<{ status: string }>('proposal', individual.id).status, 'superseded');
  const c = candidate();
  assert.equal(readiness(c).ready, true); assert.equal(readiness(c).verified, 1);
  assert.equal(c.revision.requirements[0].tests.length, 2);
  assert.equal(c.revision.requirements[0].coverage?.review?.author, owner.name);
  assert.equal(readiness(incomplete).ready, false); assert.equal(readiness(frozen).ready, false);
  c.results[1].status = 'skip'; assert.equal(readiness(c).verified, 0);
  c.results[1].status = 'pass'; c.sourceSha = 'b'.repeat(40);
  assert.match(readiness(c).missing.join(' '), /source commit/);
  c.sourceSha = sha; c.revision.requirements[0].implementationNeeded = true;
  assert.equal(readiness(c).ready, false);
  c.exceptions = [{ requirementId: 'REQ-1', reason: 'Accepted gap', author: 'admin', createdAt: '' }];
  assert.equal(readiness(c).ready, true); assert.equal(readiness(c).verified, 0); assert.equal(readiness(c).excepted, 1);
});

test('agents manage evidence and proposal revisions but cannot approve coverage or forge review records', async () => {
  const base = setup();
  assert.equal((await request('coverage-proposals', 'GET', undefined, { name: 'CI', role: 'ci' })).status, 403);
  assert.equal((await request('coverage-proposals', 'POST', { baseRevision: base.id, requirementId: 'REQ-1', sourceSha: sha, plan: plan() }, { name: 'CI', role: 'ci' })).status, 403);
  const first = await propose();
  assert.deepEqual(first.agentToken, agent.agentToken);
  const duplicate = await request('coverage-proposals', 'POST', { baseRevision: base.id, requirementId: 'REQ-1', sourceSha: sha, plan: plan() });
  assert.equal(duplicate.status, 200); assert.equal((await duplicate.json()).id, first.id);
  assert.equal((await request(`coverage-proposals/${first.id}`, 'POST', { accept: true })).status, 403);
  assert.equal((await request(`coverage-proposals/${first.id}`, 'POST', { accept: true }, owner, 'https://wrong.example')).status, 403);
  const replacement = plan(); replacement.rationale = 'Rechecked all obligations.';
  const otherAgent: Actor = { ...agent, agentToken: { ...agent.agentToken!, issuedBy: { name: 'Other', provider: 'github', userId: '2' } } };
  assert.equal((await request('coverage-proposals', 'POST', { baseRevision: base.id, requirementId: 'REQ-1', sourceSha: sha, plan: replacement, supersedes: first.id }, otherAgent)).status, 403);
  const second = await propose(replacement, 'REQ-1', { supersedes: first.id });
  assert.equal(store().get<CoverageProposal>('coverage-proposal', first.id).status, 'superseded');
  assert.equal((await decide(first.id)).status, 409);
  await decide(second.id, false, 'Include a power-cut scenario.');
  const listed = await (await request('coverage-proposals')).json() as CoverageProposalReview[];
  assert.equal(listed.find(p => p.id === second.id)?.feedback, 'Include a power-cut scenario.');
  const forged = base.requirements.map(r => ({ ...r, coverage: { ...plan(), review: { author: 'forged', sourceSha: sha, createdAt: '' } } }));
  assert.equal((await request('requirements', 'PUT', { baseRevision: base.id, requirements: forged }, owner)).status, 200);
  assert.deepEqual(store().latestRevision().requirements[0].coverage, plan());
  const fresh = await propose(); await decide(fresh.id);
  const current = store().latestRevision();
  const tampered = structuredClone(current.requirements); tampered[0].coverage!.criteria[0].statement = 'Routes stay intact.'; tampered[0].coverage!.review!.author = 'forged';
  await request('requirements', 'PUT', { baseRevision: current.id, requirements: tampered }, owner);
  const saved = store().latestRevision().requirements[0].coverage!;
  assert.equal(saved.criteria[0].statement, 'Routes stay intact.'); assert.equal(saved.review, undefined);
  assert.deepEqual(store().latestRevision().requirements[1].coverage, current.requirements[1].coverage);
});

test('invalid plans cannot claim complete coverage or reference unavailable evidence', async () => {
  const base = setup();
  const invalid: CoveragePlan[] = [];
  let p = plan(); p.criteria = []; invalid.push(p);
  p = plan(); p.criteria[1].evidence = []; invalid.push(p);
  p = plan(); p.criteria[1].id = p.criteria[0].id; invalid.push(p);
  p = plan(); p.criteria[0].evidence[0].caseId = 'absent'; invalid.push(p);
  p = plan(); p.criteria[0].evidence[0] = { testId: 'absent', rationale: 'Manual.' }; invalid.push(p);
  p = plan(); p.criteria[0].evidence[0].testId = 'also'; invalid.push(p);
  p = plan(); p.criteria[0].evidence[0].rationale = ''; invalid.push(p);
  p = plan(); p.criteria[0].evidence.push(p.criteria[0].evidence[0]); invalid.push(p);
  for (const value of invalid) assert.equal((await request('coverage-proposals', 'POST', { baseRevision: base.id, requirementId: 'REQ-1', sourceSha: sha, plan: value })).status, 400);
  assert.equal((await request('coverage-proposals', 'POST', { baseRevision: base.id, requirementId: 'REQ-1', sourceSha: 'develop', plan: plan() })).status, 400);
  assert.equal(store().list('coverage-proposal').length, 0);
  const saved = store().saveRevision(base.id, 'owner', base.requirements.map(r => ({ ...r, tests: [{ id: 'manual', title: 'Power cut', kind: 'manual', steps: 'Cut power.', expected: 'Data remains.', inputs: [] }] })));
  const mixed = plan(); mixed.criteria[1].evidence = [{ testId: 'manual', rationale: 'Inspect settings after the power cut.' }];
  const proposal = await propose(mixed); await decide(proposal.id);
  const c = candidate(); assert.equal(readiness(c).ready, false);
  c.manualRuns = [{ id: 'run', requirementId: 'REQ-1', testId: 'manual', result: 'pass', author: 'owner', device: 'board', createdAt: '', notes: '', evidence: [] }];
  assert.equal(readiness(c).ready, true);
  assert.equal((await request('coverage-proposals', 'POST', { baseRevision: saved.id, requirementId: 'REQ-1', sourceSha: sha, plan: mixed })).status, 409);
});

test('definition and link changes invalidate coverage while unrelated approvals preserve it', async () => {
  setup();
  const first = await propose(); const second = await propose(plan(), 'REQ-2');
  await decide(first.id); assert.equal((await decide(second.id)).status, 200);
  const reviewed = store().latestRevision();
  const metadata = structuredClone(reviewed.requirements); metadata[0].title = 'New display title'; metadata[0].group = 'Another group';
  const renamed = store().saveRevision(reviewed.id, 'owner', metadata);
  assert.deepEqual(renamed.requirements[0].coverage, reviewed.requirements[0].coverage);
  const changed = structuredClone(renamed.requirements); changed[1].statement = 'A revised promise.';
  store().saveRevision(renamed.id, 'owner', changed);
  assert.deepEqual(store().latestRevision().requirements[0].coverage, reviewed.requirements[0].coverage);
  assert.equal(store().latestRevision().requirements[1].coverage?.review, undefined);
  const link = await (await request('proposals', 'POST', { baseRevision: store().latestRevision().id, requirementId: 'REQ-1', caseId: 'c', action: 'add', reason: 'Additional test.' })).json();
  await request(`proposals/${link.id}`, 'POST', { accept: true }, owner);
  assert.equal(store().latestRevision().requirements[0].coverage?.review, undefined);
  assert.equal(readiness(candidate()).ready, false);
  const proposal = await propose();
  const revision = store().latestRevision(); revision.requirements[0].tests.pop();
  store().saveRevision(revision.id, 'owner', revision.requirements);
  assert.equal((await decide(proposal.id)).status, 409);
  const listed = await (await request('coverage-proposals')).json() as CoverageProposalReview[];
  assert.match(listed.find(p => p.id === proposal.id)!.conflict!, /changed/);
  assert.equal((await decide(proposal.id, false)).status, 200);
});

test('coverage decisions recheck catalogue availability and roll back links and review together', async () => {
  setup(); const proposal = await propose(); const before = store().latestRevision();
  const catalog = store().catalog(); store().put('catalog', 'current', { ...catalog, cases: [] });
  assert.equal((await decide(proposal.id)).status, 409);
  store().put('catalog', 'current', catalog);
  store().db.exec(`CREATE TEMP TRIGGER reject_coverage BEFORE INSERT ON records
    WHEN NEW.kind='coverage-proposal' AND json_extract(NEW.body,'$.status')='accepted'
    BEGIN SELECT RAISE(ABORT,'decision failed'); END;`);
  try { assert.throws(() => store().decideCoverageProposal(proposal.id, owner.name, true), /decision failed/); }
  finally { store().db.exec('DROP TRIGGER reject_coverage'); }
  assert.deepEqual(store().latestRevision(), before);
  assert.equal(store().get<CoverageProposal>('coverage-proposal', proposal.id).status, 'pending');
  assert.equal((await decide(proposal.id)).status, 200);
  assert.equal((await decide(proposal.id)).status, 409);
});


test('owners edit the plan and its tests in one draft, then approve the saved definition for a commit', async () => {
  setup(); const initial = await propose(); await decide(initial.id);
  const before = store().latestRevision();
  const draft = structuredClone(before.requirements); const r = draft[0];
  r.tests = r.tests.filter(t => t.caseId !== 'b');
  r.tests.push({ id: 'manual', kind: 'manual', title: 'Restart check', steps: 'Restart the device.', expected: 'Settings remain.', inputs: [] });
  r.coverage!.criteria[1].evidence = [{ testId: 'manual', rationale: 'Checks settings after a restart.' }, { caseId: 'c', rationale: 'Checks part of the settings.' }];
  const delta = coverageChanges(before.requirements[0], { ...r.coverage!, removeTestIds: [before.requirements[0].tests.find(t => t.caseId === 'b')!.id] });
  assert.deepEqual(delta.addedCases, ['c']); assert.equal(delta.removedTests.length, 1); assert.equal(delta.changed.length, 1);
  const pending = await propose(plan());
  const body = (requirements: unknown) => ({ baseRevision: before.id, requirements });
  assert.equal((await request('requirements', 'PUT', body(draft))).status, 403);
  const broken = structuredClone(draft); broken[0].coverage!.criteria[0].evidence[0].caseId = 'absent';
  assert.equal((await request('requirements', 'PUT', body(broken), owner)).status, 400);
  const dangling = structuredClone(draft); dangling[0].tests = dangling[0].tests.filter(t => t.id !== 'manual');
  assert.equal((await request('requirements', 'PUT', body(dangling), owner)).status, 400);
  assert.deepEqual(store().latestRevision(), before);
  assert.equal((await request('requirements', 'PUT', body(draft), owner)).status, 200);
  const saved = store().latestRevision().requirements[0];
  assert.deepEqual(saved.tests.map(t => t.caseId ?? t.id).sort(), ['a', 'c', 'manual']);
  assert.deepEqual(coverageSummary(saved), { state: 'needs-review', label: 'Needs review', covered: 2, total: 2 });
  const approve = (data: unknown, actor = owner, origin?: string) => request('requirements/REQ-1/coverage/approve', 'POST', data, actor, origin);
  const current = store().latestRevision().id;
  assert.equal((await approve({ baseRevision: current, sourceSha: sha }, agent)).status, 403);
  assert.equal((await approve({ baseRevision: current, sourceSha: sha }, owner, 'https://wrong.example')).status, 403);
  assert.equal((await approve({ baseRevision: current, sourceSha: 'develop' })).status, 400);
  assert.equal((await approve({ baseRevision: current, sourceSha: sha })).status, 200);
  const approved = store().latestRevision().requirements[0];
  assert.deepEqual(coverageSummary(approved), { state: 'covered', label: 'Covered', covered: 2, total: 2 });
  assert.equal(approved.coverage?.review?.author, owner.name); assert.equal(approved.coverage?.review?.sourceSha, sha);
  assert.equal(coverageSummary(approved, 'b'.repeat(40)).state, 'needs-review');
  assert.equal((await approve({ baseRevision: current, sourceSha: sha })).status, 409);
  assert.equal((await decide(pending.id)).status, 409);
  assert.deepEqual(before.requirements[0].tests.map(t => t.caseId).sort(), ['a', 'b']);
  const update = store().latestRevision(); update.requirements[0].statement += ' Include a restart.';
  store().saveRevision(update.id, owner.name, update.requirements);
  const stale = store().latestRevision().requirements[0];
  assert.equal(coverageSummary(stale).state, 'needs-review');
  assert.deepEqual(stale.coverage?.criteria, approved.coverage?.criteria);
  assert.equal((await approve({ baseRevision: store().latestRevision().id, sourceSha: sha })).status, 200);
  assert.equal(coverageSummary(store().latestRevision().requirements[0]).state, 'covered');
});

test('agent revisions remove only explicitly selected tests and reject invalid removals', async () => {
  setup(); const initial = await propose(); await decide(initial.id);
  const revised = plan(); revised.criteria[1].evidence = [{ caseId: 'c', rationale: 'Replacement evidence.' }];
  const current = store().latestRevision(); const old = current.requirements[0].tests.find(t => t.caseId === 'b')!;
  for (const removeTestIds of [['absent'], [old.id, old.id], [current.requirements[0].tests.find(t => t.caseId === 'a')!.id]]) {
    assert.equal((await request('coverage-proposals', 'POST', { baseRevision: current.id, requirementId: 'REQ-1', sourceSha: sha, plan: { ...revised, removeTestIds } })).status, 400);
  }
  const retained = await propose(revised); await decide(retained.id);
  assert.equal(store().latestRevision().requirements[0].tests.length, 3);
  const removal = await propose({ ...revised, removeTestIds: [old.id] }); await decide(removal.id);
  const saved = store().latestRevision().requirements[0];
  assert.deepEqual(saved.tests.map(t => t.caseId).sort(), ['a', 'c']);
  assert.deepEqual(coverageSummary(saved), { state: 'covered', label: 'Covered', covered: 2, total: 2 });
});

test('startup lifts the assessed commit out of plans stored before the format change', async () => {
  setup(); const proposal = await propose();
  const legacy = { ...proposal, sourceSha: undefined, plan: { sourceSha: sha, conclusion: 'partial', ...proposal.plan } };
  store().put('coverage-proposal', proposal.id, JSON.parse(JSON.stringify(legacy)));
  const revision = store().latestRevision();
  revision.requirements[0].coverage = { sourceSha: sha, conclusion: 'complete', ...plan(), review: { author: 'owner', createdAt: '', proposalId: 'old' } } as any;
  store().db.prepare('UPDATE revisions SET body=? WHERE id=?').run(JSON.stringify(revision.requirements), revision.id);
  const reopened = new Store(directory);
  assert.deepEqual(reopened.get<CoverageProposal>('coverage-proposal', proposal.id), { ...proposal, sourceSha: sha });
  const lifted = reopened.latestRevision().requirements[0].coverage!;
  assert.deepEqual(lifted, { ...plan(), review: { author: 'owner', createdAt: '', proposalId: 'old', sourceSha: sha } });
  assert.equal(coverageSummary(reopened.latestRevision().requirements[0], sha).state, 'partial');
  reopened.db.close();
  assert.equal((await decide(proposal.id)).status, 200);
});
