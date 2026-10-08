import { after, test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Cookies, RequestEvent } from '@sveltejs/kit';
import type { Actor, AgentToken, Candidate, CoverageProposal } from '../src/lib/types.ts';
import { authenticate, createSession } from '../src/lib/server/auth.ts';
import { api } from '../src/lib/server/api.ts';
import { Store, store } from '../src/lib/server/store.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-agent-tokens-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.ORIGIN = 'https://verify.example.com';
process.env.VERIFICATION_CI_TOKEN = 'test-ci-token';
after(() => { store().db.close(); rmSync(directory, { recursive: true, force: true }); });

function session(id: string, admin: boolean): Cookies {
  store().db.prepare('INSERT INTO github_users(id,login,admin) VALUES(?,?,?)').run(id, `user-${id}`, Number(admin));
  const values = new Map<string, string>();
  const cookies = { get: (key: string) => values.get(key), set: (key: string, value: string) => values.set(key, value), delete: (key: string) => values.delete(key) } as unknown as Cookies;
  createSession(cookies, { name: `user-${id}`, role: 'owner', provider: 'github', userId: id });
  return cookies;
}
const admin = session('1', true);
const otherAdmin = session('2', true);
const maintainer = session('3', false);
const anonymous = { get: () => undefined } as unknown as Cookies;
async function request(path: string, method = 'GET', data?: unknown, credential: Cookies | string = admin, origin = process.env.ORIGIN!) {
  const cookies = typeof credential === 'string' ? anonymous : credential;
  const headers = { 'content-type': 'application/json', ...(origin ? { origin } : {}), ...(typeof credential === 'string' ? { authorization: `Bearer ${credential}` } : {}) };
  const req = new Request(`${process.env.ORIGIN}/api/${path}`, { method, headers, body: data === undefined ? undefined : JSON.stringify(data) });
  return api({ params: { path }, url: new URL(req.url), request: req, cookies, locals: { actor: authenticate(req, cookies) } } as unknown as RequestEvent);
}
const ahead = (minutes: number) => new Date(Date.now() + minutes * 60_000).toISOString();
async function issue(name: string, minutes = 60, credential: Cookies = admin) {
  const response = await request('admin/agent-tokens', 'POST', { name, expiresAt: ahead(minutes) }, credential);
  assert.equal(response.status, 201, await response.clone().text());
  assert.equal(response.headers.get('cache-control'), 'no-store');
  return await response.json() as { token: string; access: AgentToken };
}

test('only current administrators can issue, list, or revoke tokens, with origin and expiry checks', async () => {
  for (const credential of [anonymous, maintainer, 'test-ci-token']) {
    for (const [path, method, body] of [
      ['admin/agent-tokens', 'GET', undefined],
      ['admin/agent-tokens', 'POST', { name: 'Denied' }],
      ['admin/agent-tokens/missing', 'DELETE', undefined]
    ] as const) assert.equal((await request(path, method, body, credential)).status, credential === anonymous ? 401 : 403);
  }
  for (const origin of ['', 'https://untrusted.example']) {
    assert.equal((await request('admin/agent-tokens', 'POST', { name: 'Denied' }, admin, origin)).status, 403);
    assert.equal((await request('admin/agent-tokens/missing', 'DELETE', undefined, admin, origin)).status, 403);
  }
  for (const expiresAt of [undefined, null, '', 'soon', 60, '2027', '2026-12-01T10:00:00', ahead(0), ahead(-1), ahead(367 * 24 * 60)]) {
    assert.equal((await request('admin/agent-tokens', 'POST', { name: 'Invalid', expiresAt })).status, 400);
  }
  for (const name of ['', '  ', 'x'.repeat(101)]) assert.equal((await request('admin/agent-tokens', 'POST', { name })).status, 400);
  const sent = ahead(60);
  const response = await request('admin/agent-tokens', 'POST', { name: '  Laptop  ', expiresAt: sent, role: 'owner', admin: true, issuedBy: { name: 'forged' } });
  assert.equal(response.status, 201);
  const issued = await response.json();
  assert.equal(issued.access.name, 'Laptop');
  assert.equal(issued.access.issuedBy.userId, '1');
  assert.equal(issued.access.expiresAt, sent);
  const far = ahead(366 * 24 * 60);
  const max = await (await request('admin/agent-tokens', 'POST', { name: 'Maximum', expiresAt: far })).json();
  assert.equal(max.access.expiresAt, far);
});

