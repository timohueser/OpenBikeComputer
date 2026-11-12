import type { Bike } from '../library/records';
import { plannedSeconds } from '../settings/format';
import { duration, type ItineraryDay, type PlanView, type Trip } from './editor';
import { addDays, weekdayLabel } from './layers/data-layer';
import { sleepIcon } from './search/presentation';
import { proposalTitle, type Proposal } from './stops';

/** Where a day of the compare screen ends: a place to sleep, or the finish, with its icon. */
export interface Stay { label: string; icon: string }

/** One calendar day of a version on the compare screen. A riding day runs `from` → `to` and names each train or bus on
 * the way; a rest day stays at the night before it. */
export type CompareDay = { day: ItineraryDay; stay: Stay; date?: string } & (
    { rest: false; from: string; to: string; trains: string[] } | { rest: true; name: string });

/** A train or bus: the route progress where it leaves and arrives, and "From → To". */
export interface Transfer { from: number; to: number; text: string }

/** The calendar days of a version along its view, with the suggested nights that the view already includes, and its trains
 * and buses. */
export function compareDays(trip: Trip, view: PlanView, proposals: Map<number, Proposal>): { days: CompareDay[]; transfers: Transfer[] } {
    const { days, itinerary, stops, total } = view;
    const calendar = Object.fromEntries(itinerary.filter(d => !d.rest).map(d => [d.ridingNumber, d.number]));
    const night = (riding: number): Stay => {
        if (riding === days.length) return { label: stops.at(-1)!.point.label, icon: 'flag' };
        const pinned = days[riding - 1].pinned, proposal = proposals.get(riding);
        return pinned ? { label: pinned.label, icon: sleepIcon(pinned.placeKind) }
            : proposal ? { label: proposalTitle(proposal), icon: sleepIcon(proposal.place?.place.placeKind) }
            : { label: `Night ${calendar[riding]}`, icon: 'moon' };
    };
    const length = stops.at(-1)?.distance || total || 1;
    const transfers: Transfer[] = stops.slice(1).flatMap((stop, i) => stop.point.leg === 'transfer'
        ? [{ from: stops[i].distance / length, to: stop.distance / length, text: `${stops[i].point.label} → ${stop.point.label}` }] : []);
    return { transfers, days: itinerary.map((day, i): CompareDay => {
        const stay = night(day.ridingNumber), date = trip.startDate ? weekdayLabel(addDays(trip.startDate, i), true) : undefined;
        if (day.rest) return { day, stay, date, rest: true, name: trip.restNames?.[day.restIndex!] || 'Rest day' };
        const from = day.ridingNumber === 1 ? stops[0].point.label : night(day.ridingNumber - 1).label;
        const trains = transfers.filter(leg => leg.from < day.to && leg.to > day.from).map(leg => leg.text);
        return { day, stay, date, rest: false, from, to: stay.label, trains };
    }) };
}

/** One line about a version: "6 days with 1 rest day · 3h 10m to 5h 40m of riding a day", at the pace of the rider. */
export function compareSummary(days: CompareDay[], bike: Bike): string {
    const rests = days.filter(d => d.rest).length, hours = days.flatMap(d => d.rest ? [] : [plannedSeconds(d.day.hours * 3600, bike) / 3600]);
    const range = hours.length ? `${duration(Math.min(...hours))} to ${duration(Math.max(...hours))} of riding a day` : '';
    return [`${days.length} ${days.length === 1 ? 'day' : 'days'}${rests ? ` with ${rests} rest ${rests === 1 ? 'day' : 'days'}` : ''}`, range].filter(Boolean).join(' · ');
}
