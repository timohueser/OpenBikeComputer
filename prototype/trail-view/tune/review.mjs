// The owner's review of the Europe tune, outside git in ~/.cache/obc/trail-view/review/<short>/:
// round folders <n>/ with PNG shots, and comments.jsonl with one comment per line. The dev server
// (vite.config.ts), tune/shoot.mjs and tune/comment.mjs read and write it only through here.

import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

export const REVIEW = join(homedir(), '.cache/obc/trail-view/review');
const TEST_REGIONS = [{ short: 'cascades', name: 'Cascades (outside Europe)', countries: ['United States'] }];

/** The baseline ecoregions that have a short name, then the test regions outside Europe. */
export function regions() {
  const { regions } = JSON.parse(readFileSync(join(import.meta.dirname, 'ecoregions.json'), 'utf8'));
  return [...regions.filter((r) => r.short).map(({ short, name, countries }) => ({ short, name, countries })), ...TEST_REGIONS];
}

/** Every path below REVIEW is built from a short this accepts. */
export const known = (short) => regions().some((r) => r.short === short);

/** Round number to its PNG files, by name. */
export function rounds(short) {
  const folder = join(REVIEW, short);
  if (!existsSync(folder)) return {};
  return Object.fromEntries(readdirSync(folder, { withFileTypes: true })
    .filter((e) => e.isDirectory() && /^[1-9]\d*$/.test(e.name))
    .map((e) => [e.name, readdirSync(join(folder, e.name)).filter((f) => f.endsWith('.png')).sort()]));
}

export const newest = (short) => Math.max(0, ...Object.keys(rounds(short)).map(Number));

export function comments(short) {
  const file = join(REVIEW, short, 'comments.jsonl');
  if (!existsSync(file)) return [];
  return readFileSync(file, 'utf8').split('\n').filter(Boolean).flatMap((line) => {
    try {
      return [JSON.parse(line)];
    } catch {
      return [];
    }
  });
}

/** An approval holds until a newer round: then the region is back in review. */
export function status(short) {
  const round = newest(short);
  if (!round) return 'no shots';
  return comments(short).some((c) => c.approve && c.round >= round) ? 'approved' : 'in review';
}

/** Appends one comment, stamped with the time and, unless it names one, the newest round. */
export function comment(short, { round, spot, text, by, approve }) {
  const line = { time: new Date().toISOString(), round: round ?? newest(short), spot, text, by, approve: approve || undefined };
  mkdirSync(join(REVIEW, short), { recursive: true });
  appendFileSync(join(REVIEW, short, 'comments.jsonl'), `${JSON.stringify(line)}\n`);
  return line;
}
