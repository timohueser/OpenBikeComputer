import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomBytes, scryptSync } from 'node:crypto';
import type { Cookies } from '@sveltejs/kit';
import { authenticate, createSession, localLogin, logout } from '../src/lib/server/auth.ts';
import { store } from '../src/lib/server/store.ts';
const directory = mkdtempSync(join(tmpdir(), 'obc-verification-auth-'));
process.env.VERIFICATION_DATA_DIR = directory;
test('credentials and opaque sessions fail closed, expire, and separate machine roles', () => {
  const values = new Map<string, string>();
  const cookies = { get: (name: string) => values.get(name), set: (name: string, value: string) => values.set(name, value), delete: (name: string) => values.delete(name) } as unknown as Cookies;
  const request = new Request('https://verification.example');
  try {
    assert.throws(() => localLogin('owner', 'anything', 'ip'), /Invalid credentials/);
    const salt = randomBytes(16).toString('hex');
    process.env.VERIFICATION_OWNER_USERNAME = 'owner'; process.env.VERIFICATION_OWNER_PASSWORD_HASH = `${salt}:${scryptSync('password', salt, 64).toString('hex')}`;
    const actor = localLogin('owner', 'password', 'ip');
    createSession(cookies, actor); assert.deepEqual(authenticate(request, cookies), actor);
    const token = cookies.get('obc_session')!;
    assert.equal(store().db.prepare('SELECT hash FROM sessions WHERE hash=?').get(token), undefined);
    logout(cookies); assert.equal(authenticate(request, cookies), undefined);
    createSession(cookies, actor); store().db.prepare('UPDATE sessions SET expires=0').run(); assert.equal(authenticate(request, cookies), undefined);
    process.env.VERIFICATION_AGENT_TOKEN = 'agent-token'; process.env.VERIFICATION_CI_TOKEN = 'ci-token';
    assert.equal(authenticate(new Request(request, { headers: { authorization: 'Bearer agent-token' } }), cookies)?.role, 'agent');
    assert.equal(authenticate(new Request(request, { headers: { authorization: 'Bearer ci-token' } }), cookies)?.role, 'ci');
    assert.equal(authenticate(new Request(request, { headers: { authorization: 'Bearer incorrect' } }), cookies), undefined);
  } finally { store().db.close(); rmSync(directory, { recursive: true, force: true }); }
});
