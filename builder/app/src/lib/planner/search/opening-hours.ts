export interface HoursRow { days: string; periods: string[] }
const dayNames: Record<string, string> = { Mo:'Mon', Tu:'Tue', We:'Wed', Th:'Thu', Fr:'Fri', Sa:'Sat', Su:'Sun', PH:'Public holidays', SH:'School holidays' };
const day = '(?:Mo|Tu|We|Th|Fr|Sa|Su|PH|SH)';
const selector = `${day}(?:-${day})?(?:,\\s*${day}(?:-${day})?)*`;
const rule = new RegExp(`^(?:(${selector})\\s+)?(off|closed|open|24/7|\\d{1,2}:\\d{2}\\s*-\\s*\\d{1,2}:\\d{2}(?:\\s*,\\s*\\d{1,2}:\\d{2}\\s*-\\s*\\d{1,2}:\\d{2})*)$`);

/** Format common OSM schedules for display only. Never infer a current state or unlisted days. */
export function openingHoursRows(raw: string): HoursRow[] | null {
    if (raw.trim() === '24/7') return [{days:'Every day', periods:['Open 24 hours']}];
    // OSM also permits a comma between complete rules, separate from a comma between time intervals.
    const rules = raw.replace(new RegExp(`(\\d|off|closed|open)\\s*,\\s*(?=${day})`, 'g'), '$1;').split(';');
    const rows: HoursRow[] = [];
    for (const input of rules) {
        const match = input.trim().match(rule);
        if (!match) return null;
        const [, days, hours] = match;
        rows.push({
            days: days ? days.replace(/Mo|Tu|We|Th|Fr|Sa|Su|PH|SH/g, token => dayNames[token]).replaceAll('-', '–').replace(/,\s*/g, ', ') : 'Every day',
            periods: ['off','closed'].includes(hours) ? ['Closed'] : hours === 'open' || hours === '24/7' ? ['Open 24 hours'] : hours.split(',').map(time => time.trim().replace(/\s*-\s*/g, '–')),
        });
    }
    return rows.length ? rows : null;
}
