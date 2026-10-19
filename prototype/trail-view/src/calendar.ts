/**
 * The date of the view, as a day of the year from 0 on 1 January: the sun's day, the two
 * Sentinel-2 quarterly mosaics the ground blends, the season of the trees and plants, and the day
 * step of the snow data.
 */

import { SEASONS, type Season } from './models/trees';

/** The year of the Sentinel-2 quarterly mosaics; the sun takes its day in it. */
const YEAR = 2025;
const DAY_MS = 86_400_000;
const DAYS = 365;
/** The days the quarterly mosaics centre on: 15 February, 15 May, 15 August and 15 November. */
const CENTRES = [45, 134, 226, 318];
/** 1 September, where a snow season starts (specs/planner-snow-tiles.md). */
const SEPTEMBER = 243;

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const at = (day: number) => new Date(Date.UTC(YEAR, 0, 1) + day * DAY_MS);

/** The ISO date of the day. */
export const isoDate = (day: number) => at(day).toISOString().slice(0, 10);

/** "15 Mar". */
export const dateLabel = (day: number) => `${at(day).getUTCDate()} ${MONTHS[at(day).getUTCMonth()]}`;

/** The day of "MM-DD", or null. */
export function dayOf(text: string | null): number | null {
  const [month, date] = (text ?? '').split('-').map(Number);
  if (!(month >= 1 && month <= 12 && date >= 1 && date <= 31)) return null;
  return Math.round((Date.UTC(YEAR, month - 1, date) - Date.UTC(YEAR, 0, 1)) / DAY_MS);
}

/** "MM-DD" of the day. */
export const monthDay = (day: number) => isoDate(day).slice(5);

/** The season of the trees, plants and palette: the quarter the day falls in, 0 winter to 3 autumn. */
export const quarterOf = (day: number) => Math.floor(at(day).getUTCMonth() / 3);
export const seasonOf = (day: number): Season => SEASONS[quarterOf(day)];

/** The two mosaics around the day, 0–3, and the weight of the second, linear in days between their centres. */
export function mosaicsAt(day: number): { quarters: [number, number]; weight: number } {
  const b = Math.max(CENTRES.findIndex((centre) => centre > day), 0);
  const a = (b + 3) % 4;
  const from = CENTRES[a] - (CENTRES[a] > day ? DAYS : 0);
  const to = CENTRES[b] + (CENTRES[b] > day ? 0 : DAYS);
  return { quarters: [a, b], weight: (day - from) / (to - from) };
}

/** The day step of the snow data: two days a step from 1 September. */
export const snowStep = (day: number) => Math.floor((((day - SEPTEMBER) % DAYS) + DAYS) % DAYS / 2);
