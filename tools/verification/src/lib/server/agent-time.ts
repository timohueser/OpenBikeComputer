/**
 * Stores the time, tokens and tool use of agent sessions, as `tools/agent_time.py` reports them.
 * A subagent is a session of its own; `agent_sessions` holds its parent. A session without a row
 * there is a main session.
 */
import type { AgentTime } from '../types.ts';
import { AGENT_TIME_BUCKETS, AGENT_TOKEN_FIELDS } from '../types.ts';
import { assert, identifier, Problem } from './domain.ts';
import { store } from './store.ts';

const DAY_MS = 86_400_000;
/** A day holds at most one day of time, plus slack for an interval that crosses midnight. */
const MAX_SECONDS = 90_000;
const MAX_COUNT = 1e10;
/**
 * The post needs no credential. This limit is far above the sessions one owner runs in a day, and
 * it bounds the sessions an anonymous sender can start. A subagent of a stored session does not
 * count: one workflow starts hundreds of subagents, and the hook loses a refused report without an
 * error. Its parent passed this limit first.
 */
const NEW_SESSIONS_PER_DAY = 500;
const SUBAGENT = `(kind IS 'subagent')`;
const TOTALS = [`COUNT(DISTINCT CASE WHEN NOT ${SUBAGENT} THEN session END) AS sessions`, `COUNT(DISTINCT CASE WHEN ${SUBAGENT} THEN session END) AS subagents`,
  `SUM(CASE WHEN ${SUBAGENT} THEN ${AGENT_TIME_BUCKETS.join('+')} END) AS subagent`, ...[...AGENT_TIME_BUCKETS, ...AGENT_TOKEN_FIELDS].map(f => `SUM(${f}) AS ${f}`)].join(', ');
const DAYS = 'agent_days LEFT JOIN agent_sessions USING(session)';

function seconds(value: unknown, label: string): number {
  assert(typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= MAX_SECONDS, `${label} must be between 0 and ${MAX_SECONDS} seconds.`);
  return Math.round(value * 10) / 10;
}
function count(value: unknown, label: string, max = MAX_COUNT): number {
  assert(Number.isSafeInteger(value) && (value as number) >= 0 && (value as number) <= max, `${label} must be a whole number between 0 and ${max}.`);
  return value as number;
}
function label(value: unknown, name: string, max: number): string {
  assert(value === undefined || (typeof value === 'string' && [...value].length <= max && !/[\x00-\x1f\x7f]/.test(value)), `${name} must be at most ${max} characters on one line.`);
  return (value as string | undefined) ?? '';
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
  const kind = data.kind ?? 'main';
  assert(kind === 'main' || kind === 'subagent', 'Kind must be main or subagent.');
  const parent = kind === 'subagent' ? identifier(data.parent, 'Parent') : null;
  assert(kind === 'subagent' || data.parent === undefined, 'A main session has no parent.');
  const agentType = label(data.agent_type, 'Agent type', 80);
  const description = label(data.description, 'Description', 100);
  assert(Array.isArray(data.days) && data.days.length >= 1 && data.days.length <= 60, 'Days must list 1 to 60 days.');
  assert(Array.isArray(data.tools) && data.tools.length <= 3000, 'Tools must list at most 3000 rows.');
  const days = new Map<string, unknown[]>();
  for (const entry of data.days) {
    assert(entry && typeof entry === 'object', 'Invalid day.');
    const date = day(entry.day, now);
    assert(!days.has(date), 'Duplicate day.');
    const time = AGENT_TIME_BUCKETS.map(b => seconds(entry[b], b));
    assert(time.reduce((a, b) => a + b, 0) <= MAX_SECONDS, 'A day holds too many seconds.');
    days.set(date, [...time, ...AGENT_TOKEN_FIELDS.map(t => count(entry[t], t))]);
  }
  const keys = new Set<string>();
  const tools = data.tools.map((t: any) => {
    assert(t && typeof t === 'object' && days.has(t.day), 'A tool row needs a day of the report.');
    assert(['build', 'ci', 'tool'].includes(t.kind), 'Tool kind must be build, ci or tool.');
    assert(typeof t.key === 'string' && /^[\x20-\x7e]{1,80}$/.test(t.key), 'Tool key must be 1 to 80 printable characters.');
    const id = `${t.day}\n${t.kind}\n${t.key}`;
    assert(!keys.has(id), 'Duplicate tool row.'); keys.add(id);
    const calls = count(t.calls, 'Calls', 100_000);
    assert(calls >= 1, 'Calls must be at least 1.');
    return [t.day, t.kind, t.key, calls, seconds(t.seconds, 'Tool time'), count(t.failed, 'Failed', calls), count(t.chars, 'Characters')];
  });
  const db = store().db;
  store().atomic(() => {
    const stored = (id: string | null) => !!db.prepare('SELECT 1 FROM agent_days WHERE session=? LIMIT 1').get(id);
    if (!stored(session) && !stored(parent)) {
      const recent = db.prepare(`SELECT COUNT(DISTINCT session) AS count FROM ${DAYS}
        WHERE received_at>? AND NOT (${SUBAGENT} AND parent IN (SELECT session FROM agent_days))`).get(now - DAY_MS);
      if (Number(recent?.count ?? 0) >= NEW_SESSIONS_PER_DAY) throw new Problem(429, 'Too many new sessions today.');
    }
    db.prepare('DELETE FROM agent_days WHERE session=?').run(session);
    db.prepare('DELETE FROM agent_tools WHERE session=?').run(session);
    db.prepare('INSERT OR REPLACE INTO agent_sessions(session,kind,parent,agent_type,description) VALUES(?,?,?,?,?)').run(session, kind, parent, agentType, description);
    const insertDay = db.prepare(`INSERT INTO agent_days(session,day,received_at,${[...AGENT_TIME_BUCKETS, ...AGENT_TOKEN_FIELDS]}) VALUES(?,?,?,${'?,'.repeat(7)}?)`);
    for (const [date, values] of days) insertDay.run(session, date, now, ...(values as number[]));
    const insertTool = db.prepare('INSERT INTO agent_tools(session,day,kind,key,calls,seconds,failed,chars) VALUES(?,?,?,?,?,?,?,?)');
    for (const row of tools) insertTool.run(session, ...row);
  });
}

