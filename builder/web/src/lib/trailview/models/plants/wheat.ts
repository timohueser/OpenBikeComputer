import type { Plant } from '../plant-library';
import { domed, hex, random, sheet, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/**
 * Per season: the stalks' height in crop heights, their foot and tip colours, and the ear's colour
 * or none. Winter wheat stands as young shoots from January to March, green in ear from April to
 * June and golden from July; from October the field is stubble.
 */
const SEASON: Record<Season, [height: number, stalk: Ramp, ear: string | null]> = {
    winter: [0.2, ['#3e6a2a', '#5a8a36', '#7eaa4c'], null],
    spring: [0.85, ['#46663a', '#628444', '#84a05a'], '#94aa62'],
    summer: [0.85, ['#9a7a3a', '#c4a458', '#dcc074'], '#d8b866'],
    autumn: [0.12, ['#7a6648', '#a8925e', '#c4ae78'], null],
};

const [ROWS, PER_ROW] = [4, 6];

/**
 * Wheat: a square metre of crop as one patch, four drill rows of six sheaves. A sheaf is one
 * triangle from a narrow foot to a ragged top as wide as its ears, so the patch reads as a dense
 * stand; every third sheaf lifts one ear above the rest as a diamond of two. Lit as the field, not
 * as each stalk. 40 triangles in ear.
 */
function wheat(season: Season, rand: () => number): Geometry {
    const [height, [foot, mid, tip], ear] = SEASON[season];
    const [low, high, top] = [foot, mid, tip].map(hex);
    const grain = ear ? hex(ear) : top;
    const light = domed(v3(0, 0, -2), v3(3, 3, 3), 0.5);
    const cards = sheet();
    for (let r = 0; r < ROWS; r++) {
        for (let k = 0; k < PER_ROW; k++) {
            // The same draws in every season, so a patch keeps its layout the year round.
            const [jx, jy, h, dip, turn] = [rand(), rand(), rand(), rand(), rand() * Math.PI];
            const at = v3(-0.9 + (1.8 * (k + 0.2 + 0.6 * jx)) / PER_ROW, -0.75 + 0.5 * r + 0.1 * (jy - 0.5), 0);
            const across = v3(Math.cos(turn), Math.sin(turn), 0);
            const crown = at.clone().setZ(height * (0.88 + 0.12 * h));
            const half = ear ? 0.08 : 0.025;
            const corners = [crown.clone().addScaledVector(across, half), crown.clone().addScaledVector(across, -half)];
            corners[1].z -= 0.06 * height * dip;
            cards.add([at, ...corners], [low, ear ? grain : top, (ear ? grain : top).clone().lerp(high, 0.4)], light);
            if (!ear || (r * PER_ROW + k) % 3) continue;
            // The ear hangs a little to one side when ripe.
            const end = crown.clone().add(v3(0, 0, 0.13)).addScaledVector(across, season === 'summer' ? 0.04 : 0);
            const middle = crown.clone().lerp(end, 0.45);
            const side = v3(-across.y, across.x, 0).multiplyScalar(0.025);
            cards.add([crown, middle.clone().add(side), end], [high, grain, grain], light);
            cards.add([crown, end, middle.clone().sub(side)], [high, grain, grain], light);
        }
    }
    return cards.build();
}

export default {
    name: 'Wheat',
    latin: 'Triticum aestivum',
    build: (season, seed) => wheat(season, random(seed)),
    ground: { on: ['crop'], height: [0.6, 0.9], share: 1, own: true },
    sized: true,
} satisfies Plant;
