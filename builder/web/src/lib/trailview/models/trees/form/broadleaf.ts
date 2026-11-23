// A broadleaf's crown: soft lumps on limbs, leafy or bare, and the bends of the whole tree.

import type { Season } from '../../../rules/regions';
import { computeNormals, GOLDEN, hash, hex, lathe, limb, lump, merge, paint, place, ramp, random, sheet, smooth, soft, tube, turnFromTo, UP, v3, type Geometry, type V } from '../../shape';
import type { Broadleaf, Built, Lod } from '../../trees';
import { trunk } from './bark';
import { dragon, palm, rods } from './odd';
import { berries, bough, clamp, DETAIL, hang, heads, lerp, lowest, rounded, stem, twig, TWIGS, vertices } from './parts';

/** The branches that a bare broadleaf limb forks into. */
const BRANCHES = 3;

/** A part without the triangles whose corners `hidden` all picks, and without the corners that no triangle is left on. */
function shown(geo: Geometry, hidden: (p: V) => boolean): Geometry {
    const p = v3(), gone = Array.from({ length: geo.position.length / 3 }, (_, k) => hidden(p.from(geo.position, k)));
    const at = new Map<number, number>(), out: Geometry = { position: [], normal: [], color: [], index: [] };
    for (let t = 0; t < geo.index.length; t += 3) {
        const corners = geo.index.slice(t, t + 3);
        if (corners.every(k => gone[k])) continue;
        for (const k of corners) {
            if (!at.has(k)) {
                at.set(k, at.size);
                for (const name of ['position', 'normal', 'color'] as const) out[name].push(...geo[name].slice(3 * k, 3 * k + 3));
            }
            out.index.push(at.get(k)!);
        }
    }
    return out;
}

/** Crown lumps in the crown's own frame, radius 1: a core, an optional top and a ring around. */
function lumps(count: number, rand: () => number): { at: V; r: number }[] {
    const out = [{ at: v3(), r: 0.72 }];
    if (count >= 6) out.push({ at: v3(0, 0, 0.5), r: 0.5 });
    const ring = count - out.length;
    for (let k = 0; k < ring; k++) {
        const a = (k / ring) * 2 * Math.PI + rand() * 0.5;
        const z = (k % 2 ? 0.2 : -0.15) + (rand() - 0.5) * 0.15;
        out.push({ at: v3(0.55 * Math.cos(a), 0.55 * Math.sin(a), z), r: 0.48 + rand() * 0.08 });
    }
    return out;
}

/**
 * A shaped crown in its own frame, from -1 to 1 high: a lump on top, first, and the others around the outline from low
 * to high, a part of a turn on each, at heights that share the outline's area evenly. `size` times as large as lumps
 * that close the crown.
 */
function clumps(shape: (t: number) => number, count: number, rand: () => number, size: number): { at: V; r: number }[] {
    const ring = count - 1, r0 = (size * 1.75) / Math.sqrt(count), STEPS = 24;
    const wide = Array.from({ length: STEPS }, (_, i) => shape((i + 0.5) / STEPS)), total = wide.reduce((sum, w) => sum + w, 0), widest = Math.max(...wide);
    const top = clamp(1.3 * shape(0.85), 0.6 * r0, 1.1 * r0), out = [{ at: v3(0, 0, 1 - top), r: top }];
    for (let k = 0, i = 0, below = 0; k < ring; k++) {
        const want = ((k + 0.5) / ring) * total;
        while (below + wide[i] < want) below += wide[i++];
        const t = lerp(0.08, 0.82, (i + (want - below) / wide[i]) / STEPS), edge = shape(t);
        const r = Math.min(r0 * lerp(0.7, 1.1, edge / widest) * (0.85 + 0.3 * rand()), Math.max(edge, 0.5 * r0)), a = k * GOLDEN + 0.5 * rand();
        // A lump stands a little out of the outline, a large one no further than a small one.
        const d = Math.max(0, edge - r + 0.12);
        out.push({ at: v3(d * Math.cos(a), d * Math.sin(a), Math.max(2 * t - 1, 0.8 * r - 1)), r });
    }
    return out;
}

/**
 * An open crown in its own frame, radius 1: a puff on top and smaller ones around it, with sky
 * between. More puffs are smaller; fewer huddle in a little, as a far tree has no limbs to hold them.
 */
