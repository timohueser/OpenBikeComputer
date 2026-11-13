/** Compares the area and the colours of two device maps, and gives those of the map on the device. */
import type { RememberedDevice } from '../../lib/device-sync/remembered.svelte';
import type { MapBox, NextMap } from '../../lib/library/records';

/** The regions, the boxes and the colours of a device map. */
export type MapArea = Omit<NextMap, 'bytes'>;

/**
 * The area and the colours of the map on the device, or null where this browser does not know them. It knows a map
 * that it sent. A map with the name of one catalog region is that region, in the first colours of the catalog.
 */
export function deviceArea(device: Pick<RememberedDevice, 'map' | 'mapObject' | 'built'> | null, regions: { id: string; name: string }[]): MapArea | null {
    if (!device) return null;
    if (device.built && device.built.object === device.mapObject) {
        const { regions, boxes, light, dark } = device.built;
        return { regions, boxes, light, dark };
    }
    const named = regions.filter(region => region.name === device.map);
    return named.length === 1 ? { regions: [named[0].id], boxes: [], light: 'default', dark: 'dusk' } : null;
}

const boxKey = (box: MapBox) => `${box.minLat},${box.minLon},${box.maxLat},${box.maxLon}`;
const sameSet = (a: string[], b: string[]) => new Set(a).size === new Set(b).size && a.every(value => b.includes(value));

/** The two maps have the same regions and the same boxes, in any order. */
export const sameArea = (a: MapArea, b: MapArea) => sameSet(a.regions, b.regions) && sameSet(a.boxes.map(boxKey), b.boxes.map(boxKey));

/** The two maps have the same area and the same colours. */
export const sameMap = (a: MapArea, b: MapArea) => sameArea(a, b) && a.light === b.light && a.dark === b.dark;
