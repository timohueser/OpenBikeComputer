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
import { coverageSummary, coverageChanges, coverageProgress } from '../src/lib/coverage.ts';
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
async function propose(value = plan(), requirementId = 'REQ-1', actor = agent) {
  const response = await request('coverage-proposals', 'POST', { baseRevision: store().latestRevision().id, requirementId, sourceSha: sha, plan: value }, actor);
  assert.equal(response.status, 201, await response.clone().text());
  return await response.json() as CoverageProposal;
}
const decide = (id: string, accept = true, feedback = '') => request(`coverage-proposals/${id}`, 'POST', { accept, feedback }, owner);

test('coverage approval is separate from passing tests and preserves candidate snapshots', async () => {
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
  const complete = await propose(); assert.equal((await decide(complete.id)).status, 200);
  const c = candidate();
  assert.equal(readiness(c).ready, true); assert.equal(readiness(c).verified, 1);
  assert.equal(c.revision.requirements[0].tests.length, 2);
  assert.equal(c.revision.requirements[0].coverage?.review?.author, owner.name);
  assert.equal(readiness(incomplete).ready, false); assert.equal(readiness(frozen).ready, false);
  c.results[1].status = 'skip'; assert.equal(readiness(c).verified, 0);
  c.results[1].status = 'pass'; c.revision.requirements[0].implementationNeeded = true;
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
  const second = await propose(replacement, 'REQ-1', otherAgent);
  assert.equal(second.supersedes, first.id);
  assert.equal(store().get<CoverageProposal>('coverage-proposal', first.id).status, 'superseded');
  assert.equal((await decide(first.id)).status, 409);
  store().put('coverage-proposal', 'extra-pending', { ...second, id: 'extra-pending' });
  const third = await propose(plan(), 'REQ-1', otherAgent);
  assert.deepEqual(store().list<CoverageProposal>('coverage-proposal').filter(p => p.status === 'pending' && p.requirementId === 'REQ-1').map(p => p.id), [third.id]);
  await decide(third.id, false, 'Include a power-cut scenario.');
  const listed = await (await request('coverage-proposals')).json() as CoverageProposalReview[];
  assert.equal(listed.find(p => p.id === third.id)?.feedback, 'Include a power-cut scenario.');
  const forged = base.requirements.map(r => ({ ...r, coverage: { ...plan(), review: { author: 'forged', sourceSha: sha, createdAt: '' } } }));
  assert.equal((await request('requirements', 'PUT', { baseRevision: base.id, requirements: forged }, owner)).status, 200);
  const stamped = store().latestRevision().requirements[0].coverage!;
  assert.equal(stamped.review?.author, owner.name); assert.deepEqual({ rationale: stamped.rationale, criteria: stamped.criteria }, plan());
  const fresh = await propose(); await decide(fresh.id);
  const current = store().latestRevision();
  const tampered = structuredClone(current.requirements); tampered[0].coverage!.criteria[0].statement = 'Routes stay intact.'; tampered[0].coverage!.review!.author = 'forged';
  await request('requirements', 'PUT', { baseRevision: current.id, requirements: tampered }, owner);
  const saved = store().latestRevision().requirements[0].coverage!;
  assert.equal(saved.criteria[0].statement, 'Routes stay intact.'); assert.equal(saved.review?.author, owner.name); assert.equal(saved.review?.proposalId, undefined);
  assert.deepEqual(store().latestRevision().requirements[1].coverage, current.requirements[1].coverage);
});

