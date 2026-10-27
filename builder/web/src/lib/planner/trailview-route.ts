// The plan's route in TrailView: the colour of each edge in each look, a pin at each night, and its profile.
import { PIN_SIZE } from '../trailview/route/route';
import { dayColor } from './day-colors';
import { cumulative, type Coordinate } from './geo';
import { gradeBand, gradeBands, profileGrades } from './grade-data';
import { placeCategories, type PlaceCategory } from './poi-kinds';
import { profileSamples, type ProfileSample } from './profile-data';
import type { RoutingLine, Surface } from './routing';

export type RouteLook = 'day' | 'grade' | 'surface';

/** The hues of the surface bar, brighter: its muted colours vanish on the terrain. */
export const surfaceColors: Record<Surface, string> = { Paved: '#5b8fd6', Compacted: '#8cc63f', Gravel: '#f0b429', Dirt: '#e0662a', Rough: '#b84a9c', Unknown: '#77746a' };

/** A night of a multi-day plan, in the colour of the day that ends there; a place to sleep shows its kind, else the day's label. */
export interface RouteNight { coordinate: Coordinate; color: string; category?: PlaceCategory; label: string }

export interface RouteOverlay {
    /** One colour per edge of the route in each look. */
    colours: Record<RouteLook, readonly string[]>;
    nights: readonly RouteNight[];
    /** The height at each coordinate of the route. */
    profile: readonly ProfileSample[];
}

/**
 * The colours of each edge of `line`: by the day it is ridden on, where `days` end at shares of its length;
 * by the grade bands of the elevation profile; and by its surface.
 */
export function routeColours(line: Pick<RoutingLine, 'coordinates' | 'elevation' | 'edges'> | undefined, days: readonly { number: number; to: number }[]): Record<RouteLook, string[]> {
    const lengths = cumulative(line?.coordinates ?? []), total = lengths.at(-1) ?? 0;
    const grades = line && total ? profileGrades(profileSamples(line), total) : [];
    const middles = lengths.slice(1).map((to, i) => (lengths[i] + to) / 2 / (total || 1));
    return {
        day: middles.map(middle => dayColor((days.find(day => middle < day.to) ?? days.at(-1))?.number ?? 1)),
        grade: middles.map((_, i) => gradeBands[gradeBand(grades[i] ?? null)].color),
        surface: middles.map((_, i) => surfaceColors[line?.edges.surfaces?.[i] ?? 'Unknown']),
    };
}

/** The image of a night's pin, PIN_SIZE: a drop in the day's colour that points down at the night. */
export function nightPin({ color, category, label }: RouteNight): OffscreenCanvas {
    const { canvas, context, x, y } = drop(color, '#ffffff');
    if (category) {
        context.translate(x - 9, y - 9);
        context.scale(18 / 24, 18 / 24);
        context.lineWidth = 2.4;
        context.lineCap = context.lineJoin = 'round';
        context.stroke(new Path2D(placeCategories[category].icon));
    } else {
        context.fillStyle = '#ffffff';
        context.font = '700 15px system-ui, sans-serif';
        context.textAlign = 'center';
        context.textBaseline = 'middle';
        context.fillText(label, x, y + 1);
    }
    return canvas;
}

/** The image of the pin at a point of the profile: a white drop with a dot in the ink colour. */
export function positionPin(): OffscreenCanvas {
    const { canvas, context, x, y } = drop('#ffffff', '#1c1b14');
    context.beginPath();
    context.arc(x, y, 6, 0, Math.PI * 2);
    context.fillStyle = '#1c1b14';
    context.fill();
    return canvas;
}

/** A drop, PIN_SIZE, filled and outlined, its point at the bottom centre; it draws in output pixels about the centre x, y of its head. */
function drop(fill: string, outline: string) {
    const canvas = new OffscreenCanvas(...PIN_SIZE), context = canvas.getContext('2d')!;
    context.scale(2, 2);
    const [x, y, r] = [PIN_SIZE[0] / 4, 17, 15];
    context.beginPath();
    context.arc(x, y, r, Math.PI * 0.8, Math.PI * 0.2);
    context.lineTo(x, PIN_SIZE[1] / 2 - 2);
    context.closePath();
    context.fillStyle = fill;
    context.fill();
    context.lineWidth = 2.5;
    context.strokeStyle = outline;
    context.stroke();
    return { canvas, context, x, y };
}
