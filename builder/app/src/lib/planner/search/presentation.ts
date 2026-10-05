import type { Place } from '../editor';
import { placeCategories, type PlaceCategory } from '../poi-kinds';
import type { QueryPoint, Where, QueryRequest, SearchPlace } from './types';
import contract from '../../../../../../apps/planner-search/query/contract.json' with { type: 'json' };

/** The query language that search accepts. */
export { contract };
export const kindLabel = (kind: string) => kind.replaceAll('_', ' ');
export const allKinds = Object.keys(contract.kinds);
const kinds: Record<string, { category: string | null }> = contract.kinds, dataKinds: Record<string, string> = contract.data;
/** The map category of a query kind or a search-data kind, such as `shop` for `shop=yes`. A kind without one shows as a
 * viewpoint. */
export function category(kind: string): PlaceCategory {
    return (kinds[kind]?.category ?? kinds[dataKinds[kind]]?.category ?? (Object.hasOwn(placeCategories, kind) ? kind : 'viewpoint')) as PlaceCategory;
}
export function asPlace(p: SearchPlace): Place {
    return { id: p.source, placeKind: p.kind, label: p.name, kind: 'place', category: category(p.kind), coordinate: [p.lon, p.lat],
        locality: p.city, openingHours: p.opening_hours, hoursStatus: p.hoursStatus, website: p.website, phone: p.phone, detailsLoaded: true,
        description: p.precision === 'street' ? 'Street location only' : p.description ?? '' };
}
export function pointLabel(p: QueryPoint): string {
    if ('name' in p) return p.name;
    if ('kind' in p) return `Nearest ${kindLabel(p.kind)}`;
    if ('here' in p) return 'Your location';
    if ('plan' in p) return `Route ${p.plan}`;
    if ('day' in p) return `Day ${p.day} ${p.part ?? 'end'}`;
    const a = p.along;
    return `${a.ref === 'km' ? '' : `From ${a.ref}: `}${a.at ? `${a.at.value} ${a.at.unit}` : `${a.from?.value ?? 0}–${a.to?.value ?? 'end'} ${a.to?.unit ?? a.from?.unit ?? 'km'}`}`;
}
export function whereLabel(w: Where): string {
    if (w.anchor) return 'Near the selected point';
    return [w.day ? `Day ${w.day}${w.part ? ` ${w.part}` : ''}` : w.scope === 'route' ? w.part ? `Near route ${w.part}` : 'Along the route' : w.scope === 'here' ? 'Near your location' : '',
        w.near ? `${w.near.length > 1 ? 'Between' : 'Near'} ${w.near.map(pointLabel).join(' and ')}` : '',
        w.along ? pointLabel({ along: w.along }) : '', w.before ? `Before ${pointLabel(w.before)}` : '', w.after ? `After ${pointLabel(w.after)}` : '',
    ].filter(Boolean).join(' · ') || 'In this map view';
}
export function fieldLabel(key: string, value: unknown): string {
    if (key === 'where') return whereLabel(value as Where);
    if (['from','to','point','at','near'].includes(key)) return `${key}: ${pointLabel(value as QueryPoint)}`;
    if (key === 'via') return `Via ${(value as QueryPoint[]).map(pointLabel).join(', ')}`;
    if (key === 'what') return Array.isArray(value) ? value.map(kindLabel).join(', ') : kindLabel(String(value));
    if (key === 'open') { const o = value as QueryRequest['open']; return o?.now ? 'Open now' : `Open ${o?.weekday ?? `on Day ${o?.day}`}`; }
    if (typeof value === 'object' && value && 'unit' in value && 'value' in value) return `${kindLabel(key)}: ${value.value} ${value.unit}`;
    return `${kindLabel(key)}: ${kindLabel(String(value))}`;
}

// Settlement kinds from the largest down.
const settlements = ['city', 'town', 'village', 'suburb', 'district', 'hamlet', 'locality', 'isolated_dwelling'];

/** Starts for signed routes near a typed name, best first: an exact name before others, then the larger settlement, then the search order. */
export function routesPlaces(results: SearchPlace[], name: string): SearchPlace[] {
    const typed = name.trim().toLowerCase();
    const rank = (place: SearchPlace) => (place.name.toLowerCase() === typed ? 0 : 100) + (settlements.includes(place.kind) ? settlements.indexOf(place.kind) : 50);
    return [...results].sort((a, b) => rank(a) - rank(b));
}
