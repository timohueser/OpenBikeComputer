import type { Place } from './editor';
import { groupStops, type Proposal, type Sleep, type Stop } from './stops';
import { searchPlaces, type SearchContext } from './search/types';

export type NightStops = { state: 'loading' } | { state: 'ready'; stops: Stop[] } | { state: 'error'; message: string };

const along = (from: number, to: number) => ({ scope: 'route', along: { ref: 'start', from: { value: Math.round(from * 10) / 10, unit: 'km' }, to: { value: Math.round(to * 10) / 10, unit: 'km' } } });

/** The key of the stretch `[from, to]` km of the route that `route` names. */
export const stretchKey = (route: string, [from, to]: [number, number]) => `${route}|${from.toFixed(1)}-${to.toFixed(1)}`;

// Answers kept for stretches no longer wanted, so Undo and a step back do not search again.
const kept = 40;

/** The stops along stretches of routes: one search for places to sleep and one for food, shops and water per stretch. */
export class StopFinder {
    found = $state.raw<Record<string, NightStops>>({});
    private aborts = new Map<string, AbortController>();
    private wanted: { route: string; context: SearchContext; stretches: [number, number][] } | null = null;

    /** Searches the `stretches` of `route` not yet found, along the route that `context` describes, and stops the
     * searches of other stretches. */
    want(route: string, context: SearchContext, stretches: [number, number][]): void {
        this.wanted = { route, context, stretches };
        const keys = new Set(stretches.map(stretch => stretchKey(route, stretch)));
        let found = this.found;
        for (const [key, abort] of this.aborts) {
            if (keys.has(key)) continue;
            abort.abort();
            this.aborts.delete(key);
            const { [key]: _, ...rest } = found;
            found = rest;
        }
        for (const stretch of stretches) {
            const key = stretchKey(route, stretch);
            if (!found[key]) {
                found = { ...found, [key]: { state: 'loading' } };
                this.search(key, context, stretch);
            }
        }
        const old = Object.keys(found).filter(key => !keys.has(key));
        const drop = new Set(old.slice(0, Math.max(0, old.length - kept)));
        if (drop.size) found = Object.fromEntries(Object.entries(found).filter(([key]) => !drop.has(key)));
        if (found !== this.found) this.found = found;
    }

    /** Searches a failed stretch again. */
    retry(key: string): void {
        const { [key]: _, ...rest } = this.found;
        this.found = rest;
        if (this.wanted) this.want(this.wanted.route, this.wanted.context, this.wanted.stretches);
    }

    private search(key: string, context: SearchContext, [from, to]: [number, number]): void {
        const abort = new AbortController();
        this.aborts.set(key, abort);
        const where = along(from, to);
        const settle = (answer: NightStops) => {
            if (abort.signal.aborted) return;
            this.aborts.delete(key);
            this.found = { ...this.found, [key]: answer };
        };
        Promise.all([
            searchPlaces('', context, 80, abort.signal, { type: 'places', what: ['sleep'], where }),
            searchPlaces('', context, 100, abort.signal, { type: 'places', what: ['food', 'resupply', 'water'], where }),
        ]).then(([sleep, services]) => settle({ state: 'ready', stops: groupStops(sleep.results ?? [], services.results ?? []) }),
            () => settle({ state: 'error', message: 'Places to sleep could not load. Check your connection and retry.' }));
    }
}

/** What the itinerary needs to choose where to sleep: the towns of each night and the actions on them. A night is the
 * riding day that ends there. */
export interface NightChoices {
    /** The night whose towns are listed. */
    choosing: number | null;
    /** The open town of that night. */
    stopId: string | null;
    found(night: number): NightStops | undefined;
    window(night: number): [number, number] | null;
    /** Where the rider sleeps, for the whole trip. */
    sleep: Sleep;
    /** The suggestion for a night that is not pinned: the proposal, or the town at a day end the rider set. */
    around(night: number): Proposal | undefined;
    /** The towns before and after the suggestion that suit the rider. */
    neighbours(night: number): [Stop | undefined, Stop | undefined];
    /** The suggested nights with a campsite. */
    campsites: number;
    keepCampsites(): void;
    /** The date of the night, when the trip has a start date. */
    date(night: number): string | undefined;
    elevationAt(km: number): number | null;
    choose(night: number | null): void;
    open(id: string | null): void;
    step(night: number, direction: -1 | 1): void;
    endHere(night: number, stop: Stop): void;
    stay(night: number, place: Place): void;
    selectPlace(place: Place): void;
    pick(): void;
    retry(night: number): void;
}
