import OpeningHours from 'opening_hours';

// This data service contains Germany only. opening_hours evaluates dates in the process timezone.
process.env.TZ = 'Europe/Berlin';
const weekdays = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
export function openingState(place, filter, context) {
  if (!filter) return null;
  if (!place.opening_hours) return 'unknown';
  try {
    const oh = new OpeningHours(place.opening_hours, {
      lat: place.lat,
      lon: place.lon,
      address: { country_code: 'de', state: place.region },
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
      date = new Date(`${context.openDate}T00:00:00`);
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

/** Current status is independent of a query's weekday or trip-date filter. */
export function currentOpening(place, now = Date.now()) {
  if (!place.opening_hours) return undefined;
  const status = { state: 'unknown', checkedAt: now, validUntil: now + 5 * 60_000 };
  try {
    const oh = new OpeningHours(place.opening_hours, {
      lat: place.lat, lon: place.lon,
      address: { country_code: 'de', state: place.region },
    });
    const date = new Date(now);
    if (oh.getUnknown(date)) return status;
    status.state = oh.getState(date) ? 'open' : 'closed';
    const next = oh.getNextChange(date, new Date(now + 60 * 60_000));
    if (next) {
      status.validUntil = Math.min(status.validUntil, next.valueOf());
      if (status.state === 'open' && !oh.getUnknown(next) && !oh.getState(next)) status.closesAt = next.valueOf();
    }
  } catch { /* Unparseable and conditional schedules must not imply closed. */ }
  return status;
}
