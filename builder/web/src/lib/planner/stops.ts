import { allStay, overnightWindow, planView, provisionalDays, type Day, type Place, type Stay, type Trip } from './editor';
import { movingSecondsAt, type RoutingLine } from './routing';
import { asPlace, stayKind } from './search/presentation';
import type { SearchPlace } from './search/types';

/** A place of a stop with its position along the route and its straight-line distance off it, in km. */
export interface StopPlace { place: Place; km: number; off: number }

/** A town along the route where the rider can sleep: its places to sleep, and food, shops and water around them. */
export interface Stop {
    id: string;
    name: string;
    /** Where the stop sits along the route: the middle of its places to sleep, in km. */
    km: number;
    from: number;
    to: number;
    sleep: StopPlace[];
    food: StopPlace[];
    shop: StopPlace[];
    water: StopPlace[];
}

/** Places to sleep further apart along the route than this belong to different stops, unless they are in the same
 * town and no further apart than `townGap`. A stop of places in different towns spans at most `townGap`, so a valley of
 * villages does not chain into one stop. */
const stopGap = 2.5, townGap = 6;
/** Food, shops and water count for a stop within this distance of its places, along and off the route. */
const serviceReach = 1.5;

const stopPlace = (result: SearchPlace): StopPlace | null => result.position
    ? { place: asPlace(result), km: result.position.along, off: result.position.distance } : null;

/** Groups places to sleep into stops by their gaps along the route, and gives each stop the services around it. A stop
 * takes the name of the town most of its places are in. */
export function groupStops(sleep: SearchPlace[], services: SearchPlace[]): Stop[] {
    const places = sleep.map(stopPlace).filter((p): p is StopPlace => !!p).sort((a, b) => a.km - b.km);
    const nearby = services.map(stopPlace).filter((p): p is StopPlace => !!p && p.off <= serviceReach);
    const groups: StopPlace[][] = [];
    for (const place of places) {
        const group = groups.at(-1);
        const gap = group ? place.km - group.at(-1)!.km : Infinity;
        if ((gap <= stopGap && place.km - group![0].km <= townGap) || (gap <= townGap && !!place.place.locality && group!.some(p => p.place.locality === place.place.locality))) group!.push(place);
        else groups.push([place]);
    }
    return groups.map(group => {
        const from = group[0].km, to = group.at(-1)!.km;
        const around = nearby.filter(p => p.km >= from - serviceReach && p.km <= to + serviceReach).sort((a, b) => a.off - b.off);
        return {
            id: group[0].place.id, name: townName(group), km: group[Math.floor(group.length / 2)].km, from, to,
            sleep: [...group].sort((a, b) => a.off - b.off),
            food: around.filter(p => p.place.category === 'food'),
            shop: around.filter(p => p.place.category === 'shop'),
            water: around.filter(p => p.place.category === 'water'),
        };
    });
}

/** What a stop offers: "6 hotels · 1 campsite". */
export function stopCounts(stop: Stop): string {
    const count = (category: string, one: string, many: string) => {
        const n = stop.sleep.filter(p => p.place.category === category).length;
        return n ? `${n} ${n === 1 ? one : many}` : '';
    };
    return [count('hotel', 'hotel', 'hotels'), count('camp', 'campsite', 'campsites'), count('shelter', 'shelter', 'shelters')].filter(Boolean).join(' · ');
}

function townName(group: StopPlace[]): string {
    const counts = new Map<string, number>();
    for (const { place } of group) if (place.locality) counts.set(place.locality, (counts.get(place.locality) ?? 0) + 1);
    return [...counts].sort((a, b) => b[1] - a[1])[0]?.[0] ?? group[0].place.label;
}

/** The stretch where stops for night `night` are offered, in km: half a day of riding on either side of the even day end,
 * within the fixed nights around it. */
export function stopWindow(trip: Trip, line: RoutingLine | undefined, night: number): [number, number] | null {
    const { total } = planView(trip, line);
    const window = overnightWindow(trip, line, night, .5);
    return window.blocked || !total ? null : [window.from * total, window.to * total];
}

