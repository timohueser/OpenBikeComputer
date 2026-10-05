import OpeningHours from 'opening_hours';
import {calendarDate} from './calendar-date.mjs';

const weekdays = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
// The library selects holidays by country and state, and reads the position for sun times only as strings.
const schedule = (place) => new OpeningHours(place.opening_hours, {
  lat: String(place.lat), lon: String(place.lon),
  address: { country_code: place.country, state: place.region },
});

export function openingState(place, filter, context) {
  if (!filter) return null;
  if (!place.opening_hours) return 'unknown';
  try {
    const oh = schedule(place);
    if (filter.now) {
      const date = new Date(context.now || Date.now());
      return oh.getUnknown(date)
        ? 'unknown'
        : oh.getState(date)
          ? 'open'
          : 'closed';
    }
    let date;
    if (filter.weekday) {
      // A weekday query has no date. Date- or holiday-dependent rules cannot prove it.
      if (
        /\b(?:PH|SH|Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec|week)\b|\d{4}|\[[^\]]+\]/i.test(
          place.opening_hours,
        )
      )
        return 'unknown';
      date = new Date(2024, 0, 7 + weekdays.indexOf(filter.weekday));
    } else {
      const [year, month, day] = context.openDate.split('-').map(Number);
      date = new Date(0);
      date.setFullYear(year, month - 1, day);
      date.setHours(0, 0, 0, 0);
      if (!Number.isFinite(date.valueOf())) return 'unknown';
    }
    const end = new Date(date);
    end.setDate(end.getDate() + 1);
    const intervals = oh.getOpenIntervals(date, end);
    if (intervals.some((i) => !i[2])) return 'open';
    return intervals.length ? 'unknown' : 'closed';
  } catch {
    return 'unknown';
  }
}

/** The status at search time, independent of a query's weekday or trip-date filter.
 *  `closesAt` is the local clock time of a closure within the next hour. */
export function currentOpening(place, now = Date.now()) {
  if (!place.opening_hours) return undefined;
  const status = { state: 'unknown' };
  try {
    const oh = schedule(place);
    const date = new Date(now);
    if (oh.getUnknown(date)) return status;
    status.state = oh.getState(date) ? 'open' : 'closed';
    const next = oh.getNextChange(date, new Date(now + 60 * 60_000));
    if (next && status.state === 'open' && !oh.getUnknown(next) && !oh.getState(next))
      status.closesAt = `${String(next.getHours()).padStart(2, '0')}:${String(next.getMinutes()).padStart(2, '0')}`;
  } catch { /* Unparseable and conditional schedules must not imply closed. */ }
  return status;
}

/** Opening hours in the region's time zone, whatever the host's zone. */
export function openingHours(timeZone) {
  if (typeof timeZone !== 'string' || !timeZone) throw new Error('Supply the search time zone.');
  const Regional = calendarDate(timeZone);
  // The evaluator and its library read local fields through the global Date. Evaluation is
  // synchronous, so the regional constructor replaces the global one only during a call.
  // The library also logs each holiday gap, such as PH in Liechtenstein; the result is unknown.
  const regional = (evaluate) => (...args) => {
    const host = globalThis.Date, log = console.error;
    globalThis.Date = Regional;
    console.error = () => {};
    try { return evaluate(...args); } finally { globalThis.Date = host; console.error = log; }
  };
  return { timeZone: Regional.timeZone, openingState: regional(openingState), currentOpening: regional(currentOpening) };
}
