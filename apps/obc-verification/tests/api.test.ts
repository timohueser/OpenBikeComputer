import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RequestEvent } from '@sveltejs/kit';
import type { Actor, Candidate } from '../src/lib/types.ts';
import { api } from '../src/lib/server/api.ts';
import { store } from '../src/lib/server/store.ts';
import { report } from '../src/lib/server/report.ts';
import { readiness } from '../src/lib/server/domain.ts';
import { boundedBody } from '../src/lib/server/files.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-verification-api-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.ORIGIN = 'https://verify.example.com';
function request(path: string, method = 'GET', data?: unknown, actor?: Actor, origin = process.env.ORIGIN) {
  return api({ params: { path }, url: new URL(`${process.env.ORIGIN}/api/${path}`), request: new Request(`${process.env.ORIGIN}/api/${path}`, { method, headers: { 'content-type': 'application/json', ...(origin ? { origin } : {}) }, body: data === undefined ? undefined : JSON.stringify(data) }), locals: { actor }, cookies: { get: () => undefined }, getClientAddress: () => '127.0.0.1' } as unknown as RequestEvent);
}
test('API enforces prose ownership, origins, proposed link approval, and frozen evidence', async () => {
  const owner: Actor = { name: 'owner', role: 'owner' };
  const agent: Actor = { name: 'agent', role: 'agent' };
  const ci: Actor = { name: 'ci', role: 'ci' };
  try {
    assert.equal((await request('bootstrap')).status, 401);
    process.env.VERIFICATION_OWNER_USERNAME = 'owner';
    store().db.prepare('INSERT OR IGNORE INTO local_admin(id,password_hash) VALUES(1,?)').run('a'.repeat(32) + ':' + 'b'.repeat(128));
    const admin: Actor = { name: 'owner', role: 'owner', provider: 'local', userId: 'local', admin: true };
    assert.equal((await request('users', 'GET', undefined, admin)).status, 200);
    const reset = { baseRevision: store().latestRevision().id, clearCurrent: true, confirmation: 'START FRESH' };
    assert.equal((await request('admin/history')).status, 401);
    store().db.prepare('INSERT INTO github_users(id,login,admin) VALUES(?,?,?)').run('7', 'member', 0);
    const member: Actor = { name: 'member', role: 'owner', provider: 'github', userId: '7', admin: true };
    for (const denied of [agent, ci, member]) {
      assert.equal((await request('admin/history', 'GET', undefined, denied)).status, 403);
      assert.equal((await request('admin/history', 'POST', reset, denied)).status, 403);
    }
    assert.equal((await request('admin/history', 'POST', reset, admin, 'https://attacker.example')).status, 403);
    assert.equal((await request('admin/history', 'POST', { ...reset, confirmation: 'CLEAR HISTORY' }, admin)).status, 400);
    assert.equal((await request('admin/history', 'POST', { ...reset, clearCurrent: 'true' }, admin)).status, 400);
    assert.equal(store().latestRevision().id, reset.baseRevision);
    assert.equal((await request('admin/history', 'GET', undefined, admin)).status, 200);
    assert.equal((await request('users/123', 'DELETE', undefined, admin, 'https://attacker.example')).status, 403);
    assert.equal((await request('account/password', 'POST', { currentPassword: 'wrong', newPassword: 'valid-new-password' }, admin)).status, 403);

    assert.equal((await request('users', 'GET', undefined, agent)).status, 403);
    assert.equal((await request('users', 'GET', undefined, ci)).status, 403);
    assert.equal((await request('users', 'POST', { login: 'alice', admin: true }, owner)).status, 403);
    assert.equal((await request('account/password', 'POST', { currentPassword: 'secret', newPassword: 'sixteen-characters' }, agent)).status, 403);
    assert.equal((await request('users/123', 'DELETE', undefined, owner, 'https://attacker.example')).status, 403);
    for (const denied of [agent, ci]) assert.equal((await request('requirements/next-id', 'POST', undefined, denied)).status, 403);
    assert.equal((await request('requirements/next-id', 'POST', undefined, owner, 'https://attacker.example')).status, 403);
    assert.equal((await request('requirements/next-id', 'POST', undefined, owner)).status, 200);
    assert.equal((await request('requirements/next-id', 'POST', { count: 0 }, owner)).status, 400);
    const batch = await (await request('requirements/next-id', 'POST', { count: 3 }, owner)).json();
    assert.equal(batch.ids.length, 3); assert.equal(batch.id, batch.ids[0]); assert.notEqual(batch.ids[0], batch.ids[2]);
    const revision = store().latestRevision();
    const payload = { baseRevision: revision.id, requirements: revision.requirements };
    assert.equal((await request('requirements', 'PUT', payload, agent)).status, 403);
    assert.equal((await request('requirements', 'PUT', payload, ci)).status, 403);
    assert.equal((await request('requirements', 'PUT', payload, owner, 'https://attacker.example')).status, 403);
    store().put('catalog', 'current', { sourceSha: 'a'.repeat(40), updatedAt: '', cases: [{ id: 'suite::class::case', suite: 'suite', name: 'A case' }] });
    const proposalResponse = await request('proposals', 'POST', { baseRevision: revision.id, requirementId: 'EXAMPLE-001', caseId: 'suite::class::case', action: 'add', reason: 'Please link the existing case.' }, agent);
    assert.equal(proposalResponse.status, 201); const proposal = await proposalResponse.json();
    assert.equal(store().latestRevision().id, revision.id);
    assert.equal((await request(`proposals/${proposal.id}`, 'POST', { accept: true }, agent)).status, 403);
    assert.equal((await request(`proposals/${proposal.id}`, 'POST', { accept: true }, owner)).status, 200);
    assert.equal(store().latestRevision().requirements[0].statement, revision.requirements[0].statement);
    const candidate: Candidate = { id: 'candidate', version: '0.1.0', sourceRef: 'develop', sourceSha: 'a'.repeat(40), createdAt: '', status: 'published', ciStatus: 'success', revision: { ...revision, requirements: [{ ...revision.requirements[0], tests: [{ id: 'manual', kind: 'manual', title: 'Check', steps: 'Do it', expected: 'Done', inputs: [] }] }] }, results: [], assets: [], manualRuns: [] };
    store().put('candidate', candidate.id, candidate); store().put('publication', candidate.id, candidate);
    store().put('publication-files', candidate.id, { html: report(candidate), evidence: JSON.stringify({ candidate, readiness: readiness(candidate) }) });
    const manual = { requirementId: 'EXAMPLE-001', testId: 'manual', result: 'pass', device: 'device', notes: '', evidence: [] };
    assert.equal((await request('candidates/candidate/runs', 'POST', manual, ci)).status, 403);
    assert.equal((await request('candidates/candidate/runs', 'POST', manual, owner)).status, 409);
    assert.equal((await request('candidates/candidate/report', 'GET', undefined, ci)).status, 200);
    const before = await (await request('candidates/candidate/evidence', 'GET', undefined, ci)).text();
    store().updateCandidate(candidate.id, (value) => { value.releaseUrl = 'https://example.com/release'; });
    assert.equal(await (await request('candidates/candidate/evidence', 'GET', undefined, ci)).text(), before);
    const decision = structuredClone(candidate);
    decision.id = 'decision'; decision.status = 'running';
    decision.revision.requirements[0].active = true;
    decision.revision.requirements.push({ ...decision.revision.requirements[0], id: 'TODO', todo: true }, { ...decision.revision.requirements[0], id: 'INACTIVE', active: false });
    store().put('candidate', decision.id, decision);
    const exception = { requirementId: 'EXAMPLE-001', reason: '  Device recovery is not yet reliable.  ', author: 'forged' };
    for (const denied of [agent, ci, member, owner]) {
      assert.equal((await request('candidates/decision/exceptions', 'POST', exception, denied)).status, 403);
      assert.equal((await request('candidates/decision/exceptions/EXAMPLE-001', 'DELETE', undefined, denied)).status, 403);
    }
    assert.equal((await request('candidates/decision/exceptions', 'POST', exception, admin, 'https://attacker.example')).status, 403);
    assert.equal((await request('candidates/decision/exceptions', 'POST', { ...exception, reason: ' ' }, admin)).status, 400);
    assert.equal((await request('candidates/decision/exceptions', 'POST', { ...exception, reason: 'x'.repeat(5001) }, admin)).status, 400);
    assert.equal((await request('candidates/decision/exceptions', 'POST', { ...exception, requirementId: 'missing' }, admin)).status, 404);
    assert.equal((await request('candidates/decision/exceptions', 'POST', { ...exception, requirementId: 'INACTIVE' }, admin)).status, 404);
    assert.equal((await request('candidates/decision/exceptions', 'POST', { ...exception, requirementId: 'TODO' }, admin)).status, 409);
    const acceptedResponse = await request('candidates/decision/exceptions', 'POST', exception, admin);
    assert.equal(acceptedResponse.status, 200);
    const accepted: Candidate = await acceptedResponse.json();
    assert.equal(accepted.exceptions?.[0].reason, 'Device recovery is not yet reliable.');
    assert.equal(accepted.exceptions?.[0].author, 'owner');
    assert(accepted.exceptions?.[0].createdAt);
    assert.deepEqual(accepted.revision, decision.revision);
    assert.equal((await request('candidates/decision/exceptions', 'POST', exception, admin)).status, 409);
    assert.equal((await request('candidates/decision/exceptions/EXAMPLE-001', 'DELETE', undefined, admin)).status, 200);
    assert.deepEqual(store().candidate('decision').exceptions, []);
    const audit = store().db.prepare("SELECT body FROM records WHERE kind='candidate' AND id='decision' ORDER BY seq").all();
    assert(audit.some((row) => JSON.parse(String(row.body)).exceptions?.[0]?.reason === 'Device recovery is not yet reliable.'));
    assert.equal((await request('candidates/decision/exceptions/EXAMPLE-001', 'DELETE', undefined, admin)).status, 404);
    for (const frozen of [{ status: 'publishing' as const }, { status: 'published' as const }, { evidenceFrozen: true }]) {
      store().put('candidate', 'decision', { ...accepted, ...frozen });
      assert.equal((await request('candidates/decision/exceptions', 'POST', exception, admin)).status, 409);
      assert.equal((await request('candidates/decision/exceptions/EXAMPLE-001', 'DELETE', undefined, admin)).status, 409);
    }
    store().put('candidate', 'decision', accepted); store().put('publication', 'decision', accepted);
    assert.equal((await request('candidates/decision/report', 'GET', undefined, ci)).status, 409);
    assert.equal((await request('candidates/decision/evidence', 'GET', undefined, ci)).status, 409);
    assert.equal((await request('candidates/decision/exceptions/EXAMPLE-001', 'DELETE', undefined, admin)).status, 409);
    const publishable = structuredClone(candidate);
    publishable.id = 'publish-review'; publishable.version = 'v0.2.0'; publishable.status = 'ready';
    publishable.revision.requirements[0].active = true;
    publishable.manualRuns = [{ ...manual, id: 'passed', result: 'pass', author: 'owner', createdAt: '' }];
    publishable.assets = ['UPDATE.BIN', 'manifest.json', 'SHA256SUMS.txt', 'obc-boot.elf', 'obc-fw-nrf54l.elf'].map((name) => ({ id: name, name, size: 1, sha256: 'a'.repeat(64) }));
    publishable.exceptions = [{ requirementId: 'EXAMPLE-001', reason: 'A newly accepted gap', author: 'owner', createdAt: '2026-09-16T00:00:00Z' }];
    store().put('candidate', publishable.id, publishable);
    assert.equal((await request('candidates/publish-review/publish', 'POST', {}, owner)).status, 400);
    const outdated = await request('candidates/publish-review/publish', 'POST', { exceptions: [] }, owner);
    assert.equal(outdated.status, 409); assert.match((await outdated.json()).error, /Exceptions changed/);
    assert.equal((await request('candidates/publish-review/publish', 'POST', { exceptions: [{ ...publishable.exceptions[0], reason: 'Old reason' }] }, owner)).status, 409);
    assert.equal(store().candidate(publishable.id).status, 'ready');
    assert.equal(store().maybe('publication', publishable.id), undefined);
    const originalFetch = globalThis.fetch;
    const originalToken = process.env.GITHUB_TOKEN; const originalRepo = process.env.GITHUB_REPOSITORY;
    process.env.GITHUB_TOKEN = 'test-token'; process.env.GITHUB_REPOSITORY = 'test/repo';
    let dispatches = 0;
    globalThis.fetch = async () => { dispatches++; return new Response('{}', { status: 422 }); };
    try {
      const reviewed = publishable.exceptions.map(({ createdAt, author, reason, requirementId }) => ({ createdAt, author, reason, requirementId }));
      assert.equal((await request('candidates/publish-review/publish', 'POST', { exceptions: reviewed }, owner)).status, 200);
      assert.equal(dispatches, 1);
      const frozenReview = store().get<Candidate>('publication', publishable.id);
      assert.deepEqual(frozenReview.exceptions, publishable.exceptions);
      assert.equal(frozenReview.evidenceFrozen, true);
      assert.equal(store().candidate(publishable.id).status, 'publishing');
      assert.equal((await request('candidates/publish-review/publish', 'POST', { exceptions: [] }, owner)).status, 409);
      assert.equal(dispatches, 1);
      const frozenFiles = store().get<{ html: string; evidence: string }>('publication-files', publishable.id);
      assert.deepEqual(JSON.parse(frozenFiles.evidence).candidate.exceptions, publishable.exceptions);
      assert.equal((await request('candidates/publish-review/publish', 'POST', { exceptions: reviewed }, owner)).status, 200);
      assert.equal(dispatches, 2);
      assert.deepEqual(store().get('publication-files', publishable.id), frozenFiles);
      store().updateCandidate(publishable.id, (value) => { value.status = 'published'; value.releaseUrl = 'https://example.com/v0.2.0'; });
      assert.equal(await (await request('candidates/publish-review/report', 'GET', undefined, ci)).text(), frozenFiles.html);
      assert.equal(await (await request('candidates/publish-review/evidence', 'GET', undefined, ci)).text(), frozenFiles.evidence);
    } finally {
      globalThis.fetch = originalFetch;
      if (originalToken === undefined) delete process.env.GITHUB_TOKEN; else process.env.GITHUB_TOKEN = originalToken;
      if (originalRepo === undefined) delete process.env.GITHUB_REPOSITORY; else process.env.GITHUB_REPOSITORY = originalRepo;
    }
    const current = store().latestRevision();
    const cleared = await request('admin/history', 'POST', { baseRevision: current.id, clearCurrent: false, confirmation: 'CLEAR HISTORY' }, admin);
    assert.equal(cleared.status, 200);
    const replacement = await cleared.json();
    assert.deepEqual(replacement.requirements, current.requirements);
    assert(replacement.id > current.id);
    assert.equal((await request('admin/history', 'POST', { baseRevision: current.id, clearCurrent: true, confirmation: 'START FRESH' }, admin)).status, 409);
    assert.equal(await (await request('candidates/candidate/evidence', 'GET', undefined, ci)).text(), before);
  } finally { store().db.close(); rmSync(directory, { recursive: true, force: true }); }
});
test('body limits apply without trusting content-length', async () => {
  const req = new Request('https://example.com', { method: 'POST', body: '12345' });
  await assert.rejects(() => boundedBody(req, 4), /too large/);
});
