import type { GpxImport } from '../../lib/core/gpx';
import type { Waypoint } from '../../lib/planner/route-facts';
import type { RoutingLine } from '../../lib/planner/routing';

/** The name of a file of a route or a ride. */
export const fileName = (name: string, extension: string) => `${name.replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 80) || 'route'}.${extension}`;

/** Saves a file that the page made, named after a route or a ride. */
export function download(content: BlobPart, type: string, name: string, extension: string): void {
    const url = URL.createObjectURL(new Blob([content], { type }));
    const link = document.createElement('a');
    link.href = url;
    link.download = fileName(name, extension);
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** The calculated line of a route with its waypoints, in the microdegrees of `lib/core`, for a GPX or FIT file. */
export function routeCourse(name: string, line: RoutingLine, stops: Waypoint[]): GpxImport {
    const micro = (value: number) => Math.round(value * 1e6);
    return {
        name,
        points: line.coordinates.map(([lon, lat], i) => {
            const ele = line.elevation[i];
            return ele === null ? { lat: micro(lat), lon: micro(lon) } : { lat: micro(lat), lon: micro(lon), ele };
        }),
        waypoints: stops.map(({ point }) => ({ lat: micro(point.coordinate[1]), lon: micro(point.coordinate[0]), name: point.label, note: point.note })),
    };
}
