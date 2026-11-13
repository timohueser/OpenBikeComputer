/** The town at the top of a climb, for the name "To <town>". */
import type { Coordinate } from '../../lib/planner/geo';
import { townName } from '../../lib/planner/point-names';

/** At most SEARCHES place searches run at a time: each search has a time limit. */
const SEARCHES = 3;
const towns = new Map<string, Promise<string | null>>(), waiting: (() => void)[] = [];
let searching = 0;

/** One place search for each top while the app lives. A search that finds no town, or fails, is not kept. */
export function townOf(top: Coordinate): Promise<string | null> {
    const key = top.join();
    if (!towns.has(key)) towns.set(key, new Promise(resolve => {
        const search = () => {
            searching++;
            void townName(top).then(name => { if (name === null) towns.delete(key); resolve(name); }).finally(() => { searching--; waiting.shift()?.(); });
        };
        if (searching < SEARCHES) search(); else waiting.push(search);
    }));
    return towns.get(key)!;
}
