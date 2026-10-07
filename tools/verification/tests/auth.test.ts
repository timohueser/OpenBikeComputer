import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomBytes, scryptSync } from 'node:crypto';
import type { Cookies } from '@sveltejs/kit';
import { authenticate, changePassword, createAgentToken, createSession, githubLogin, localLogin, logout, requireAdmin } from '../src/lib/server/auth.ts';
import { approveGitHubUser, removeGitHubUser } from '../src/lib/server/accounts.ts';
import { Store, store } from '../src/lib/server/store.ts';
const directory = mkdtempSync(join(tmpdir(), 'obc-verification-auth-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.VERIFICATION_OWNER_USERNAME = 'owner';
const initialPassword = 'initial-long-password';
const salt = randomBytes(16).toString('hex');
process.env.VERIFICATION_OWNER_PASSWORD_HASH = `${salt}:${scryptSync(initialPassword, salt, 64).toString('hex')}`;
function cookieJar(): Cookies {
  const values = new Map<string, string>();
  return { get: (name: string) => values.get(name), set: (name: string, value: string) => values.set(name, value), delete: (name: string) => values.delete(name) } as unknown as Cookies;
}
test('persistent local credentials rotate sessions without resetting requirements or accepting stale environment hashes', async () => {
  const cookies = cookieJar();
  const other = cookieJar();
  const request = new Request('https://verification.example');
  {
    assert.throws(() => localLogin('owner', 'wrong', 'ip'), /Invalid credentials/);
    const actor = localLogin('owner', initialPassword, 'ip');
    assert.equal(actor.admin, true); assert.equal(actor.provider, 'local');
    createSession(cookies, actor); createSession(other, actor); assert.deepEqual(authenticate(request, cookies), actor);
    const oldToken = cookies.get('obc_session')!;
    assert.equal(store().db.prepare('SELECT hash FROM sessions WHERE hash=?').get(oldToken), undefined);
    assert.throws(() => changePassword(actor, 'incorrect', 'new-long-password', cookies), /Current password/);
    assert.throws(() => changePassword(actor, initialPassword, 'short', cookies), /16 to 1024/);
    assert.throws(() => changePassword(actor, initialPassword, 'x'.repeat(1025), cookies), /16 to 1024/);
    const requirement = store().latestRevision();
    changePassword(actor, initialPassword, 'new-password-with-16-characters', cookies);
    assert.notEqual(cookies.get('obc_session'), oldToken);
    assert.equal(authenticate(request, cookies)?.admin, true); assert.equal(authenticate(request, other), undefined);
    assert.throws(() => localLogin('owner', initialPassword, 'ip'), /Invalid credentials/);
    localLogin('owner', 'new-password-with-16-characters', 'ip');
    const reopened = new Store(directory);
    assert.deepEqual(reopened.latestRevision(), requirement);
    assert.equal(reopened.localPasswordHash(), store().localPasswordHash());
    assert.notEqual(reopened.localPasswordHash(), process.env.VERIFICATION_OWNER_PASSWORD_HASH); reopened.db.close();
    logout(cookies); assert.equal(authenticate(request, cookies), undefined);
    createSession(cookies, actor); store().db.prepare('UPDATE sessions SET expires=0').run(); assert.equal(authenticate(request, cookies), undefined);
    process.env.VERIFICATION_AGENT_TOKEN = 'agent-token'; process.env.VERIFICATION_CI_TOKEN = 'ci-token';
    assert.equal(authenticate(new Request(request, { headers: { authorization: 'Bearer agent-token' } }), cookies), undefined);
    const { token } = createAgentToken(actor, 'Local agent', new Date(Date.now() + 3_600_000).toISOString());
    const agent = authenticate(new Request(request, { headers: { authorization: `Bearer ${token}` } }), cookies)!;
    assert.equal(agent.role, 'agent'); assert.throws(() => requireAdmin(agent), /administrator/);
    assert.equal(authenticate(new Request(request, { headers: { authorization: 'Bearer ci-token' } }), cookies)?.role, 'ci');
    assert.equal(authenticate(new Request(request, { headers: { authorization: 'Bearer incorrect' } }), cookies), undefined);
  }
});
test('GitHub approvals bind stable IDs, recheck privileges, revoke sessions, and reject OAuth completion after removal', async () => {
  const original = globalThis.fetch;
  const owner = localLogin('owner', 'new-password-with-16-characters', 'ip');
  const cookies = cookieJar();
  const request = new Request('https://verification.example');
  try {
    globalThis.fetch = async (url, options) => {
      assert.equal(String(url), 'https://api.github.com/users/alice');
      assert.equal((options?.headers as Record<string, string>).Authorization, undefined);
      return new Response(JSON.stringify({ id: 123, login: 'Alice', type: 'User', email: 'never-stored@example.com' }));
    };
    const user = await approveGitHubUser(owner, 'alice', false);
    assert.deepEqual(user, { id: '123', login: 'Alice', admin: false });
    assert.deepEqual(store().githubUsers(), [user]);
    const collaborator = githubLogin({ id: 123, login: 'RenamedAlice' });
    assert.equal(collaborator.name, 'RenamedAlice'); assert.equal(collaborator.role, 'owner');
    createSession(cookies, collaborator); assert.equal(authenticate(request, cookies)?.userId, '123');
    assert.throws(() => githubLogin({ id: 456, login: 'Alice' }), /not approved/);
    assert.throws(() => requireAdmin(collaborator), /administrator/);
    await assert.rejects(() => approveGitHubUser(collaborator, 'alice', true), /administrator/);
    assert.throws(() => removeGitHubUser(collaborator, '123'), /administrator/);
    store().db.prepare('UPDATE github_users SET admin=1 WHERE id=?').run('123');
    const admin = authenticate(request, cookies)!;
    assert.equal(admin.admin, true);
    assert.throws(() => removeGitHubUser(admin, '123'), /own account/);
    assert.throws(() => changePassword(admin, initialPassword, 'another-long-password', cookies), /local administrator/);
    removeGitHubUser(owner, '123');
    assert.equal(authenticate(request, cookies), undefined);
    assert.throws(() => createSession(cookieJar(), admin), /not approved/);
    await approveGitHubUser(owner, 'alice', true);
    assert.equal(authenticate(request, cookies), undefined);
    const githubAdmin = githubLogin({ id: 123, login: 'Alice' });
    globalThis.fetch = async () => {
      removeGitHubUser(owner, '123');
      return new Response(JSON.stringify({ id: 789, login: 'Bob', type: 'User' }));
    };
    await assert.rejects(() => approveGitHubUser(githubAdmin, 'bob', false), /administrator/);
    assert.equal(store().githubUser('789'), undefined);
    assert.equal(requireAdmin(owner).admin, true);
  } finally { globalThis.fetch = original; store().db.close(); rmSync(directory, { recursive: true, force: true }); }
});
