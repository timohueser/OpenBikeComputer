import { randomBytes } from 'node:crypto';
import { type RequestHandler } from '@sveltejs/kit';
import { cookieOptions, createSession, oauthEnabled, sameSecret, githubLogin } from '$lib/server/auth';
import { assert, Problem } from '$lib/server/domain';

export const GET: RequestHandler = async ({ params, cookies, url }) => {
  try {
    assert(oauthEnabled(), 'GitHub login is not configured.', 503);
    if (params.path === 'github') {
      const state = randomBytes(32).toString('base64url');
      cookies.set('obc_oauth', state, { ...cookieOptions(), maxAge: 600 });
      const target = new URL('https://github.com/login/oauth/authorize');
      target.search = new URLSearchParams({ client_id: process.env.GITHUB_CLIENT_ID!, state, redirect_uri: `${process.env.ORIGIN}/auth/callback`, scope: '' }).toString();
      return new Response(null, { status: 302, headers: { Location: target.toString() } });
    }
    assert(params.path === 'callback', 'Not found.', 404);
    const savedState = cookies.get('obc_oauth') || '';
    cookies.delete('obc_oauth', { path: '/' });
    assert(sameSecret(savedState, url.searchParams.get('state') || ''), 'Login expired or state is invalid.', 403);
    const code = url.searchParams.get('code');
    assert(code && code.length < 1000, 'Missing authorization code.');
    const response = await fetch('https://github.com/login/oauth/access_token', { method: 'POST', headers: { Accept: 'application/json', 'Content-Type': 'application/json' }, body: JSON.stringify({ client_id: process.env.GITHUB_CLIENT_ID, client_secret: process.env.GITHUB_CLIENT_SECRET, code, redirect_uri: `${process.env.ORIGIN}/auth/callback` }), signal: AbortSignal.timeout(15000) });
    const token = await response.json(); assert(response.ok && token.access_token, 'GitHub login failed.', 502);
    const identity = await fetch('https://api.github.com/user', { headers: { Authorization: `Bearer ${token.access_token}`, Accept: 'application/vnd.github+json' }, signal: AbortSignal.timeout(15000) });
    const user = await identity.json();
    assert(identity.ok, 'GitHub identity could not be read.', 502);
    createSession(cookies, githubLogin(user));
    return new Response(null, { status: 302, headers: { Location: '/' } });
  } catch (error) {
    const message = error instanceof Problem ? error.message : 'GitHub login failed. Please try again.';
    return new Response(null, { status: 302, headers: { Location: `/login?error=${encodeURIComponent(message)}` } });
  }
};
