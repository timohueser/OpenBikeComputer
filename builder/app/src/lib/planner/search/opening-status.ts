import type { HoursStatus } from './types';

export function openingStatus(value: HoursStatus, now: number) {
    const state = now >= value.checkedAt && now < value.validUntil ? value.state : 'unknown';
    const remaining = value.closesAt ? value.closesAt - now : 0;
    const closesIn = state === 'open' && remaining > 0 && remaining < 60 * 60_000 ? Math.max(1, Math.floor(remaining / 60_000)) : null;
    return { state, label: state === 'open' ? 'Open' : state === 'closed' ? 'Closed' : 'Hours unknown', closesIn };
}
