/** Where a line starts and which way it goes there: the start mark of the routes map. */
import { distanceM } from '../../lib/library/summary';

/** The direction at the start looks this far along the line, so a short first segment does not turn the arrow. */
const AHEAD_M = 150;

/** The first point of a line of `[lon, lat, …]`, and the bearing in degrees from north, clockwise, to the first point
 * at least `AHEAD_M` from it, else to the last point. A line with fewer than two points has no start. */
export function startOf(line: number[]): { at: [number, number]; bearing: number } | undefined {
    if (line.length < 4) return undefined;
    const [lon, lat] = line;
    let i = 2;
    while (i < line.length - 2 && distanceM(lon, lat, line[i], line[i + 1]) < AHEAD_M) i += 2;
    const east = (line[i] - lon) * Math.cos(lat * Math.PI / 180), north = line[i + 1] - lat;
    return { at: [lon, lat], bearing: (Math.atan2(east, north) * 180 / Math.PI + 360) % 360 };
}

/** A white chevron with a dark edge that points up, 32 × 32 pixels. */
export function chevronImage(): ImageData {
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 32;
    const context = canvas.getContext('2d')!;
    context.lineCap = context.lineJoin = 'round';
    for (const [colour, width] of [['#3a342c', 9], ['#fff', 5]] as const) {
        context.beginPath();
        context.moveTo(8, 21);
        context.lineTo(16, 12);
        context.lineTo(24, 21);
        context.strokeStyle = colour;
        context.lineWidth = width;
        context.stroke();
    }
    return context.getImageData(0, 0, 32, 32);
}
