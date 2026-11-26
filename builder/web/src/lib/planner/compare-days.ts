import { duration, type Day, type PlanView, type Trip } from './editor';
import { addDays, weekdayLabel } from './layers/data-layer';
import { sleepIcon } from './search/presentation';
import { proposalTitle, type Proposal } from './stops';

/** Where a day of the compare screen ends: a place to sleep, or the finish, with its icon. */
export interface Stay { label: string; icon: string }

/** One day of a version on the compare screen: it runs `from` → `to`. */
export interface CompareDay { day: Day; stay: Stay; date?: string; from: string; to: string }

/** The days of a version along its view, with the suggested nights that the view already includes. */
export function compareDays(trip: Trip, view: PlanView, proposals: Map<number, Proposal>): CompareDay[] {
    const { days, stops } = view;
    const night = (number: number): Stay => {
        if (number === days.length) return { label: stops.at(-1)!.point.label, icon: 'flag' };
        const pinned = days[number - 1].pinned, proposal = proposals.get(number);
        return pinned ? { label: pinned.label, icon: sleepIcon(pinned.placeKind) }
            : proposal ? { label: proposalTitle(proposal), icon: sleepIcon(proposal.place?.place.placeKind) }
            : { label: `Night ${number}`, icon: 'moon' };
    };
    return days.map((day, i): CompareDay => {
        const stay = night(day.number), date = trip.startDate ? weekdayLabel(addDays(trip.startDate, i), true) : undefined;
        return { day, stay, date, from: i ? night(i).label : stops[0].point.label, to: stay.label };
    });
}

/** One line about a version: "6 days · 3h 10m to 5h 40m of riding a day". */
export function compareSummary(days: CompareDay[]): string {
    const hours = days.map(d => d.day.hours);
    const range = hours.length ? `${duration(Math.min(...hours))} to ${duration(Math.max(...hours))} of riding a day` : '';
    return [`${days.length} ${days.length === 1 ? 'day' : 'days'}`, range].filter(Boolean).join(' · ');
}
