import { createHash, randomBytes, scryptSync, timingSafeEqual } from 'node:crypto';
import type { Cookies } from '@sveltejs/kit';
import type { Actor } from '../types.ts';
import { assert } from './domain.ts';
import { store } from './store.ts';

export const digest = (value: string) => createHash('sha256').update(value).digest('hex');
export const sameSecret = (a: string, b: string) => !!a && !!b && timingSafeEqual(Buffer.from(digest(a)), Buffer.from(digest(b)));
export const oauthEnabled = () => !!(process.env.GITHUB_CLIENT_ID && process.env.GITHUB_CLIENT_SECRET && process.env.VERIFICATION_OWNERS);
export function cookieOptions() { return { path: '/', httpOnly: true, sameSite: 'lax' as const, secure: process.env.NODE_ENV === 'production', maxAge: 60 * 60 * 12 }; }
export function authenticate(request: Request, cookies: Cookies): Actor | undefined {
  const authorization = request.headers.get('authorization');
  if (authorization?.startsWith('Bearer ')) {
    const token = authorization.slice(7);
    if (sameSecret(token, process.env.VERIFICATION_CI_TOKEN || '')) return { name: 'CI', role: 'ci' };
    if (sameSecret(token, process.env.VERIFICATION_AGENT_TOKEN || '')) return { name: 'Coding agent', role: 'agent' };
    return;
  }
  const token = cookies.get('obc_session');
  if (!token) return;
  const row = store().db.prepare('SELECT actor FROM sessions WHERE hash=? AND expires>?').get(digest(token), Date.now());
  if (row) return JSON.parse(String(row.actor));
}
export function createSession(cookies: Cookies, actor: Actor): void {
  const token = randomBytes(32).toString('base64url');
  const old = cookies.get('obc_session');
  if (old) store().db.prepare('DELETE FROM sessions WHERE hash=?').run(digest(old));
  store().db.prepare('DELETE FROM sessions WHERE expires<?').run(Date.now());
  store().db.prepare('INSERT INTO sessions(hash,actor,expires) VALUES(?,?,?)').run(digest(token), JSON.stringify(actor), Date.now() + 12 * 60 * 60 * 1000);
  cookies.set('obc_session', token, cookieOptions());
}
export function logout(cookies: Cookies): void {
  const token = cookies.get('obc_session');
  if (token) store().db.prepare('DELETE FROM sessions WHERE hash=?').run(digest(token));
  cookies.delete('obc_session', { path: '/' });
}
export function localLogin(username: unknown, password: unknown, key: string): Actor {
  const db = store().db;
  db.prepare('DELETE FROM login_attempts WHERE expires<?').run(Date.now());
  const rateKey = digest(key);
  const attempt = db.prepare('SELECT count FROM login_attempts WHERE key=?').get(rateKey);
  assert(Number(attempt?.count ?? 0) < 10, 'Too many login attempts. Try again in 15 minutes.', 429);
  db.prepare('INSERT INTO login_attempts(key,count,expires) VALUES(?,1,?) ON CONFLICT(key) DO UPDATE SET count=count+1').run(rateKey, Date.now() + 15 * 60 * 1000);
  assert(typeof username === 'string' && typeof password === 'string' && password.length <= 1024, 'Invalid credentials.', 401);
  const configured = process.env.VERIFICATION_OWNER_PASSWORD_HASH || '';
  const [salt, expected] = configured.split(':');
  const validConfig = /^[a-f0-9]{32}$/.test(salt ?? '') && /^[a-f0-9]{128}$/.test(expected ?? '');
  const actual = scryptSync(password, validConfig ? salt : 'unconfigured', 64).toString('hex');
  assert(validConfig && sameSecret(username, process.env.VERIFICATION_OWNER_USERNAME || '') && sameSecret(actual, expected), 'Invalid credentials.', 401);
  db.prepare('DELETE FROM login_attempts WHERE key=?').run(rateKey);
  return { name: username, role: 'owner' };
}
export function sameOrigin(request: Request): void {
  const expected = process.env.ORIGIN;
  assert(!!expected && request.headers.get('origin') === new URL(expected).origin, 'Request origin is not allowed.', 403);
}
