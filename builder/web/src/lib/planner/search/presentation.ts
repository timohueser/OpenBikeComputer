import type { Place, Stay } from '../editor';
import { placeCategories, poiKinds, type PlaceCategory } from '../poi-kinds';
import type { QueryPoint, SearchPlace } from './types';
import contract from '../../../../../../planner/search/query/contract.json' with { type: 'json' };

/** The query language that search accepts. */
export { contract };
export const kindLabel = (kind: string) => kind.replaceAll('_', ' ');
export const allKinds = Object.keys(contract.kinds);
const kinds: Record<string, { category: string | null }> = contract.kinds, dataKinds: Record<string, string> = contract.data;
/** The map category of a query kind or a search-data kind, such as `shop` for `shop=yes`. A kind without one shows as a
 * viewpoint. */
export function category(kind: string): PlaceCategory {
    return (poiKinds[kind]?.category ?? kinds[kind]?.category ?? kinds[dataKinds[kind]]?.category ?? (Object.hasOwn(placeCategories, kind) ? kind : 'viewpoint')) as PlaceCategory;
}
const stayKinds: Record<string, Stay> = { campsite: 'camp', hut: 'hut', hostel: 'hostel', hotel: 'hotel',
    guest_house: 'hotel', motel: 'hotel', chalet: 'hotel', shelter: 'shelter' };
/** The kind of place to sleep of a place kind, such as `hotel` for `guest_house`; undefined for any other kind. */
export const stayKind = (placeKind?: string): Stay | undefined => placeKind && Object.hasOwn(stayKinds, placeKind) ? stayKinds[placeKind] : undefined;
/** The map category of a place to sleep, such as `camp`; undefined for any other kind, such as a town. */
export const sleepCategory = (placeKind?: string): PlaceCategory | undefined => stayKind(placeKind) ? category(placeKind!) : undefined;
/** The five kinds of place to sleep as the trip row and a night list them. */
export const stayOptions: { kind: Stay; label: string }[] = [
    { kind: 'camp', label: 'Campsites' }, { kind: 'hut', label: 'Huts' }, { kind: 'hostel', label: 'Hostels' },
    { kind: 'hotel', label: 'Hotels' }, { kind: 'shelter', label: 'Shelters' },
];
/** The icon of a kind of place to sleep. */
export const stayIcon = (stay?: Stay) => ({ camp: 'camp', hut: 'hotel', hostel: 'hotel', hotel: 'hotel' } as Record<string, string>)[stay ?? ''] ?? 'moon';
/** The icon of a night by the kind of place it is at. */
export const sleepIcon = (placeKind?: string) => stayIcon(stayKind(placeKind));
export function asPlace(p: SearchPlace): Place {
    return { id: p.source, placeKind: p.kind, label: p.name, kind: 'place', category: category(p.kind), coordinate: [p.lon, p.lat],
        locality: p.city, openingHours: p.opening_hours, hoursStatus: p.hoursStatus, website: p.website, phone: p.phone, content: p.content,
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