function puffs(count: number, rand: () => number, dense = 1, layers = 0): { at: V; r: number }[] {
    const out = [{ at: v3(0.1, 0, 0.5), r: 0.5 }];
    const ring = count - 1;
    for (let k = 0; k < ring; k++) {
        if (layers) {
            // Whorl after whorl takes a puff, each a part of a turn on from the one below.
            const j = k % layers, n = Math.ceil((ring - j) / layers), z = lerp(-0.75, 0.1, j / Math.max(1, layers - 1));
            const a = ((Math.floor(k / layers) + 0.5 * j + 0.3 * rand()) / n) * 2 * Math.PI, d = clamp(ring / 8, 0.5, 1) * 0.8 * Math.sqrt(1 - ((z + 0.75) / 1.9) ** 2);
            out.push({ at: v3(d * Math.cos(a), d * Math.sin(a), z + 0.1 * (rand() - 0.5)), r: dense * Math.min(1, Math.sqrt(8 / ring)) * (0.34 + 0.06 * rand()) });
            continue;
        }
        const a = (k / ring) * 2 * Math.PI + rand() * 0.6;
        const d = clamp(ring / 5, 0.85, 1) * (0.62 + 0.25 * rand());
        out.push({ at: v3(d * Math.cos(a), d * Math.sin(a), -0.5 + 0.9 * rand()), r: dense * Math.min(1, Math.sqrt(5 / ring)) * (0.32 + 0.1 * rand()) });
    }
    return out;
}

/**
 * A crown of soft lumps on a trunk. A bare crown is a limb into each lump, its branches and twigs
 * out to the lump's far side, with sky between them, so its outline stays at every distance.
 */
