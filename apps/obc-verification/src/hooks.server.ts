import type { Handle } from '@sveltejs/kit';
import { authenticate } from '$lib/server/auth';
export const handle: Handle = async ({ event, resolve }) => {
  event.locals.actor = authenticate(event.request, event.cookies);
  const response = await resolve(event);
  response.headers.set('X-Content-Type-Options', 'nosniff');
  response.headers.set('Referrer-Policy', 'same-origin');
  response.headers.set('X-Frame-Options', 'DENY');
  response.headers.set('Cache-Control', 'no-store');
  return response;
};
