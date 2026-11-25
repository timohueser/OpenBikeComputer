// Landmarks: the buildings a place is known by, drawn as themselves rather than as a kit house. Footprint-sized, base at
// the origin, z up.
import { rgb } from '../structures/mesh';
import { fieldstone } from './amenities';
import { footing, Model, random, shade, type Vec } from './flat';

const STONE = ['#8f8b80', '#847f75', '#999589'].map(h => rgb(h)), INSIDE = rgb('#6d6a62'), HEAD = rgb('#8c8f72'), DARK_OPENING = rgb('#2c2a28');
/** A ruin's wall is this thick at most, stands in pieces about this long, and breaks in courses this high. */
const WALL_M = 0.7, PIECE_M = 1.5, COURSE_M = 0.9;

/**
 * A ruin on a width × depth footprint: one shell of thick walls. One corner stands to the full height, and the heads
 * step down from it in courses to low walls, with moss on them, a doorway, slits, and the fallen stones in a heap at
 * the far corner. No two faces of it lie in one plane over each other.
 */
export function ruin(width: number, depth: number, height: number, seed: number): Model {
    const m = new Model(), rand = random(seed), [hx, hy] = [width / 2, depth / 2], thick = Math.min(WALL_M, 0.3 * Math.min(width, depth)), deep = footing(Math.hypot(hx, hy));
    // The stops round the wall, counter-clockwise: a point of its outer face and the point across it on its inner face.
    const corners = [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]], stops: { out: [number, number]; inn: [number, number]; side: number }[] = [];
    corners.forEach(([ax, ay], side) => {
        const [bx, by] = corners[(side + 1) % 4], n = Math.max(2, Math.round(Math.hypot(bx - ax, by - ay) / PIECE_M));
        for (let k = 0; k < n; k++) {
            const x = ax + ((bx - ax) * k) / n, y = ay + ((by - ay) * k) / n;
            stops.push({ out: [x, y], inn: [Math.max(thick - hx, Math.min(hx - thick, x)), Math.max(thick - hy, Math.min(hy - thick, y))], side });
        }
    });
    // One corner still stands to the full height. From it the heads step down in whole courses, both ways round, to low
    // walls about the far corner: the breach. A piece here and there lost a course more.
    const count = stops.length, corner = stops.findIndex(stop => stop.side === Math.floor(rand() * 4)), breach = (corner + Math.floor(count / 2)) % count, door = (corner + 2) % count;
    const tops = stops.map((_, i) => {
        const from = Math.min(Math.abs(i - corner), count - Math.abs(i - corner)), away = (from + (i < corner ? 0.5 : 0)) / (count / 2), share = Math.max(0.16, Math.min(1, (0.62 - away) / 0.45));
        const lost = rand() < 0.3 ? COURSE_M : 0;
        return Math.max(i === door ? 4 * COURSE_M : 2 * COURSE_M, COURSE_M * Math.round((height * share) / COURSE_M) - lost);
    });
    const at = ([x, y]: [number, number], z: number): Vec => [x, y, z];
    stops.forEach((stop, i) => {
        const next = stops[(i + 1) % count], top = tops[i], stone = STONE[stop.side % STONE.length];
        // The outer face in three bands of stone, the inner face, and the head.
        [-deep, top / 3, (2 * top) / 3, top].slice(1).forEach((z, band, all) => {
            const low = band ? all[band - 1] : -deep;
            m.poly([at(stop.out, low), at(next.out, low), at(next.out, z), at(stop.out, z)], shade(stone, 0.95 + 0.09 * rand()));
        });
        m.poly([at(next.inn, -deep), at(stop.inn, -deep), at(stop.inn, top), at(next.inn, top)], INSIDE);
        m.poly([at(stop.out, top), at(next.out, top), at(next.inn, top), at(stop.inn, top)], HEAD);
        // The step to the next piece's head.
        const other = tops[(i + 1) % count];
        if (other !== top) m.poly([at(next.out, Math.min(top, other)), at(next.inn, Math.min(top, other)), at(next.inn, Math.max(top, other)), at(next.out, Math.max(top, other))], shade(stone, 0.9));
        // The doorway under a pointed head, and a slit in each third piece that is high enough, each a little before the wall.
        const [dx, dy] = [next.out[0] - stop.out[0], next.out[1] - stop.out[1]], turn = Math.atan2(dy, dx), middle: Vec = [(stop.out[0] + next.out[0]) / 2, (stop.out[1] + next.out[1]) / 2, 0];
        if (i === door) m.add(new Model().box([0, -0.01, 1], [0.95, 0.08, 2], DARK_OPENING).prism([[-0.475, 2], [0.475, 2], [0, 2.6]], -0.05, 0.03, DARK_OPENING).place(turn, middle));
        else if (i % 3 === 1 && top > 5 * COURSE_M) m.add(new Model().box([0, -0.01, 0.55 * top], [0.28, 0.08, 1.1], DARK_OPENING).place(turn, middle));
    });
    // The stones that fell at the breach: a heap outside the wall and a few inside.
    const [bx, by] = stops[breach].out, far = Math.hypot(bx, by) || 1;
    for (let k = 0; k < 9; k++) {
        const out = k < 6 ? 0.6 + 1.6 * rand() : -(thick + 0.5 + rand()), along = 2.4 * (rand() - 0.5), size = 0.45 + 0.6 * rand();
        m.add(fieldstone(k, size, 0.65 * size, STONE[k % STONE.length]).place(6 * rand(), [bx + (bx / far) * out - (by / far) * along, by + (by / far) * out + (bx / far) * along, -0.1]));
    }
    return m;
}

