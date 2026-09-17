import { createHash, randomBytes, scryptSync, timingSafeEqual } from 'node:crypto';
import type { Cookies } from '@sveltejs/kit';
import type { SQLOutputValue } from 'node:sqlite';
import type { Actor, AgentToken } from '../types.ts';
import { assert, text } from './domain.ts';
import { store } from './store.ts';

export const digest = (value: string) => createHash('sha256').update(value).digest('hex');
export const sameSecret = (a: string, b: string) => !!a && !!b && timingSafeEqual(Buffer.from(digest(a)), Buffer.from(digest(b)));
export const oauthEnabled = () => !!(process.env.GITHUB_CLIENT_ID && process.env.GITHUB_CLIENT_SECRET);
export function cookieOptions() { return { path: '/', httpOnly: true, sameSite: 'lax' as const, secure: process.env.NODE_ENV === 'production', maxAge: 60 * 60 * 12 }; }
export function currentActor(actor: Actor): Actor | undefined {
  if (actor.role !== 'owner') return;
  if (actor.provider === 'local' && store().localPasswordHash() && process.env.VERIFICATION_OWNER_USERNAME) {
    return { name: process.env.VERIFICATION_OWNER_USERNAME, role: 'owner', provider: 'local', userId: 'local', admin: true };
  }
  if (actor.provider === 'github' && actor.userId) {
    const user = store().githubUser(actor.userId);
    if (user) return { name: user.login, role: 'owner', provider: 'github', userId: user.id, admin: user.admin };
  }
}
export function requireAdmin(actor: Actor): Actor {
  const current = currentActor(actor);
  assert(current?.admin, 'Only an administrator can manage accounts.', 403);
  return current;
}
export function authenticate(request: Request, cookies: Cookies): Actor | undefined {
  const authorization = request.headers.get('authorization');
  if (authorization?.startsWith('Bearer ')) {
    const token = authorization.slice(7);
    if (sameSecret(token, process.env.VERIFICATION_CI_TOKEN || '')) return { name: 'CI', role: 'ci' };
    const row = store().db.prepare('SELECT * FROM agent_tokens WHERE hash=? AND expires>? AND revoked_at IS NULL').get(digest(token), Date.now());
    if (row) {
      const access = agentTokenDetails(row);
      if (!currentActor({ ...access.issuedBy, role: 'owner' })?.admin) {
        store().db.prepare('UPDATE agent_tokens SET revoked_at=? WHERE id=?').run(Date.now(), access.id);
        return;
      }
      store().db.prepare('UPDATE agent_tokens SET last_used=? WHERE id=?').run(Date.now(), access.id);
      return { name: access.name, role: 'agent', agentToken: { id: access.id, name: access.name, issuedBy: access.issuedBy } };
    }
    return;
  }
  const token = cookies.get('obc_session');
  if (!token) return;
  const row = store().db.prepare('SELECT actor FROM sessions WHERE hash=? AND expires>?').get(digest(token), Date.now());
  if (row) return currentActor(JSON.parse(String(row.actor)));
}
export function createSession(cookies: Cookies, actor: Actor): void {
  const current = currentActor(actor);
  assert(current, 'This account is not approved.', 403);
  const token = randomBytes(32).toString('base64url');
  const old = cookies.get('obc_session');
  if (old) store().db.prepare('DELETE FROM sessions WHERE hash=?').run(digest(old));
  store().db.prepare('DELETE FROM sessions WHERE expires<?').run(Date.now());
  store().db.prepare('INSERT INTO sessions(hash,actor,expires) VALUES(?,?,?)').run(digest(token), JSON.stringify(current), Date.now() + 12 * 60 * 60 * 1000);
  cookies.set('obc_session', token, cookieOptions());
}
export function logout(cookies: Cookies): void {
  const token = cookies.get('obc_session');
  if (token) store().db.prepare('DELETE FROM sessions WHERE hash=?').run(digest(token));
  cookies.delete('obc_session', { path: '/' });
}
function agentTokenDetails(row: Record<string, SQLOutputValue>): AgentToken {
  return { id: String(row.id), name: String(row.name), issuedBy: JSON.parse(String(row.issuer)),
    createdAt: new Date(Number(row.created_at)).toISOString(), expiresAt: new Date(Number(row.expires)).toISOString(),
    ...(row.last_used === null ? {} : { lastUsedAt: new Date(Number(row.last_used)).toISOString() }),
    ...(row.revoked_at === null ? {} : { revokedAt: new Date(Number(row.revoked_at)).toISOString() }) };
}
export function agentTokens(actor: Actor): AgentToken[] {
  requireAdmin(actor);
  return store().db.prepare('SELECT id,name,issuer,created_at,expires,last_used,revoked_at FROM agent_tokens ORDER BY created_at DESC, rowid DESC').all().map(agentTokenDetails);
}
export function createAgentToken(actor: Actor, name: unknown, lifetimeMinutes: unknown = 60): { token: string; access: AgentToken } {
  const admin = requireAdmin(actor);
  const label = text(name, 'Token name', 100);
  assert(typeof lifetimeMinutes === 'number' && Number.isInteger(lifetimeMinutes) && lifetimeMinutes >= 1 && lifetimeMinutes <= 240, 'Token lifetime must be between 1 and 240 minutes.');
  const token = `obc_agent_${randomBytes(32).toString('base64url')}`;
  const now = Date.now();
  const expires = now + lifetimeMinutes * 60_000;
  const access: AgentToken = { id: store().id(), name: label, issuedBy: { name: admin.name, provider: admin.provider, userId: admin.userId }, createdAt: new Date(now).toISOString(), expiresAt: new Date(expires).toISOString() };
  store().db.prepare('INSERT INTO agent_tokens(id,hash,name,issuer,created_at,expires) VALUES(?,?,?,?,?,?)')
    .run(access.id, digest(token), label, JSON.stringify(access.issuedBy), now, expires);
  return { token, access };
}
export function revokeAgentToken(actor: Actor, id: string): void {
  requireAdmin(actor);
  const result = store().db.prepare('UPDATE agent_tokens SET revoked_at=COALESCE(revoked_at,?) WHERE id=?').run(Date.now(), id);
  assert(result.changes, 'Agent token not found.', 404);
}
function passwordMatches(password: string, hash: string): boolean {
  const [salt, expected] = hash.split(':');
  const valid = /^[a-f0-9]{32}$/.test(salt ?? '') && /^[a-f0-9]{128}$/.test(expected ?? '');
  const actual = scryptSync(password, valid ? salt : 'unconfigured', 64).toString('hex');
  return valid && sameSecret(actual, expected);
}
function loginAttempt(key: string): string {
  const db = store().db;
  db.prepare('DELETE FROM login_attempts WHERE expires<?').run(Date.now());
  const rateKey = digest(key);
  const attempt = db.prepare('SELECT count FROM login_attempts WHERE key=?').get(rateKey);
  assert(Number(attempt?.count ?? 0) < 10, 'Too many login attempts. Try again in 15 minutes.', 429);
  db.prepare('INSERT INTO login_attempts(key,count,expires) VALUES(?,1,?) ON CONFLICT(key) DO UPDATE SET count=count+1').run(rateKey, Date.now() + 15 * 60 * 1000);
  return rateKey;
}
export function localLogin(username: unknown, password: unknown, key: string): Actor {
  const rateKey = loginAttempt(key);
  assert(typeof username === 'string' && typeof password === 'string' && password.length <= 1024, 'Invalid credentials.', 401);
  const valid = passwordMatches(password, store().localPasswordHash());
  assert(valid && sameSecret(username, process.env.VERIFICATION_OWNER_USERNAME || ''), 'Invalid credentials.', 401);
  store().db.prepare('DELETE FROM login_attempts WHERE key=?').run(rateKey);
  return { name: username, role: 'owner', provider: 'local', userId: 'local', admin: true };
}
export function changePassword(actor: Actor, currentPassword: unknown, newPassword: unknown, cookies: Cookies): void {
  const current = requireAdmin(actor);
  assert(current.provider === 'local', 'Sign in as the local administrator to change its password.', 403);
  const rateKey = loginAttempt('password-change:local');
  assert(typeof currentPassword === 'string' && currentPassword.length <= 1024 && passwordMatches(currentPassword, store().localPasswordHash()), 'Current password is incorrect.', 403);
  assert(typeof newPassword === 'string' && newPassword.length >= 16 && newPassword.length <= 1024, 'New password must contain 16 to 1024 characters.');
  const salt = randomBytes(16).toString('hex');
  const hash = `${salt}:${scryptSync(newPassword, salt, 64).toString('hex')}`;
  store().atomic(() => {
    store().db.prepare('UPDATE local_admin SET password_hash=? WHERE id=1').run(hash);
    store().db.prepare("DELETE FROM sessions WHERE json_extract(actor,'$.provider')='local' OR json_extract(actor,'$.provider') IS NULL").run();
    store().db.prepare('DELETE FROM login_attempts WHERE key=?').run(rateKey);
    createSession(cookies, current);
  });
}
export function githubLogin(user: { id: unknown; login: unknown }): Actor {
  assert(typeof user.id === 'number' && Number.isSafeInteger(user.id) && user.id > 0 && typeof user.login === 'string', 'Invalid GitHub identity.', 403);
  const current = store().githubUser(String(user.id));
  assert(current, 'This GitHub account is not approved.', 403);
  store().db.prepare('UPDATE github_users SET login=? WHERE id=?').run(user.login, current.id);
  return { name: user.login, role: 'owner', provider: 'github', userId: current.id, admin: current.admin };
}
export function sameOrigin(request: Request): void {
  const expected = process.env.ORIGIN;
  assert(!!expected && request.headers.get('origin') === new URL(expected).origin, 'Request origin is not allowed.', 403);
}
