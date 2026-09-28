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
