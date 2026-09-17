import type { Actor, ApprovedGitHubUser } from '../types.ts';
import { assert, text } from './domain.ts';
import { requireAdmin } from './auth.ts';
import { store } from './store.ts';

export async function approveGitHubUser(actor: Actor, login: unknown, admin: unknown): Promise<ApprovedGitHubUser> {
  requireAdmin(actor);
  const username = text(login, 'GitHub username', 39);
  assert(/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/.test(username), 'Enter a GitHub username, without @ or a URL.');
  assert(typeof admin === 'boolean', 'Administrator must be a boolean.');
  const response = await fetch(`https://api.github.com/users/${encodeURIComponent(username)}`, { headers: { Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28' }, signal: AbortSignal.timeout(15000) });
  assert(response.ok, response.status === 404 ? 'GitHub user not found.' : 'GitHub could not resolve this user. Try again.', response.status === 404 ? 404 : 502);
  const identity = await response.json();
  assert(identity.type === 'User' && typeof identity.id === 'number' && Number.isSafeInteger(identity.id) && identity.id > 0 && typeof identity.login === 'string', 'Choose a personal GitHub user account.');
  const user = { id: String(identity.id), login: identity.login, admin };
  requireAdmin(actor);
  assert(!store().githubUser(user.id), 'This GitHub account is already approved.', 409);
  store().db.prepare('INSERT INTO github_users(id,login,admin) VALUES(?,?,?)').run(user.id, user.login, user.admin ? 1 : 0);
  return user;
}
export function removeGitHubUser(actor: Actor, id: string): void {
  const current = requireAdmin(actor);
  assert(/^[1-9]\d*$/.test(id), 'Invalid GitHub user ID.');
  assert(!(current.provider === 'github' && current.userId === id), 'You cannot remove your own account.', 409);
  assert(store().githubUser(id), 'Approved user not found.', 404);
  store().atomic(() => {
    store().db.prepare('DELETE FROM github_users WHERE id=?').run(id);
    store().db.prepare("UPDATE agent_tokens SET revoked_at=COALESCE(revoked_at,?) WHERE json_extract(issuer,'$.provider')='github' AND json_extract(issuer,'$.userId')=?").run(Date.now(), id);
    store().db.prepare("DELETE FROM sessions WHERE json_extract(actor,'$.provider')='github' AND json_extract(actor,'$.userId')=?").run(id);
  });
}