/** The `stopWindow` of each night without a kept point, and of night `also`. */
export function nightWindows(trip: Trip, line: RoutingLine | undefined, also: number | null = null): Record<number, [number, number]> {
    if (!line) return {};
    return Object.fromEntries(planView(trip, line).days.slice(0, -1).flatMap(day => {
        const window = !day.pinned || day.number === also ? stopWindow(trip, line, day.number) : null;
        return window ? [[day.number, window]] : [];
    }));
}

/** The two days around night `night` when the rider sleeps at `km`. */
export function daysAround(trip: Trip, line: RoutingLine | undefined, night: number, km: number): [Day, Day] {
    const { total } = planView(trip, line);
    const days = provisionalDays(trip, line, night, total ? km / total : 0);
    return [days[night - 1], days[night]];
}

/** The stop that gives the most even days, which the list marks. */
export function evenStop(stops: Stop[], trip: Trip, line: RoutingLine | undefined, night: number): Stop | undefined {
    const { total } = planView(trip, line);
    const center = overnightWindow(trip, line, night).center * total;
    return [...stops].sort((a, b) => Math.abs(a.km - center) - Math.abs(b.km - center))[0];
}

/** The kinds of place to sleep of night `night`. */
export const nightKinds = (trip: Trip, night: number): Stay[] => trip.nightStay?.[night] ?? trip.stay ?? allStay;

/** A suggested night: a stop, the place it ends at, and where the day ends, in km. */
export interface Proposal { stop: Stop; place?: StopPlace; km: number }

// The planner names no hotel or hostel: it cannot pick one in the rider's price range.
const namedKinds: Stay[] = ['camp', 'hut', 'shelter'];

/** A night of only campsites, huts or shelters ends at the place of that kind nearest `km`; any other night ends in the
 * town. */
export function proposal(stop: Stop, kinds: Stay[], km = stop.km): Proposal {
    const kind = kinds.length === 1 && namedKinds.includes(kinds[0]) ? kinds[0] : undefined;
    const place = kind && stop.sleep.filter(p => stayKind(p.place.placeKind) === kind).sort((a, b) => Math.abs(a.km - km) - Math.abs(b.km - km))[0];
    return { stop, place, km: place?.km ?? stop.km };
}

/** How the ladder names a suggested night: by the place it ends at, else by its town. */
export const proposalTitle = (p: Proposal) => p.place ? p.place.place.label : `Around ${p.stop.name}`;

/** Whether a stop has a place of one of `kinds`. */
export const suits = (stop: Stop, kinds: Stay[]) => stop.sleep.some(p => kinds.some(kind => kind === stayKind(p.place.placeKind)));

/** A stop for every night that is neither pinned nor set by the rider, night by night: of the stops that suit the rider,
 * the one nearest an even share of the riding time left before the next fixed night. A night without one keeps its even
 * end. */
export function proposeNights(trip: Trip, line: RoutingLine | undefined, stopsOf: (night: number) => Stop[]): Map<number, Proposal> {
    const proposals = new Map<number, Proposal>();
    const { total, days } = planView(trip, line);
    if (!line || !total) return proposals;
    const effort = (km: number) => line.seconds > 0 ? movingSecondsAt(line, km / total) : km;
    const pinned = new Set(trip.points.filter(p => p.kind === 'night').map(p => p.night));
    let planned = trip;
    for (let night = 1; night < days.length; night++) {
        if (pinned.has(night) || trip.splits?.[night] !== undefined) continue;
        const window = overnightWindow(planned, line, night, .5);
        if (window.blocked) continue;
        const even = effort(window.center * total), kinds = nightKinds(trip, night);
        const best = stopsOf(night).filter(stop => suits(stop, kinds)).map(stop => proposal(stop, kinds, window.center * total))
            .filter(p => p.km >= window.from * total && p.km <= window.to * total)
            .sort((a, b) => Math.abs(effort(a.km) - even) - Math.abs(effort(b.km) - even))[0];
        if (!best) continue;
        proposals.set(night, best);
        planned = withProposals(planned, line, new Map([[night, best]]));
    }
    return proposals;
}

/** The trip with each proposed night as its day end. */
export function withProposals(trip: Trip, line: RoutingLine | undefined, proposals: Map<number, Proposal>): Trip {
    const { total } = planView(trip, line);
    if (!proposals.size || !total) return trip;
    return { ...trip, splits: { ...Object.fromEntries([...proposals].map(([night, p]) => [night, p.km / total])), ...trip.splits } };
}
