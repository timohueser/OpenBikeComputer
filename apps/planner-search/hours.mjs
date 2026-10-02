import OpeningHours from 'opening_hours';

const weekdays = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
export function openingState(place, filter, context, countryCode = 'de') {
  if (!filter) return null;
  if (!place.opening_hours) return 'unknown';
  try {
    const oh = new OpeningHours(place.opening_hours, {
      lat: place.lat,
      lon: place.lon,
      address: { country_code: countryCode, state: place.region },
    });
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
export function currentOpening(place, now = Date.now(), countryCode = 'de') {
  if (!place.opening_hours) return undefined;
  const status = { state: 'unknown' };
  try {
    const oh = new OpeningHours(place.opening_hours, {
      lat: place.lat, lon: place.lon,
      address: { country_code: countryCode, state: place.region },
    });
    const date = new Date(now);
    if (oh.getUnknown(date)) return status;
    status.state = oh.getState(date) ? 'open' : 'closed';
    const next = oh.getNextChange(date, new Date(now + 60 * 60_000));
    if (next && status.state === 'open' && !oh.getUnknown(next) && !oh.getState(next))
      status.closesAt = `${String(next.getHours()).padStart(2, '0')}:${String(next.getMinutes()).padStart(2, '0')}`;
  } catch { /* Unparseable and conditional schedules must not imply closed. */ }
  return status;
}

export function openingHours({countryCode, timeZone}) {
  if (!/^[a-z]{2}$/.test(countryCode)) throw new Error('Supply the search country code.');
  if (typeof timeZone !== 'string' || !timeZone) throw new Error('Supply the search time zone.');
  const zone = new Intl.DateTimeFormat('en', {timeZone}).resolvedOptions().timeZone;
  const assertEnvironment = () => {
    // The evaluator creates local Date values internally, including for DST and holidays.
    if ((Date.timeZone ?? new Intl.DateTimeFormat('en').resolvedOptions().timeZone) !== zone)
      throw new Error(`Opening hours require a calendar runtime in ${zone}.`);
  };
  return {countryCode,timeZone:zone,assertEnvironment,
    openingState:(place,filter,context)=>openingState(place,filter,context,countryCode),
    currentOpening:(place,now)=>currentOpening(place,now,countryCode)};
}