/**
 * The last `days` days, grouped by day, or by week from Monday for more than 31 days. A tool's
 * `repeated` counts the sessions that called it 3 or more times in the period.
 */
export function agentTime(days: number, now = Date.now()): AgentTime {
  assert(Number.isSafeInteger(days) && days >= 1 && days <= 730, 'Days must be between 1 and 730.');
  const iso = (at: number) => new Date(at).toISOString().slice(0, 10);
  const from = iso(now - (days - 1) * DAY_MS);
  const weekly = days > 31;
  const start = weekly ? `date(day, '-' || ((CAST(strftime('%w', day) AS INTEGER) + 6) % 7) || ' days')` : 'day';
  const db = store().db;
  const numbers = <T,>(row: Record<string, unknown>) => Object.fromEntries(Object.entries(row).map(([k, v]) => [k, typeof v === 'string' ? v : Number(v ?? 0)])) as T;
  const totals = (first: string, last: string) => numbers<AgentTime['recent']['now']>(db.prepare(`SELECT ${TOTALS},
    (SELECT SUM(chars) FROM agent_tools WHERE day>=?1 AND day<=?2) AS chars FROM ${DAYS} WHERE day>=?1 AND day<=?2`).get(first, last) ?? {});
  return {
    from, weekly,
    recent: { now: totals(iso(now - 6 * DAY_MS), iso(now)), before: totals(iso(now - 13 * DAY_MS), iso(now - 7 * DAY_MS)) },
    periods: db.prepare(`SELECT ${start} AS start, ${TOTALS} FROM ${DAYS} WHERE day>=? GROUP BY 1 ORDER BY 1`).all(from).map(r => numbers(r)),
    tools: db.prepare(`SELECT start, kind, key, SUM(calls) AS calls, SUM(seconds) AS seconds, SUM(failed) AS failed, SUM(chars) AS chars,
        COUNT(*) AS sessions, SUM(calls >= 3) AS repeated
      FROM (SELECT ${start} AS start, session, kind, key, SUM(calls) AS calls, SUM(seconds) AS seconds, SUM(failed) AS failed, SUM(chars) AS chars
        FROM agent_tools WHERE day>=? GROUP BY 1, 2, 3, 4)
      GROUP BY 1, 2, 3 ORDER BY 1`).all(from).map(r => numbers(r))
  };
}
