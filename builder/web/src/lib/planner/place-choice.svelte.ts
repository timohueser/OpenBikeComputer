/** The places that the planner map shows: all or none, the kinds that are on, and one highlighted kind. The browser keeps the choice. */
import { categoryIds, defaultKinds, type PlaceCategory } from './poi-kinds';

export interface PlaceChoice {
    shown: boolean;
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
    return { shown: typeof fields.shown === 'boolean' ? fields.shown : true, kinds, highlight };
}

function stored(): PlaceChoice {
    try {
        return readChoice(JSON.parse(localStorage.getItem(KEY) ?? 'null'));
    } catch {
        return readChoice(null);
    }
}

export const placeChoice: PlaceChoice = $state(stored());

function change(next: Partial<PlaceChoice>): void {
    Object.assign(placeChoice, readChoice({ ...placeChoice, ...next }));
    try { localStorage.setItem(KEY, JSON.stringify(placeChoice)); } catch { /* The choice still applies to this page without storage. */ }
}

/** Hides or shows all places, and keeps the kinds. */
export function togglePlaces(): void {
    change({ shown: !placeChoice.shown });
}

/** A kind that goes off loses its highlight. */
export function toggleKind(kind: PlaceCategory): void {
    const { kinds } = placeChoice;
    change({ shown: true, kinds: kinds.includes(kind) ? kinds.filter(other => other !== kind) : [...kinds, kind] });
}

/** A highlighted kind is on, and it is the only highlighted kind. */
export function toggleHighlight(kind: PlaceCategory): void {
    change({ shown: true, kinds: [...placeChoice.kinds, kind], highlight: placeChoice.highlight === kind ? null : kind });
}