export function broadleaf(spec: Broadleaf, season: Season, lod: Lod, seed: number, withHeads: boolean): Built {
    const rand = random(seed);
    const [cz, rx, ry, rz] = spec.crown;
    const toTree = (p: V) => v3(p.x * rx, p.y * ry, cz + p.z * rz);
    const leaves = spec.leaves[season];
    const bare = leaves === 'bare';
    // A bare crown grows the limbs of the nearest level at every level.
    const { lumps: count, ico: corners } = DETAIL[bare ? 0 : lod];
    // The lumps of a shaped crown: a leafy one has fewer, larger ones a level. Its far tier is simplified from its far
    // level, whose lumps are round enough to stay lumps.
    const clumped = spec.shape && Math.round((spec.clumps ?? 12) * (bare ? 1 : [1, 0.75, 0.55][lod]));
    const ico = clumped ? Math.max(1, corners) : corners;
    const [deep, mid, tip] = ramp(bare ? spec.twigs : leaves);
    const flecks = spec.flecks?.[season];
    const foliage = (p: V, n: V) => {
        const c = deep
            .clone()
            .lerp(mid, 0.7 * smooth(-0.6, 0.3, n.z) + 0.3 * smooth(-0.8, 0.6, p.z))
            .lerp(tip, 0.7 * smooth(0.2, 0.9, n.z) * smooth(-0.2, 0.8, p.z));
        // Under its heads a crown takes a little of their colour on its upper sides, which a far tier keeps.
        return flecks && spec.heads ? c.lerp(hex(flecks[0]), 0.5 * flecks[1] * smooth(0, 0.8, n.z)) : c;
    };
    // A palm or a dragon tree grows its crown from the trunk's top.
    if (spec.fronds || spec.branching || spec.rods) {
        const crown = spec.fronds ? palm(spec, lod, [deep, mid, tip], rand)
            : spec.rods ? rods(spec, lod, bare, foliage, [mid, tip], flecks ? [hex(flecks[0]), flecks[1]] : [], rand, seed, withHeads)
            : dragon(spec, lod, [deep, mid, tip], rand);
        // A far shrub's rods hide under its plumes, so it has one trunk as stout as all of them, up into the plumes.
        const bole = spec.rods && lod === 2 ? trunk({ ...spec, trunk: 1.4 * spec.trunk }, 0.35 * (cz + rz), lod, seed) : trunk(spec, spec.fork, lod, seed);
        return { geo: merge([bole, ...crown]), trunk: vertices([bole]), crown: lowest(crown) };
    }
    // A far crooked tree has a puff more, as it keeps its limbs.
    const whorled = spec.layers && [Math.round(3.6 * spec.layers), Math.round(2.4 * spec.layers), 4][lod] + 1;
    const crown = clumped ? clumps(spec.shape!, clumped, rand, spec.open ? (spec.dense ?? 0.6) : 1) : spec.open ? puffs((whorled || count + (spec.pads && !lod ? 3 : 0)) + (spec.crook && lod === 2 ? 1 : 0), rand, spec.dense, spec.layers) : lumps(count, rand);
    const parts: Geometry[] = [];
    // An open crown's trunk runs up into its top puff, which no limb reaches.
    const high = spec.open ? cz + (spec.layers ? 0.5 : 0.3) * rz : spec.fork + 0.3 * rz;
    // A shaped crown's trunk ends where its limbs leave: under the top lump with `stagger`, in the crown's middle where
    // it has stems, else at the fork. A far open one runs to the middle, as its limbs are too thin to keep.
    const top = !clumped ? high : spec.stagger ? (spec.stems ? cz : toTree(crown[0].at).z - crown[0].r * rz) : spec.open && lod === 2 && !bare ? cz : spec.fork + 0.1 * rz;
    // A far tree's stems hide under its crown, so it has one trunk as stout as all of them.
    const stems = lod < 2 ? (spec.stems ?? 1) : 1;
    const stout = { ...spec, trunk: spec.trunk * Math.sqrt((spec.stems ?? 1) / stems) };
    const leaders: { out: V; foot: V; lean: V }[] = [];
    for (let s = 0; spec.trunk && s < stems; s++) {
        const a = (s / stems) * 2 * Math.PI;
        // Stems lean apart from one stool, their feet a little apart.
        const out = stems > 1 ? v3(Math.cos(a), Math.sin(a), 0) : v3();
        const lean = out.clone().multiplyScalar(0.3).setZ(1).normalize();
        leaders.push({ out: out.clone(), foot: out.clone().multiplyScalar(1.2 * spec.trunk), lean });
        parts.push(place(trunk(stout, top, lod, seed + s, spec.crook ? [5, 3, clumped ? 1 : 3][lod] : 1), { at: out.multiplyScalar(1.2 * spec.trunk), turn: turnFromTo(UP, lean) }));
    }
    // Where a limb towards `to` leaves the trunk at height z: the middle, or with `leaders` the stem on that side.
    const leave = (to: V, z: number) => {
        if (!spec.leaders || !leaders.length) return v3(0, 0, z);
        const { foot, lean } = leaders.reduce((best, stem) => (stem.out.dot(to) > best.out.dot(to) ? stem : best));
        return foot.clone().addScaledVector(lean, Math.min(z, 0.95 * top) / lean.z);
    };
    const [foot, bole] = spec.bark.map(hex);
    const bark = (p: V) => foot.clone().lerp(bole, smooth(0, spec.fork, p.z));
    // The limbs of a shaped crown, by lump: a main limb leaves the trunk, another forks from the main limb on its side.
    // A bare limb runs to its lump's middle. The top lump's comes last, as it forks from a main limb.
    const MAINS = 5;
    const main = (n: number) => !!spec.stagger || (n > 0 && n <= MAINS);
    const limbs: ((s: number) => V)[] = [];
    for (const n of clumped ? [...crown.keys()].slice(1).concat(0) : []) {
        const end = toTree(crown[n].at.clone().multiplyScalar(bare ? 1 : 0.8)), level = Math.hypot(end.x, end.y);
        const from = spec.stagger ? leave(end, n ? clamp(end.z - 0.6 * level, spec.fork, top) : top)
            : main(n) ? leave(end, spec.fork)
            : limbs.slice(1, MAINS + 1).reduce((best, limb) => (limb(1).setZ(0).normalize().dot(end) > best(1).setZ(0).normalize().dot(end) ? limb : best))(0.45);
        const length = from.distanceTo(end), side = level > 1e-6 ? v3(-end.y, end.x, 0).normalize() : v3(1, 0, 0);
        // The limb's bend: up or down by `bow`, and a little to one side.
        const knee = from.clone().lerp(end, 0.5).addScaledVector(UP, 0.5 * (spec.bow ?? 0) * length).addScaledVector(side, 0.12 * length * (hash(end, seed) - 0.5));
        limbs[n] = (s: number) => from.clone().multiplyScalar((1 - s) ** 2).addScaledVector(knee, 2 * s * (1 - s)).addScaledVector(end, s * s);
    }
    if (bare) {
        const twigs = sheet();
        const [dead, share] = flecks ? [hex(flecks[0]), flecks[1]] : [mid, 0];
        const weep = spec.weep ?? 0;
        let kept = 0;
        for (const [n, { at, r }] of crown.entries()) {
            const out = at.clone().normalize(), centre = toTree(at), rise = rand();
            // No limb leaves the trunk above its top, so the top lump of an open crown grows from it.
            // Without a stagger the limbs leave over the lowest part of the way from the fork to their lumps.
            const fork = spec.stagger ? Math.max(spec.fork, centre.z - 0.6 * Math.hypot(centre.x, centre.y)) : lerp(spec.fork, Math.max(spec.fork, centre.z), 0.3 * rise);
            const from = leave(centre, Math.min(top, fork));
            if (clumped) stem(parts, twigs, (lod < 2 ? [0, 0.33, 0.67, 1] : [0, 0.5, 1]).map(limbs[n]), (main(n) ? 0.45 : 0.3) * spec.trunk, spec.trunk * 0.15, bark, DETAIL[lod].limbs, n);
            else bough(parts, twigs, from, centre, spec.trunk * 0.45, bark, DETAIL[lod].limbs, n, spec.trunk * 0.2);
            // The limb forks into branches that rise as they spread, and short twigs fan from their
            // outer parts to the lump's far side: a tree's tapering fork, not spikes from one point.
            for (let b = 0; b < BRANCHES; b++) {
                const way = v3(rand() - 0.5, rand() - 0.5, rand() - 0.2 - weep).multiplyScalar(1.6).add(out).normalize();
                const end = at.clone().addScaledVector(way, (clumped ? 0.85 : 0.6) * r);
                bough(parts, twigs, centre, toTree(end), spec.trunk * 0.2, bark, lod ? 0 : 4, BRANCHES * n + b, spec.trunk * 0.04);
                // A shaped crown's twigs fork in threes, at the branch's end and along it.
                for (let k = 0, fans = Math.round(TWIGS / crown.length / BRANCHES / 3); clumped && k < fans; k++) {
                    const root = at.clone().lerp(end, 1 - (0.7 * (k + 0.5 * rand())) / fans);
                    const along = v3(rand() - 0.5, rand() - 0.5, rand() - 0.3 - weep).add(way).normalize(), across = v3(rand() - 0.5, rand() - 0.5, rand() - 0.5).cross(along).normalize();
                    const colours = rand() < share ? [dead, dead] : [mid, tip], length = (0.4 + 0.4 * rand()) * r;
                    for (const fork of [0, -1, 1]) {
                        const reach = root.clone().addScaledVector(along.clone().addScaledVector(across, 0.55 * fork).normalize(), length * (fork ? 0.8 : 1));
                        reach.z -= weep * rand() * r;
                        twig(twigs, toTree(root), toTree(reach), colours, rand, lod, kept++);
                        // A catkin hangs from a twig, as many on the crown as it has bunches.
                        if (flecks && !lod && Math.floor((kept * (spec.bunches ?? 0)) / TWIGS) > Math.floor(((kept - 1) * (spec.bunches ?? 0)) / TWIGS)) {
                            const size = 0.035 + 0.03 * hash(reach, seed);
                            parts.push(place(paint(soft(lathe(BUNCH, 4, 2)), (q) => dead.clone().multiplyScalar(0.75 + 0.35 * smooth(-1, 0, q.z))), { at: toTree(root.clone().lerp(reach, 0.6)), scale: [0.16 * size, 0.16 * size, size] }));
                        }
                    }
                }
                if (clumped) continue;
                for (let k = 0; k < TWIGS / crown.length / BRANCHES; k++) {
                    const root = at.clone().lerp(end, 0.3 + 0.7 * rand());
                    const reach = root.clone().addScaledVector(v3(rand() - 0.5, rand() - 0.5, rand() - 0.3 - weep).add(way).normalize(), (0.2 + 0.25 * rand()) * r);
                    // A weeping tree's twigs hang below.
                    reach.z -= weep * rand() * r;
                    twig(twigs, toTree(root), toTree(reach), rand() < share ? [dead, dead] : [mid, tip], rand, lod, kept++);
                }
            }
        }
        parts.push(twigs.build());
    } else {
        // A closed shaped crown has a core inside its lumps.
        if (clumped && !spec.open) {
            const core = soft(lathe([[0, -0.9], ...Array.from({ length: 5 }, (_, j): [number, number] => [0.7 * spec.shape!(0.1 + 0.2 * j), -0.8 + 0.4 * j]), [0, 0.9]], [8, 6, 4][lod], [6, 4, 3][lod]));
            parts.push(place(paint(core, (p, n) => foliage(p, n).multiplyScalar(0.85)), { at: [0, 0, cz], scale: [rx, ry, rz] }));
        }
        // A shaped crown's lumps are less tall in a crown taller than wide, and taller in a wide one, so they are nearly round.
        const tall = clumped ? clamp((rx / rz) ** 0.6, 0.6, 1.3) : 1;
        crown.forEach(({ at, r }, k) => {
            // Each lump a little lighter or darker, so the crown does not read as one blob.
            const tone = 0.9 + 0.2 * rand();
            // A flat pad needs fewer corners than a round puff, and so does a small puff that swells.
            const puff = soft(lump(r, spec.pads || ((spec.billows || clumped) && r < 0.44) ? Math.min(ico, 2) : ico), 0.04, 4, seed + k);
            // A far lump has too few corners to swell.
            const swell = spec.billows && ico > 1 ? billow(puff, r, spec.billows, seed + k) : undefined;
            if (spec.weep) hang(puff, r, spec.weep);
            if (spec.pads) pad(puff, r);
            let v = 0;
            // A whorl's puff is no taller in a crown taller than wide; a far one has too few corners for a pad, and is flat.
            const whole = paint(place(puff, { at, scale: [1, 1, spec.layers ? Math.min(ico < 2 ? 0.6 : 1, rx / rz) : tall] }), (p, n) => foliage(p, n).multiplyScalar(tone * (swell ? 0.8 + 0.4 * swell[v++] : 1)));
            // A shaped crown's lump keeps the skin that shows: not what stands in another lump or in the core. A lump is
            // less deep where it swells between tufts or hangs to a point.
            const sunk = (swell ? 0.62 : 0.88) * (1 - 0.5 * (spec.weep ?? 0));
            const within = (other: V, p: V) => Math.hypot(p.x - other.x, p.y - other.y, (p.z - other.z) / tall);
            const geo = clumped ? shown(whole, p => crown.some((other, j) => j !== k && within(other.at, p) < sunk * other.r) || (!spec.open && Math.abs(p.z) < 0.75 && Math.hypot(p.x, p.y) < 0.55 * spec.shape!((p.z + 1) / 2))) : whole;
            parts.push(place(clumped ? rounded(geo, 0.3) : geo, { at: [0, 0, cz], scale: [rx, ry, rz] }));
        });
        // Whether a point stands within `share` of the radius of a lump other than `own`.
        const inside = (share: number) => (p: V, own: number) => crown.some((other, k) => k !== own && other.at.distanceTo(v3(p.x / rx, p.y / ry, (p.z - cz) / rz)) < share * other.r);
        if (spec.berries && !lod) parts.push(...berries(parts.slice(-crown.length), spec.berries, rand, inside(0.75)));
        if (spec.heads && flecks && !lod && withHeads) parts.push(...heads(parts.slice(-crown.length), spec.heads, flecks, mid, rand, inside(1)));
        // A bunch hangs from the side or the underside of a lump, where no other lump hides it.
        for (let k = 0, tries = 0; flecks && lod === 0 && k < (spec.bunches ?? 0) && tries < 1000; tries++) {
            const { at, r } = crown[Math.floor(rand() * crown.length)];
            const a = 2 * Math.PI * rand(), up = -0.75 + 0.9 * rand(), level = Math.sqrt(1 - up * up);
            // A lump that swells is less deep between its tufts.
            const p = at.clone().addScaledVector(v3(level * Math.cos(a), level * Math.sin(a), up * tall), (clumped && spec.billows ? 0.72 : 0.97) * r);
            if (crown.some(other => other.at !== at && other.at.distanceTo(p) < other.r)) continue;
            const size = 0.05 + 0.025 * rand(), fruit = hex(flecks[0]);
            const bunch = paint(soft(lathe(BUNCH, 5, 3)), (q) => mid.clone().lerp(fruit, 0.25 + 0.6 * smooth(0.1, -0.7, q.z)));
            parts.push(place(bunch, { at: toTree(p), rot: [0, 0, a], scale: [0.26 * size, 0.26 * size, size] }));
            k++;
        }
        // Limbs run from the fork into the outer lumps. An open crown keeps them a level further out,
        // so its puffs never float, and a crooked one at every level, as its puffs stand far from the trunk.
        if (clumped) {
            for (const [n, limb] of spec.trunk && lod < 2 ? limbs.entries() : []) parts.push(tube((main(n) ? [0, 0.33, 0.67, 1] : [0, 0.5, 1]).map(limb), (main(n) ? 0.45 : 0.3) * spec.trunk, 0.2 * spec.trunk, 4, bark));
        } else if (spec.trunk && (lod === 0 || (spec.open && lod === 1) || spec.crook)) {
            for (const { at } of crown.slice(1)) {
                const end = toTree(at.clone().multiplyScalar(0.8));
                const from = leave(end, spec.stagger ? Math.max(spec.fork, end.z - 0.6 * Math.hypot(end.x, end.y)) : spec.fork);
                if (spec.crook) {
                    // The limb leaves the trunk nearly level and turns up under its puff.
                    const knee = from.clone().lerp(end, 0.6).setZ(lerp(from.z, end.z, 0.25));
                    parts.push(tube(lod < 2 ? [from, knee, end] : [from, end], spec.trunk * 0.45, spec.trunk * 0.3, [6, 4, 3][lod], bark));
                } else parts.push(limb(from.toArray(), end.toArray(), spec.trunk * 0.45, bark, 0, lod ? 4 : 6));
            }
        }
    }
    const geo = merge(parts);
    if (spec.crook) crook(geo, spec.crook);
    if (spec.sweep) sweep(geo, spec.sweep);
    const trunks = spec.trunk ? stems : 0;
    return { geo, trunk: vertices(parts.slice(0, trunks)), crown: trunks ? lowest(parts.slice(trunks)) : 0 };
}