test('tokens persist as hashes, expose metadata only after creation, and stop exactly at expiry or revocation', async t => {
  let now = Date.now();
  t.mock.method(Date, 'now', () => now);
  const first = await issue('First machine', 1);
  const second = await issue('Second machine', 1);
  assert.match(first.token, /^obc_agent_[A-Za-z0-9_-]{43}$/);
  assert.notEqual(first.token, second.token);
  const db = store().db.prepare('SELECT * FROM agent_tokens WHERE id=?').get(first.access.id)!;
  assert.match(String(db.hash), /^[a-f0-9]{64}$/);
  assert(!JSON.stringify(db).includes(first.token));
  const reopened = new Store(directory);
  assert.equal(reopened.db.prepare('SELECT hash FROM agent_tokens WHERE id=?').get(first.access.id)?.hash, db.hash);
  reopened.db.close();
  const before = await (await request('admin/agent-tokens')).json() as AgentToken[];
  assert.equal(before.find(token => token.id === first.access.id)?.lastUsedAt, undefined);
  now += 59_999;
  assert.equal((await request('bootstrap', 'GET', undefined, first.token)).status, 200);
  const listed = await (await request('admin/agent-tokens')).json() as AgentToken[];
  assert.equal(listed.find(token => token.id === first.access.id)?.lastUsedAt, new Date(now).toISOString());
  assert(!JSON.stringify(listed).includes(first.token)); assert(!JSON.stringify(listed).includes(String(db.hash)));
  assert.equal((await request(`admin/agent-tokens/${second.access.id}`, 'DELETE', undefined, otherAdmin)).status, 200);
  assert.equal((await request('bootstrap', 'GET', undefined, second.token)).status, 401);
  assert.equal((await request('bootstrap', 'GET', undefined, first.token)).status, 200);
  now += 1;
  assert.equal((await request('bootstrap', 'GET', undefined, first.token)).status, 401);
  assert.equal((await request('bootstrap', 'GET', undefined, `${first.token}bad`)).status, 401);
  assert.equal((await request(`admin/agent-tokens/${second.access.id}`, 'DELETE')).status, 200);
  assert.equal((await request('admin/agent-tokens/missing', 'DELETE')).status, 404);
});

test('agent tokens submit attributed proposals but cannot exercise owner, administrator, or CI permissions', async () => {
  const { token, access } = await issue('Coverage review');
  const revision = store().latestRevision();
  store().put('catalog', 'current', { sourceSha: 'a'.repeat(40), updatedAt: '', cases: [{ id: 'test-a', suite: 'unit', name: 'Test A' }] });
  const candidate: Candidate = { id: 'candidate', version: 'v0.1.0', sourceRef: 'develop', sourceSha: 'a'.repeat(40), revision, createdAt: '', status: 'queued', ciStatus: 'pending', results: [], manualRuns: [], assets: [] };
  store().put('candidate', candidate.id, candidate);
  for (const path of ['bootstrap', 'catalog', 'revisions', `revisions/${revision.id}`, 'candidates', 'coverage-proposals']) {
    assert.equal((await request(path, 'GET', undefined, token)).status, 200, path);
  }
  const bootstrap = await (await request('bootstrap', 'GET', undefined, token)).json();
  assert.equal((bootstrap.actor as Actor).role, 'agent'); assert.equal(bootstrap.actor.admin, undefined);
  const plan = { rationale: 'The catalogue test checks the stated behavior.', criteria: [{ id: 'behavior', statement: 'The upload keeps every point.', evidence: [{ caseId: 'test-a', rationale: 'Asserts the retained point count.' }], gap: '' }] };
  const proposed = await request('coverage-proposals', 'POST', { baseRevision: revision.id, requirementId: 'EXAMPLE-001', sourceSha: 'a'.repeat(40), plan, agentToken: { id: 'forged' }, author: 'forged' }, token, '');
  assert.equal(proposed.status, 201);
  const proposal = await proposed.json() as CoverageProposal;
  assert.equal(proposal.author, access.name);
  assert.deepEqual(proposal.agentToken, { id: access.id, name: access.name, issuedBy: access.issuedBy });
  for (const [path, method] of [
    ['admin/agent-tokens', 'GET'], ['admin/agent-tokens', 'POST'], [`admin/agent-tokens/${access.id}`, 'DELETE'],
    ['admin/history', 'POST'], ['users', 'GET'], ['account/password', 'POST'], ['requirements', 'PUT'],
    ['requirements/next-id', 'POST'], ['files', 'POST'], ['candidates', 'POST'], ['candidates/candidate/runs', 'POST'],
    ['candidates/candidate/publish', 'POST'], ['candidates/candidate/exceptions', 'POST'], ['ci/catalog', 'POST'],
    [`coverage-proposals/${proposal.id}`, 'POST']
  ]) assert.equal((await request(path, method, method === 'POST' ? { accept: true } : undefined, token)).status, 403, path);
  await request(`admin/agent-tokens/${access.id}`, 'DELETE');
  assert.equal(store().get<CoverageProposal>('coverage-proposal', proposal.id).status, 'pending');
  // An owner session decides it. Acceptance belongs to the revision save, so this is the rejection.
  assert.equal((await request(`coverage-proposals/${proposal.id}`, 'POST', { accept: false })).status, 200);
  assert.deepEqual(store().get<CoverageProposal>('coverage-proposal', proposal.id).agentToken, proposal.agentToken);
});

test('removing an issuer permanently revokes their tokens and current privileges are checked on every request', async () => {
  const removedAdmin = session('4', true);
  const first = await issue('Removed issuer', 60, removedAdmin);
  assert.equal((await request('users/4', 'DELETE')).status, 200);
  assert.equal((await request('bootstrap', 'GET', undefined, first.token)).status, 401);
  store().db.prepare('INSERT INTO github_users(id,login,admin) VALUES(?,?,1)').run('4', 'Returned admin');
  assert.equal((await request('bootstrap', 'GET', undefined, first.token)).status, 401);
  const demotedAdmin = session('5', true);
  const second = await issue('Demoted issuer', 60, demotedAdmin);
  store().db.prepare('UPDATE github_users SET admin=0 WHERE id=?').run('5');
  assert.equal((await request('bootstrap', 'GET', undefined, second.token)).status, 401);
  assert.equal((await request('admin/agent-tokens', 'POST', { name: 'Denied' }, demotedAdmin)).status, 403);
  store().db.prepare('UPDATE github_users SET admin=1 WHERE id=?').run('5');
  assert.equal((await request('bootstrap', 'GET', undefined, second.token)).status, 401);
});
