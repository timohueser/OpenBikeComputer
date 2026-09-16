export async function api<T>(path: string, method = 'GET', body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method, headers: body instanceof FormData ? undefined : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : body instanceof FormData ? body : JSON.stringify(body)
  });
  if (response.status === 401) { window.location.assign('/login'); throw new Error('Please sign in.'); }
  const value = await response.json().catch(() => ({ error: `The server could not complete this request (HTTP ${response.status}). Please try again.` }));
  if (!response.ok) throw new Error(response.status === 409 && path === '/api/requirements' ? `${value.error} Your draft has been kept. Refresh before saving again.` : value.error || 'The request failed. Please try again.');
  return value as T;
}
export function date(value: string) { return new Date(value).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }); }
export function size(value: number) { return value < 1024 ? `${value} B` : value < 1048576 ? `${(value / 1024).toFixed(1)} KB` : `${(value / 1048576).toFixed(1)} MB`; }
export const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value));
export const message = (error: unknown) => error instanceof Error ? error.message : 'An unexpected error occurred.';
