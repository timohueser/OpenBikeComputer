/** The places that the planner map shows: the kinds that are on, and one highlighted kind. The browser keeps the choice. */
import { categoryIds, defaultKinds, type PlaceCategory } from './poi-kinds';

export interface PlaceChoice {
    /** In the order of `categoryIds`. */
    kinds: PlaceCategory[];
    /** Always one of `kinds`. */
    highlight: PlaceCategory | null;
}

const KEY = 'obc-planner-places';

/** The valid fields of `value`, and the choice of a new rider for the others. */
export function readChoice(value: unknown): PlaceChoice {
    const fields = typeof value === 'object' && value !== null ? value as Record<string, unknown> : {};
    const kinds = Array.isArray(fields.kinds) ? categoryIds.filter(kind => (fields.kinds as unknown[]).includes(kind)) : [...defaultKinds];
    const highlight = kinds.find(kind => kind === fields.highlight) ?? null;
    return { kinds, highlight };
}

function stored(): PlaceChoice {
    try {
        return readChoice(JSON.parse(localStorage.getItem(KEY) ?? 'null'));
    } catch {
        return readChoice(null);
    }
}

export const placeChoice: PlaceChoice = $state(stored());

// Another tab changed the choice: take it, so that the next change here does not write back an old copy.
if (typeof window !== 'undefined') {
    window.addEventListener('storage', event => {
        if (event.key === KEY || event.key === null) Object.assign(placeChoice, stored());
    });
}

function change(next: Partial<PlaceChoice>): void {
    Object.assign(placeChoice, readChoice({ ...placeChoice, ...next }));
    try { localStorage.setItem(KEY, JSON.stringify(placeChoice)); } catch { /* The choice still applies to this page without storage. */ }
}

/** A kind that goes off loses its highlight. */
export function toggleKind(kind: PlaceCategory): void {
    const { kinds } = placeChoice;
    change({ kinds: kinds.includes(kind) ? kinds.filter(other => other !== kind) : [...kinds, kind] });
}

/** A highlighted kind is on, and it is the only highlighted kind. */
export function toggleHighlight(kind: PlaceCategory): void {
    change({ kinds: [...placeChoice.kinds, kind], highlight: placeChoice.highlight === kind ? null : kind });
}