/** A round part about the z axis from rows of [z, radius], with `sides` sides, a colour per band of rows. */
function turned(m: Model, rows: [number, number][], sides: number, colours: number[]): Model {
    const foot = Array.from({ length: sides }, (_, k): [number, number] => [Math.cos((2 * Math.PI * (k + 0.5)) / sides), Math.sin((2 * Math.PI * (k + 0.5)) / sides)]);
    return m.loft(foot, rows, band => colours[Math.min(band, colours.length - 1)], colours[0], colours[colours.length - 1]);
}

const WHITE = rgb('#f2efe6'), DARK = rgb('#2f3a40'), CAP = rgb('#b8402e'), BASE = rgb('#8f8b82'), GLASS = rgb('#ffe9b0');
/** The tower's radius at its foot and at its gallery, the gallery's and the lantern's, for a footprint of radius r. */
const sizes = (width: number, depth: number) => {
    const r = Math.max(1.5, Math.min(width, depth) / 2);
    return { foot: 1.41 * r, head: 1.13 * r, gallery: 1.5 * r, lantern: 0.78 * r };
};

/**
 * A white lighthouse on the middle of its footprint: a tapering tower on a stone foot with a door toward −y and
 * windows, a gallery with a rail, the bars of the lantern, and a red cap. `lighthouseLight` is the lantern's glass.
 */
export function lighthouse(width: number, depth: number, height: number): Model {
    const { foot, head, gallery, lantern } = sizes(width, depth), m = new Model(), at = (z: number) => foot + (head - foot) * (z / height);
    turned(m, [[-footing(foot), foot + 0.12], [0.9, foot + 0.1]], 12, [BASE]);
    turned(m, [[0.9, at(0.9)], [height, head]], 12, [WHITE]);
    // The door and three windows, each on a face of the tower: 0.966 of its radius from the axis.
    const face = (z: number) => -0.966 * at(z) - 0.02;
    m.box([0, face(1.2), 1.35], [0.9, 0.06, 1.9], DARK).prism([[-0.45, 2.3], [0.45, 2.3], [0, 2.75]], face(2.3) - 0.03, face(2.3) + 0.03, DARK);
    for (const share of [0.45, 0.65, 0.85]) m.box([0, face(share * height), share * height], [0.4, 0.06, 0.8], DARK);
    turned(m, [[height, gallery], [height + 0.3, gallery]], 12, [DARK]);
    for (let k = 0; k < 12; k++) {
        const a = (Math.PI * (k + 0.5)) / 6, b = (Math.PI * (k + 1.5)) / 6, p = (t: number, z: number): Vec => [0.97 * gallery * Math.cos(t), 0.97 * gallery * Math.sin(t), z];
        m.beam(p(a, height + 0.3), p(a, height + 1.25), 0.06, DARK, { sides: 3 }).beam(p(a, height + 1.25), p(b, height + 1.25), 0.07, DARK, { sides: 3 });
    }
    for (let k = 0; k < 5; k++) {
        const a = (2 * Math.PI * (k + 0.5)) / 5;
        m.beam([lantern * Math.cos(a), lantern * Math.sin(a), height + 0.3], [lantern * Math.cos(a), lantern * Math.sin(a), height + 1.9], 0.1, DARK, { sides: 3 });
    }
    turned(m, [[height + 1.9, 1.2 * lantern], [height + 2.1, 1.15 * lantern], [height + 3, 0.03]], 10, [CAP]);
    return m.box([0, 0, height + 3.2], [0.12, 0.12, 0.5], DARK);
}

/** The glass of a lighthouse's lantern, which shines. */
export function lighthouseLight(width: number, depth: number, height: number): Model {
    return turned(new Model(), [[height + 0.3, 0.97 * sizes(width, depth).lantern], [height + 1.9, 0.97 * sizes(width, depth).lantern]], 5, [GLASS]);
}
