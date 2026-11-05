import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RequestEvent } from '@sveltejs/kit';
import type { Actor, AgentTime } from '../src/lib/types.ts';
import { api } from '../src/lib/server/api.ts';
import { recordAgentTime } from '../src/lib/server/agent-time.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-verification-agent-time-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.ORIGIN = 'https://verify.example.com';
function request(path: string, method = 'GET', data?: unknown, actor?: Actor) {
  const url = `${process.env.ORIGIN}/api/${path}`;
  return api({ params: { path: path.split('?')[0] }, url: new URL(url), request: new Request(url, { method, headers: { 'content-type': 'application/json' }, body: data === undefined ? undefined : JSON.stringify(data) }), locals: { actor }, cookies: { get: () => undefined }, getClientAddress: () => '127.0.0.1' } as unknown as RequestEvent);
}
const today = new Date().toISOString().slice(0, 10);
const report = (session: string, build: number) => ({ session, days: [{ day: today, seconds: { model: 60, tools: 10, build, ci: 5 },
  commands: [{ kind: 'build', key: 'obc test -p planner', count: 2, seconds: build }] }] });

test('agent sessions post time without a credential, and only signed-in people read it', async () => {
  try {
    assert.equal((await request('agent-time', 'POST', report('a', 100))).status, 200);
    // A later report of the same session replaces the earlier one.
    assert.equal((await request('agent-time', 'POST', report('a', 200))).status, 200);
    assert.equal((await request('agent-time', 'POST', report('b', 50))).status, 200);
    for (const bad of [{ ...report('c', 1), days: [] }, report('c', -1), report('c', 1e6), { ...report('c', 1), session: '../x' },
      { session: 'c', days: [{ ...report('c', 1).days[0], day: '2001-01-01' }] }]) {
      assert.equal((await request('agent-time', 'POST', bad)).status, 400);
    }

    assert.equal((await request('agent-time')).status, 401);
    assert.equal((await request('agent-time', 'GET', undefined, { name: 'agent', role: 'agent' })).status, 403);
    const response = await request('agent-time?days=30', 'GET', undefined, { name: 'owner', role: 'owner' });
    const result = await response.json() as AgentTime;
    assert.deepEqual(result.days, [{ day: today, sessions: 2, seconds: { model: 120, tools: 20, build: 250, ci: 10 } }]);
    assert.deepEqual(result.commands, [{ day: today, kind: 'build', key: 'obc test -p planner', count: 4, seconds: 250 }]);

    for (let i = 0; i < 498; i++) recordAgentTime(report(`s${i}`, 1));
    assert.throws(() => recordAgentTime(report('one-too-many', 1)), { status: 429 });
    recordAgentTime(report('a', 1));
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
