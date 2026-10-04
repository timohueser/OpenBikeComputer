import {Temporal} from '@js-temporal/polyfill';

// Hours evaluation replaces the global Date with a regional one; this module keeps the host type.
const Host = Date;

/** Local Date methods for the hours evaluator; UTC methods stay native. */
export function calendarDate(timeZone) {
  const zone = new Intl.DateTimeFormat('en', {timeZone}).resolvedOptions().timeZone;
  const instants = new Map(), locals = new Map();
  const remember = (cache, key, value) => {
    if (cache.size >= 512) cache.clear();
    cache.set(key, value);
    return value;
  };
  const instant = local => {
    if (Number.isNaN(+local)) return NaN;
    if (instants.has(+local)) return instants.get(+local);
    const value = Temporal.ZonedDateTime.from({
      timeZone: zone, year: local.getUTCFullYear(), month: local.getUTCMonth() + 1,
      day: local.getUTCDate(), hour: local.getUTCHours(), minute: local.getUTCMinutes(),
      second: local.getUTCSeconds(), millisecond: local.getUTCMilliseconds(),
    }, {disambiguation: 'compatible'}).epochMilliseconds;
    return remember(instants, +local, value);
  };
  const local = date => {
    if (Number.isNaN(+date)) return new Host(NaN);
    if (locals.has(+date)) return new Host(locals.get(+date));
    const zoned = Temporal.Instant.fromEpochMilliseconds(+date).toZonedDateTimeISO(zone);
    const value = new Host(0);
    value.setUTCFullYear(zoned.year, zoned.month - 1, zoned.day);
    value.setUTCHours(zoned.hour, zoned.minute, zoned.second, zoned.millisecond);
    return new Host(remember(locals, +date, +value));
  };
  class CalendarDate extends Host {
    static timeZone = zone;
    constructor(...args) {
      if (args.length > 1) super(instant(new Host(Host.UTC(...args))));
      else if (typeof args[0] === 'string' && /^\d{4}-\d\d-\d\d[T ]\d\d:\d\d(?::\d\d(?:\.\d+)?)?$/.test(args[0]))
        super(instant(new Host(`${args[0]}Z`)));
      else super(...args);
    }
    getTimezoneOffset() {
      return Number.isNaN(+this) ? NaN : -Math.trunc(
        Temporal.Instant.fromEpochMilliseconds(+this).toZonedDateTimeISO(zone).offsetNanoseconds / 60e9,
      );
    }
  }
  for (const name of ['FullYear', 'Month', 'Date', 'Day', 'Hours', 'Minutes', 'Seconds', 'Milliseconds']) {
    CalendarDate.prototype[`get${name}`] = function () { return local(this)[`getUTC${name}`](); };
    if (name !== 'Day') CalendarDate.prototype[`set${name}`] = function (...args) {
      const value = local(this);
      value[`setUTC${name}`](...args);
      return this.setTime(instant(value));
    };
  }
  for (const name of ['toLocaleString', 'toLocaleDateString', 'toLocaleTimeString']) {
    CalendarDate.prototype[name] = function (locale, options) {
      return Host.prototype[name].call(this, locale, {timeZone: zone, ...options});
    };
  }
  return CalendarDate;
}
