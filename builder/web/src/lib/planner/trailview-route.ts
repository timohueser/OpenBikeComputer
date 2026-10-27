// The plan's route in TrailView: the colour of each edge in each look, a pin at each night, and its profile.
import { BADGE_SIZE } from '../trailview/route/route';
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

/** The badge of a night's pin, BADGE_SIZE: a disc in the day's colour with a white edge. */
export function nightBadge({ color, category, label }: RouteNight): OffscreenCanvas {
    const canvas = new OffscreenCanvas(BADGE_SIZE, BADGE_SIZE), context = canvas.getContext('2d')!;
    // Drawn in output pixels, at half the image's pixels.
    context.scale(2, 2);
    const c = BADGE_SIZE / 4;
    context.beginPath();
    context.arc(c, c, c - 1.5, 0, Math.PI * 2);
    context.fillStyle = color;
    context.fill();
    context.lineWidth = 2.5;
    context.strokeStyle = '#ffffff';
    context.stroke();
    if (category) {
        context.translate(c - 9, c - 9);
        context.scale(18 / 24, 18 / 24);
        context.lineWidth = 2.4;
        context.lineCap = context.lineJoin = 'round';
        context.stroke(new Path2D(placeCategories[category].icon));
    } else {
        context.fillStyle = '#ffffff';
        context.font = '700 15px system-ui, sans-serif';
        context.textAlign = 'center';
        context.textBaseline = 'middle';
        context.fillText(label, c, c + 1);
    }
    return canvas;
}
