import type { Bounds } from './config';
import type { Coordinate } from './geo';

export type NightView = { action: 'stay' } | { action: 'fit'; box: Bounds };

const inside = ([west, south, east, north]: Bounds, [lon, lat]: Coordinate) => lon >= west && lon <= east && lat >= south && lat <= north;

/** How the map shows a night: the end of the day, `spot`, and the places of each stop, `towns`. It stays when all of
 * them are in `view` and their box takes at least 60 % of its width or its height, so that the areas of the towns have
 * room. Else it fits the box. */
export function nightView(view: Bounds, spot: Coordinate, towns: Coordinate[][]): NightView {
    const points = [spot, ...towns.flat()], lons = points.map(p => p[0]), lats = points.map(p => p[1]);
    const box: Bounds = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];
    const roomy = !towns.length || box[2] - box[0] >= .6 * (view[2] - view[0]) || box[3] - box[1] >= .6 * (view[3] - view[1]);
    return roomy && points.every(point => inside(view, point)) ? { action: 'stay' } : { action: 'fit', box };
}
