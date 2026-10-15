import type { Place } from './editor';
import { groupStops, type Stop } from './stops';
import { searchPlaces, type SearchContext } from './search/types';

export type NightStops = { state: 'loading' } | { state: 'ready'; stops: Stop[] } | { state: 'error'; message: string };

const along = (from: number, to: number) => ({ scope: 'route', along: { ref: 'start', from: { value: Math.round(from * 10) / 10, unit: 'km' }, to: { value: Math.round(to * 10) / 10, unit: 'km' } } });

/** The stops along stretches of one route: one search for places to sleep and one for food, shops and water per stretch.
 * A stretch keeps its answer while the route stays the same. */
export class StopFinder {
    found = $state.raw<Record<string, NightStops>>({});
    private route = '';
    private aborts = new Map<string, AbortController>();

    /** Searches the stretch `[from, to]` km of the route that `context` describes; `route` names that route. */
    find(route: string, context: SearchContext, [from, to]: [number, number]): string {
        const key = `${from.toFixed(1)}-${to.toFixed(1)}`;
        if (route !== this.route) this.reset(route);
        if (this.found[key] || this.aborts.has(key)) return key;
        const abort = new AbortController();
        this.aborts.set(key, abort);
        this.found = { ...this.found, [key]: { state: 'loading' } };
        const where = along(from, to);
        Promise.all([
            searchPlaces('', context, 80, abort.signal, { type: 'places', what: ['sleep'], where }),
            searchPlaces('', context, 100, abort.signal, { type: 'places', what: ['food', 'resupply', 'water'], where }),
        ]).then(([sleep, services]) => {
            if (abort.signal.aborted) return;
            this.aborts.delete(key);
            this.found = { ...this.found, [key]: { state: 'ready', stops: groupStops(sleep.results ?? [], services.results ?? []) } };
        }, () => {
            if (abort.signal.aborted) return;
            this.aborts.delete(key);
            this.found = { ...this.found, [key]: { state: 'error', message: 'Places to sleep could not load. Check your connection and retry.' } };
        });
        return key;
    }

    /** Forgets a failed stretch, so the next `find` searches it again. */
    retry(key: string): void {
        const { [key]: _, ...rest } = this.found;
        this.found = rest;
    }

    private reset(route: string): void {
        for (const abort of this.aborts.values()) abort.abort();
        this.aborts.clear();
        this.route = route;
        this.found = {};
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
    /** The town at the suggested end of a day that is not pinned. */
    around(night: number): Stop | undefined;
    /** The towns before and after the suggested end. */
    neighbours(night: number): [Stop | undefined, Stop | undefined];
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
