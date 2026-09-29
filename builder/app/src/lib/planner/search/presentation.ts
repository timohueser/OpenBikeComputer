import type { Place } from '../editor';
import type { PlaceCategory } from '../poi-kinds';
import type { QueryPoint, Where, QueryRequest, SearchPlace } from './types';
export const kindLabel = (kind: string) => kind.replaceAll('_', ' ');
export const kindGroups: Record<PlaceCategory, string[]> = {
    water: ['water','drinking_water','water_point','water_tap','fountain','spring'],
    camp: ['campsite'], shelter: ['shelter'], hotel: ['sleep','lodging','hotel','hostel','guest_house','motel','hut'],
    shop: ['resupply','supermarket','convenience','bakery','butcher','marketplace','fuel','shop'],
    food: ['food','cafe','restaurant','fast_food','bar','ice_cream','pub','pizza','kebab'],
    bike: ['bike','bike_shop','repair_station','charging'], toilets: ['toilets','shower','laundry'],
    pharmacy: ['pharmacy','medical','hospital','doctor','clinic'], station: ['transport','train_station','bus_stop','ferry'],
    peak: ['summit','pass'], viewpoint: ['swimming','lake','beach','swimming_pool','sight','viewpoint','castle','church','monastery','museum','ruins','waterfall','tower','bridge'],
};
export const allKinds = [...new Set([...Object.values(kindGroups).flat().filter(k => !['water_point','clinic','shop','pub','pizza','kebab'].includes(k)), 'atm', 'town'])];
export function category(kind: string): PlaceCategory {
    return (Object.entries(kindGroups).find(([, kinds]) => kinds.includes(kind))?.[0] ?? 'viewpoint') as PlaceCategory;
}
export function asPlace(p: SearchPlace): Place {
    return { id: p.source, placeKind: p.kind, label: p.name, kind: 'place', category: category(p.kind), coordinate: [p.lon, p.lat], progress: 0,
        locality: p.city, openingHours: p.opening_hours, hoursStatus: p.hoursStatus, description: p.precision === 'street' ? 'Street location only' : '' };
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
