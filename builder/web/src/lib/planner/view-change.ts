import type { Bounds } from './config';
import type { Coordinate } from './geo';

export type NightView = { action: 'stay' } | { action: 'pan' | 'fit'; box: Bounds };

const inside = ([west, south, east, north]: Bounds, [lon, lat]: Coordinate) => lon >= west && lon <= east && lat >= south && lat <= north;

/** How far the map moves to show the end of a day, `spot`, and the places of each listed town, `towns`, nearest first. It
 * stays when `spot` and three towns are in `view`. Else it pans to the box of `spot` and the three nearest towns, and it
 * zooms out only when the box takes more than 80 % of the width or the height of `view`. */
export function nightView(view: Bounds, spot: Coordinate, towns: Coordinate[][]): NightView {
    const needed = Math.min(3, towns.length);
    if (inside(view, spot) && towns.filter(places => places.some(place => inside(view, place))).length >= needed) return { action: 'stay' };
    const points = [spot, ...towns.slice(0, needed).flat()], lons = points.map(p => p[0]), lats = points.map(p => p[1]);
    const box: Bounds = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];
    const pan = box[2] - box[0] <= .8 * (view[2] - view[0]) && box[3] - box[1] <= .8 * (view[3] - view[1]);
    return { action: pan ? 'pan' : 'fit', box };
}
