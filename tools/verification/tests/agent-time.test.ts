import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RequestEvent } from '@sveltejs/kit';
import type { Actor, AgentTime } from '../src/lib/types.ts';
import { api } from '../src/lib/server/api.ts';
import { agentTime, recordAgentTime } from '../src/lib/server/agent-time.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-verification-agent-time-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.ORIGIN = 'https://verify.example.com';
function request(path: string, method = 'GET', data?: unknown, actor?: Actor) {
  const url = `${process.env.ORIGIN}/api/${path}`;
  return api({ params: { path: path.split('?')[0] }, url: new URL(url), request: new Request(url, { method, headers: { 'content-type': 'application/json' }, body: data === undefined ? undefined : JSON.stringify(data) }), locals: { actor }, cookies: { get: () => undefined }, getClientAddress: () => '127.0.0.1' } as unknown as RequestEvent);
}
const today = new Date().toISOString().slice(0, 10);
const report = (session: string, build: number, calls = 2) => ({
  session,
  days: [{ day: today, model: 60, tools: 10, build, ci: 5, input: 10, output: 200, cache_read: 5000, cache_write: 300 }],
  tools: [{ day: today, kind: 'build', key: 'obc test -p planner', calls, seconds: build, failed: 1, chars: 4000 }]
});

test('agent sessions post without a credential, and only signed-in people read the totals', async () => {
  try {
    assert.equal((await request('agent-time', 'POST', report('a', 100))).status, 200);
    // A later report of the same session replaces the earlier one.
    assert.equal((await request('agent-time', 'POST', report('a', 200, 3))).status, 200);
    assert.equal((await request('agent-time', 'POST', report('b', 50))).status, 200);
    const sub = { ...report('a1', 30), kind: 'subagent', parent: 'a', agent_type: 'Explore', description: 'Find the bug' };
    assert.equal((await request('agent-time', 'POST', sub)).status, 200);
    const good = report('c', 1);
    for (const bad of [{ ...good, days: [] }, report('c', -1), report('c', 1e6), { ...good, session: '../x' },
      { ...good, days: [{ ...good.days[0], day: '2001-01-01' }] }, { ...good, days: [{ ...good.days[0], output: 1.5 }] },
      { ...good, tools: [{ ...good.tools[0], failed: 9 }] }, { ...good, tools: [{ ...good.tools[0], day: '2026-01-01' }] },
      { ...good, kind: 'subagent' }, { ...good, parent: 'a' }, { ...sub, kind: 'other' }, { ...sub, description: 'a\nb' }]) {
      assert.equal((await request('agent-time', 'POST', bad)).status, 400);
    }

    assert.equal((await request('agent-time')).status, 401);
    assert.equal((await request('agent-time', 'GET', undefined, { name: 'agent', role: 'agent' })).status, 403);
    const result = await (await request('agent-time?days=7', 'GET', undefined, { name: 'owner', role: 'owner' })).json() as AgentTime;
    // The subagent is counted once, in its own row, and not as a session.
    const totals = { sessions: 2, subagents: 1, subagent: 105, model: 180, tools: 30, build: 280, ci: 15, input: 30, output: 600, cache_read: 15000, cache_write: 900 };
    assert.deepEqual(result.periods, [{ start: today, ...totals }]);
    assert.deepEqual(result.recent.now, { ...totals, chars: 12000 });
    assert.deepEqual(result.tools, [{ start: today, kind: 'build', key: 'obc test -p planner', calls: 7, seconds: 280, failed: 3, chars: 12000, sessions: 3, repeated: 1 }]);
    assert.equal(agentTime(90).weekly, true);

    for (let i = 0; i < 497; i++) recordAgentTime(report(`s${i}`, 1));
    assert.throws(() => recordAgentTime(report('one-too-many', 1)), { status: 429 });
    recordAgentTime(report('a', 1));
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
