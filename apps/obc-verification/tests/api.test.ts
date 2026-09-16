import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RequestEvent } from '@sveltejs/kit';
import type { Actor, Candidate } from '../src/lib/types.ts';
import { api } from '../src/lib/server/api.ts';
import { store } from '../src/lib/server/store.ts';
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
    const manual = { requirementId: 'EXAMPLE-001', testId: 'manual', result: 'pass', device: 'device', notes: '', evidence: [] };
    assert.equal((await request('candidates/candidate/runs', 'POST', manual, ci)).status, 403);
    assert.equal((await request('candidates/candidate/runs', 'POST', manual, owner)).status, 409);
    assert.equal((await request('candidates/candidate/report', 'GET', undefined, ci)).status, 200);
    const before = await (await request('candidates/candidate/evidence', 'GET', undefined, ci)).text();
    store().updateCandidate(candidate.id, (value) => { value.releaseUrl = 'https://example.com/release'; });
    assert.equal(await (await request('candidates/candidate/evidence', 'GET', undefined, ci)).text(), before);
  } finally { store().db.close(); rmSync(directory, { recursive: true, force: true }); }
});
test('body limits apply without trusting content-length', async () => {
  const req = new Request('https://example.com', { method: 'POST', body: '12345' });
  await assert.rejects(() => boundedBody(req, 4), /too large/);
});