test('invalid plans cannot claim complete coverage or reference unavailable evidence', async () => {
  const base = setup();
  const invalid: CoveragePlan[] = [];
  let p = plan(); p.criteria = []; invalid.push(p);
  p = plan(); p.criteria[1].id = p.criteria[0].id; invalid.push(p);
  p = plan(); p.criteria[0].evidence[0].caseId = 'absent'; invalid.push(p);
  p = plan(); p.criteria[0].evidence[0] = { testId: 'absent', rationale: 'Manual.' }; invalid.push(p);
  p = plan(); p.criteria[0].evidence[0].testId = 'also'; invalid.push(p);
  p = plan(); p.criteria[0].evidence[0].rationale = ''; invalid.push(p);
  p = plan(); p.criteria[0].evidence.push(p.criteria[0].evidence[0]); invalid.push(p);
  // A covered criterion has nothing left to build, so a next test without a gap is refused: it is
  // how a manual procedure gets written up twice and leaves a covered criterion looking unfinished.
  p = plan(); p.criteria[0].next = { level: 'unit', summary: 'Assert the saved value.' }; invalid.push(p);
  for (const value of invalid) assert.equal((await request('coverage-proposals', 'POST', { baseRevision: base.id, requirementId: 'REQ-1', sourceSha: sha, plan: value })).status, 400);
  // A renamed test is the common way evidence goes missing, and the two ways it can go missing have
  // different fixes — so each refusal names the ID it could not find and says where to look for it.
  const absentCase = plan(); absentCase.criteria[0].evidence[0].caseId = 'renamed-away';
  const absentManual = plan(); absentManual.criteria[0].evidence[0] = { testId: 'no-such-procedure', rationale: 'Manual.' };
  const refusal = async (value: CoveragePlan) => (await (await request('coverage-proposals', 'POST', { baseRevision: base.id, requirementId: 'REQ-1', sourceSha: sha, plan: value })).json() as { error: string }).error;
  assert.match(await refusal(absentCase), /catalogue.*“renamed-away”.*renamed/s);
  assert.match(await refusal(absentManual), /procedure.*“no-such-procedure”/s);
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

test('a save approves the plans it changes and keeps the reviews it leaves untouched', async () => {
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
  const restated = store().latestRevision().requirements[1].coverage?.review;
  assert.equal(restated?.author, 'owner'); assert.equal(restated?.sourceSha, sha); assert.equal(restated?.proposalId, undefined);
  const linked = store().latestRevision();
  linked.requirements[0].tests.push({ id: 'extra', kind: 'automated', title: 'Test c', caseId: 'c', inputs: [] });
  store().saveRevision(linked.id, 'owner', linked.requirements);
  assert.equal(store().latestRevision().requirements[0].coverage?.review?.author, 'owner');
  assert.equal(readiness(candidate()).ready, true);
  const proposal = await propose();
  const revision = store().latestRevision(); revision.requirements[0].tests.pop(); revision.requirements[0].statement = 'Preserve routes.';
  store().saveRevision(revision.id, 'owner', revision.requirements);
  const listed = (await (await request('coverage-proposals')).json() as CoverageProposalReview[]).find(p => p.id === proposal.id)!;
  assert.equal(listed.conflict, undefined);
  assert.match(listed.stale!, /statement and tests changed/);
  assert.equal((await decide(proposal.id)).status, 200);
  assert.equal(store().latestRevision().requirements[0].coverage?.review?.proposalId, proposal.id);
  const renewal = await propose();
  const relabelled = store().latestRevision(); relabelled.requirements[0].title = 'Keep data';
  store().saveRevision(relabelled.id, 'owner', relabelled.requirements);
  const pending = (await (await request('coverage-proposals')).json() as CoverageProposalReview[]).find(p => p.id === renewal.id)!;
  assert.equal(pending.stale, undefined);
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


test('owners edit the plan and its tests in one draft, and the save approves it', async () => {
  setup(); const initial = await propose(); await decide(initial.id);
  const before = store().latestRevision();
  const draft = structuredClone(before.requirements); const r = draft[0];
  r.tests = r.tests.filter(t => t.caseId !== 'b');
  r.tests.push({ id: 'manual', kind: 'manual', title: 'Restart check', steps: 'Restart the device.', expected: 'Settings remain.', inputs: [] });
  r.coverage!.criteria[1].evidence = [{ testId: 'manual', rationale: 'Checks settings after a restart.' }, { caseId: 'c', rationale: 'Checks part of the settings.' }];
  const delta = coverageChanges(before.requirements[0], r.coverage!);
  assert.equal(delta.changed.length, 1); assert.equal(delta.added.length, 0); assert.equal(delta.removed.length, 0);
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
  assert.deepEqual(coverageSummary(saved), { state: 'covered', label: 'Covered', covered: 2, total: 2 });
  assert.equal(saved.coverage?.review?.author, owner.name); assert.equal(saved.coverage?.review?.sourceSha, sha);
  assert.equal(saved.coverage?.review?.proposalId, undefined);
  const listed = (await (await request('coverage-proposals')).json() as CoverageProposalReview[]).find(p => p.id === pending.id)!;
  assert.match(listed.stale!, /tests and coverage plan changed/); assert.equal((await decide(pending.id, false)).status, 200);
  assert.deepEqual(before.requirements[0].tests.map(t => t.caseId).sort(), ['a', 'b']);
  const update = store().latestRevision(); update.requirements[0].statement += ' Include a restart.';
  store().saveRevision(update.id, owner.name, update.requirements);
  const restated = store().latestRevision().requirements[0];
  assert.equal(coverageSummary(restated).state, 'covered');
  assert.deepEqual(restated.coverage?.criteria, saved.coverage?.criteria);
  assert.notEqual(restated.coverage?.review?.createdAt, saved.coverage?.review?.createdAt);
  store().put('catalog', 'current', { ...store().catalog(), sourceSha: '' });
  const offline = store().latestRevision(); offline.requirements[0].statement += ' Twice.';
  store().saveRevision(offline.id, owner.name, offline.requirements);
  const local = store().latestRevision().requirements[0];
  assert.equal(local.coverage?.review?.sourceSha, undefined);
  assert.doesNotMatch(report({ ...candidate(), revision: { ...store().latestRevision(), requirements: [local] } }), /source <code>/);
});

test('a requirement carries exactly the tests its plan cites', async () => {
  setup(); const initial = await propose(); await decide(initial.id);
  const current = store().latestRevision();
  const draft = structuredClone(current.requirements);
  draft[0].tests.push({ id: 'manual', kind: 'manual', title: 'Power cut', steps: 'Cut power.', expected: 'Data remains.', inputs: [] });
  draft[0].coverage!.criteria[1].evidence.push({ testId: 'manual', rationale: 'Inspects settings after a power cut.' });
  assert.equal((await request('requirements', 'PUT', { baseRevision: current.id, requirements: draft }, owner)).status, 200);
  assert.deepEqual(store().latestRevision().requirements[0].tests.map(t => t.caseId ?? t.id).sort(), ['a', 'b', 'manual']);
  // A plan that omits evidence unlinks that test; a manual procedure it no longer cites goes with its content.
  const revised = plan(); revised.criteria[1].evidence = [{ caseId: 'c', rationale: 'Replacement evidence.' }];
  assert.deepEqual(coverageChanges(store().latestRevision().requirements[0], revised).deletedProcedures.map(t => t.title), ['Power cut']);
  const replacement = await propose(revised); await decide(replacement.id);
  const saved = store().latestRevision().requirements[0];
  assert.deepEqual(saved.tests.map(t => t.caseId ?? t.id).sort(), ['a', 'c']);
  assert.deepEqual(coverageSummary(saved), { state: 'covered', label: 'Covered', covered: 2, total: 2 });
  // A requirement whose plan is deleted keeps no tests.
  const without = structuredClone(store().latestRevision().requirements); delete without[0].coverage;
  assert.equal((await request('requirements', 'PUT', { baseRevision: store().latestRevision().id, requirements: without }, owner)).status, 200);
  assert.deepEqual(store().latestRevision().requirements[0].tests, []);
});

test('an older snapshot is judged by its plan, not by the tests it still carries', async () => {
  setup(); const approved = await propose(); await decide(approved.id);
  const frozen = candidate();
  const requirement = frozen.revision.requirements[0];
  requirement.tests.push({ id: 'legacy-manual', kind: 'manual', title: 'Legacy power cut', steps: 'Cut power.', expected: 'Data remains.', inputs: [] },
    { id: 'legacy-auto', kind: 'automated', title: 'Legacy smoke test', caseId: 'c', inputs: [] });
  frozen.results.push({ caseId: 'c', status: 'fail' });
  frozen.manualRuns.push({ id: 'run', requirementId: 'REQ-1', testId: 'legacy-manual', result: 'pass', device: 'board', notes: 'Legacy note.', evidence: [], author: 'owner', createdAt: '' });
  assert.equal(readiness(frozen).ready, true); assert.equal(readiness(frozen).verified, 1);
  assert.doesNotMatch(report(frozen), /Legacy smoke test|Legacy power cut|Legacy note/);
});

test('a plan may propose criteria before any evidence', async () => {
  setup();
  const outline = { rationale: '', criteria: [{ id: 'routes', statement: 'Routes remain intact.', evidence: [], gap: '' }, { id: 'settings', statement: 'Settings remain intact.', evidence: [], gap: 'Needs a settings test.' }] };
  const proposal = await propose(outline);
  assert.equal((await decide(proposal.id)).status, 200);
  const saved = store().latestRevision().requirements[0];
  assert.deepEqual(coverageSummary(saved), { state: 'partial', label: 'Partial', covered: 0, total: 2 });
  assert.equal(saved.tests.length, 0);
  assert.match(readiness({ ...candidate(), revision: { ...store().latestRevision(), requirements: [saved] } }).missing.join(' '), /Routes remain intact.*: no evidence mapped/);
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
  assert.equal(coverageSummary(reopened.latestRevision().requirements[0]).state, 'partial');
  reopened.db.close();
  assert.equal((await decide(proposal.id)).status, 200);
});

test('a snapshot with an unapproved plan from the earlier workflow does not pass the gate', async () => {
  setup(); const proposal = await propose(); await decide(proposal.id);
  const c = candidate(); delete c.revision.requirements[0].coverage!.review;
  assert.equal(coverageSummary(c.revision.requirements[0]).state, 'needs-review');
  assert.equal(readiness(c).ready, false);
  assert.match(readiness(c).missing.join(' '), /not been approved/);
});

test('a proposal may name the next test to build and bring a manual procedure that approval creates', async () => {
  setup();
  const procedure = { id: 'ride-restart', title: 'Ride check after restart', steps: 'Restart, then ride.', expected: 'The choice holds.' };
  const value = plan();
  value.criteria[1].evidence = [{ testId: 'ride-restart', rationale: 'Confirms the choice on the device.' }];
  value.criteria[1].gap = 'No automated check yet.';
  value.criteria[1].next = { level: 'unit', summary: 'Persist the choice, reload, and assert the stored value.' };
  const post = (body: Record<string, unknown>) => request('coverage-proposals', 'POST', { baseRevision: store().latestRevision().id, requirementId: 'REQ-1', sourceSha: sha, ...body });
  assert.equal((await post({ plan: value })).status, 400);
  const uncited = await post({ plan: value, procedures: [procedure, { ...procedure, id: 'extra' }] });
  assert.equal(uncited.status, 400); assert.match(await uncited.text(), /must be cited/);
  value.criteria[0].evidence.push({ testId: 'ride-restart', rationale: 'Also confirms routes after the restart.' });
  assert.equal((await post({ plan: { ...value, criteria: [{ ...value.criteria[1], next: { level: 'bogus', summary: 'x' } }] }, procedures: [procedure] })).status, 400);
  const created = await post({ plan: value, procedures: [procedure] });
  assert.equal(created.status, 201, await created.clone().text());
  const proposal = await created.json() as CoverageProposal;
  assert.equal(proposal.procedures?.[0].kind, 'manual');
  assert.equal(coverageChanges(store().latestRevision().requirements[0], proposal.plan).deletedProcedures.length, 0);
  assert.equal((await decide(proposal.id, false, 'Prefer an automated test.')).status, 200);
  assert.deepEqual(store().latestRevision().requirements[0].tests, []);
  const again = await post({ plan: value, procedures: [procedure] });
  assert.equal((await decide((await again.json() as CoverageProposal).id)).status, 200);
  const approved = store().latestRevision().requirements[0];
  assert.deepEqual(approved.tests.filter(t => t.id === 'ride-restart'), [{ ...procedure, kind: 'manual', inputs: [] }]);
  assert.deepEqual(approved.coverage?.criteria[1].next, value.criteria[1].next);
  assert.deepEqual(coverageSummary(approved), { state: 'partial', label: 'Partial', covered: 1, total: 2 });
  const draft = structuredClone(store().latestRevision().requirements); delete draft[0].coverage!.criteria[1].next;
  store().saveRevision(store().latestRevision().id, 'owner', draft);
  assert.equal(store().latestRevision().requirements[0].coverage?.criteria[1].next, undefined);
});

test('coverage progress counts active requirements, their criteria, and the catalogue tests their plans cite', async () => {
  setup(); const proposal = await propose(); await decide(proposal.id);
  const revision = store().latestRevision();
  const catalog = store().catalog();
  assert.deepEqual(coverageProgress(revision.requirements, catalog), { states: { unassessed: 1, 'needs-review': 0, partial: 0, covered: 1 }, active: 2, criteria: { covered: 2, total: 2 }, tests: { cited: 2, catalog: 3, manual: 0 } });
  const excluded = structuredClone(revision.requirements); excluded[1].active = false;
  assert.equal(coverageProgress(excluded, catalog).active, 1);
});