// A hanging bunch or catkin, 1 long below the point that it hangs from.
const BUNCH: [number, number][] = [[0, -1], [0.9, -0.62], [0.75, -0.2], [0, 0.12]];

/** Bends a tree from side to side on its way up, by `amount` tree heights; its foot stays. */
function crook(geo: Geometry, amount: number): void {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) {
        const k = amount * smooth(0, 0.25, p[i + 2]);
        p[i] += k * Math.sin(9 * p[i + 2]);
        p[i + 1] += k * Math.sin(6.5 * p[i + 2] + 2);
    }
    computeNormals(geo);
}

/**
 * Swells a lump of radius r into `count` round tufts that stand out of a core, most of them on its upper side, as
 * balls that share one skin. Returns how far each vertex swells, 0 to 1.
 */
function billow(geo: Geometry, r: number, count: number, seed: number): number[] {
    const p = geo.position, d = v3(), swell: number[] = [];
    const tufts = Array.from({ length: count }, (_, k) => {
        const z = lerp(0.9, -0.4, (k + 0.5) / count), level = Math.sqrt(1 - z * z), a = k * GOLDEN + seed;
        return v3(level * Math.cos(a), level * Math.sin(a), z);
    });
    // A tuft is a ball of radius 0.5 r that stands 0.55 r out of the centre; the core has radius 0.7 r.
    for (let k = 0; k < p.length / 3; k++) {
        const far = Math.max(0.7, ...tufts.map(c => {
            const cos = d.from(p, k).normalize().dot(c), under = 0.25 - 0.3025 * (1 - cos * cos);
            return cos > 0 && under > 0 ? 0.55 * cos + Math.sqrt(under) : 0;
        }));
        d.multiplyScalar(r * far);
        p.splice(3 * k, 3, d.x, d.y, d.z);
        swell.push((far - 0.7) / 0.35);
    }
    computeNormals(geo);
    return swell;
}

/** Squashes a lump of radius r into a pad with a flat top and round sides. */
function pad(geo: Geometry, r: number): void {
    const p = geo.position;
    for (let i = 2; i < p.length; i += 3) p[i] -= 0.4 * r * smooth(0.2 * r, r, p[i]);
    computeNormals(geo);
}

/** Shears a tree downwind, towards +x, more the higher it reaches. */
function sweep(geo: Geometry, amount: number): void {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) p[i] += amount * p[i + 2] ** 2;
    computeNormals(geo);
}
