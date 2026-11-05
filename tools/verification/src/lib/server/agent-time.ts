/** Stores the time agent sessions spend per bucket and per command, as `tools/agent_time.py` reports it. */
import type { AgentTime, AgentTimeBucket } from '../types.ts';
import { AGENT_TIME_BUCKETS } from '../types.ts';
import { assert, identifier, Problem } from './domain.ts';
import { store } from './store.ts';

const DAY_MS = 86_400_000;
/** A day row holds at most one day of time, plus slack for an interval that crosses midnight. */
const MAX_SECONDS = 90_000;
/**
 * The post needs no credential. This limit is far above the sessions one owner runs in a day, and
 * it bounds what an anonymous sender can store.
 */
const NEW_SESSIONS_PER_DAY = 500;

function seconds(value: unknown, label: string): number {
  assert(typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= MAX_SECONDS, `${label} must be between 0 and ${MAX_SECONDS} seconds.`);
  return Math.round(value * 10) / 10;
}

function day(value: unknown, now: number): string {
  assert(typeof value === 'string' && /^\d{4}-\d\d-\d\d$/.test(value), 'Day must be YYYY-MM-DD.');
  const at = Date.parse(`${value}T00:00:00Z`);
  assert(at >= now - 400 * DAY_MS && at <= now + DAY_MS, 'Day is out of range.');
  return value;
}

/** Replaces every row of the session, so the latest report of a session wins. */
export function recordAgentTime(data: Record<string, any>, now = Date.now()): void {
  const session = identifier(data.session, 'Session');
  assert(session.length <= 100, 'Session is too long.');
  assert(Array.isArray(data.days) && data.days.length >= 1 && data.days.length <= 60, 'Days must list 1 to 60 days.');
  const seen = new Set<string>();
  const rows = data.days.map((entry: any) => {
    assert(entry && typeof entry === 'object', 'Invalid day.');
    const date = day(entry.day, now);
    assert(!seen.has(date), 'Duplicate day.'); seen.add(date);
    assert(entry.seconds && typeof entry.seconds === 'object', 'Seconds are required.');
    const totals = Object.fromEntries(AGENT_TIME_BUCKETS.map(b => [b, seconds(entry.seconds[b] ?? 0, b)])) as Record<AgentTimeBucket, number>;
    assert(Object.values(totals).reduce((a, b) => a + b, 0) <= MAX_SECONDS, 'A day holds too many seconds.');
    assert(Array.isArray(entry.commands ?? []) && (entry.commands ?? []).length <= 40, 'At most 40 commands per day.');
    const commands = (entry.commands ?? []).map((c: any) => {
      assert(c && ['build', 'ci'].includes(c.kind), 'Command kind must be build or ci.');
      assert(typeof c.key === 'string' && /^[\x20-\x7e]{1,80}$/.test(c.key), 'Command key must be 1 to 80 printable characters.');
      assert(Number.isSafeInteger(c.count) && c.count >= 1 && c.count <= 100_000, 'Command count is out of range.');
      return { kind: c.kind, key: c.key, count: c.count, seconds: seconds(c.seconds, 'Command time') };
    });
    return { day: date, body: JSON.stringify({ seconds: totals, commands }) };
  });
  const db = store().db;
  store().atomic(() => {
    if (!db.prepare('SELECT 1 FROM agent_time WHERE session=? LIMIT 1').get(session)) {
      const recent = db.prepare('SELECT COUNT(DISTINCT session) AS count FROM agent_time WHERE received_at>?').get(now - DAY_MS);
      if (Number(recent?.count ?? 0) >= NEW_SESSIONS_PER_DAY) throw new Problem(429, 'Too many new sessions today.');
    }
    db.prepare('DELETE FROM agent_time WHERE session=?').run(session);
    const insert = db.prepare('INSERT INTO agent_time(session,day,received_at,body) VALUES(?,?,?,?)');
    for (const row of rows) insert.run(session, row.day, now, row.body);
  });
}

/** Totals per day and per command and day, for the last `days` days. */
export function agentTime(days: number, now = Date.now()): AgentTime {
  assert(Number.isSafeInteger(days) && days >= 1 && days <= 730, 'Days must be between 1 and 730.');
  const from = new Date(now - (days - 1) * DAY_MS).toISOString().slice(0, 10);
  const totals = new Map<string, AgentTime['days'][number]>();
  const commands = new Map<string, AgentTime['commands'][number]>();
  for (const row of store().db.prepare('SELECT day, body FROM agent_time WHERE day>=? ORDER BY day').all(from)) {
    const date = String(row.day);
    const body = JSON.parse(String(row.body));
    const total = totals.get(date) ?? { day: date, sessions: 0, seconds: { model: 0, tools: 0, build: 0, ci: 0 } };
    total.sessions += 1;
    for (const b of AGENT_TIME_BUCKETS) total.seconds[b] += body.seconds[b];
    totals.set(date, total);
    for (const c of body.commands) {
      const id = `${date}\n${c.kind}\n${c.key}`;
      const command = commands.get(id) ?? { day: date, kind: c.kind, key: c.key, count: 0, seconds: 0 };
      command.count += c.count; command.seconds += c.seconds;
      commands.set(id, command);
    }
  }
  return { from, days: [...totals.values()], commands: [...commands.values()] };
}
