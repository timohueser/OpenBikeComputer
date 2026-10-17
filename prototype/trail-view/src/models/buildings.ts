/**
 * Buildings fitted to a footprint rectangle: centred on the origin, the ridge along the longer
 * side and z up from the ground floor. Windows and doors are painted cells of the wall grid, not
 * geometry. Faces are flat shaded.
 */

import * as THREE from 'three';
import { hex, random } from './shape';

export type BuildingKind = 'cottage' | 'house' | 'farmhouse' | 'barn' | 'block';

export interface BuildingStyle {
  name: string;
  /** Roof pitch range in degrees. */
  pitch: [number, number];
  /** Roof overhang past the long walls and past the gable ends, in metres. */
  eave: number;
  verge: number;
  /** Share of the roof height hipped at the gable ends: 0 is a gable, 1 a full hip. */
  hip: number;
  /** Which walls are timber rather than render. */
  timber: 'none' | 'upper' | 'all';
  shutters: boolean;
  chimneys: 0 | 1 | 2;
  /** Each building takes one colour from each list. */
  palette: Record<'render' | 'wood' | 'roof' | 'door' | 'shutter', string[]> & { plinth: string; chimney: string };
}

export interface Footprint {
  /** Along x and along y, in metres. */
  width: number;
  depth: number;
  /** Wall height to the eaves; by default the kind's storeys. */
  wall?: number;
  /** Roof pitch in degrees; by default a seeded value in the style's range. */
  pitch?: number;
  seed?: number;
  /** Walls without windows and doors, for a building too far away to show them. */
  plain?: boolean;
}

interface Kind {
  name: string;
  storey: number;
  storeys: number;
  /** Window width and the spacing of windows along a wall, and the door width. */
  window: [number, number];
  door: number;
  /** Sill and lintel heights as shares of a storey; the door reaches the lintel. */
  opening?: [number, number];
  flat?: boolean;
}

const KINDS: Record<BuildingKind, Kind> = {
  cottage: { name: 'Cottage', storey: 2.6, storeys: 1, window: [0.8, 2.2], door: 0.9 },
  house: { name: 'House', storey: 2.7, storeys: 2, window: [1, 2.6], door: 1 },
  farmhouse: { name: 'Farmhouse', storey: 2.8, storeys: 2, window: [0.9, 2.2], door: 1.2 },
  barn: { name: 'Barn or shed', storey: 4.2, storeys: 1, window: [0.7, 6], door: 3, opening: [0.55, 0.75] },
  block: { name: 'Block', storey: 3, storeys: 4, window: [1.4, 2.8], door: 1.6, flat: true },
};

export const BUILDING_KINDS = Object.keys(KINDS) as BuildingKind[];
export const buildingLabel = (kind: BuildingKind) => KINDS[kind].name;

const GLASS = '#36424c';
const FLAT_ROOF = '#7a766e';
const ROOF_THICKNESS = 0.3;
const PLINTH = 0.35;

/** Flat-shaded triangles with one colour each. */
class Faces {
  private readonly position: number[] = [];
  private readonly color: number[] = [];

  /** A convex polygon, counter-clockwise seen from outside. Repeated corners are skipped. */
  poly(points: THREE.Vector3[], c: THREE.Color): void {
    const corners = points.filter((p, i) => p.distanceTo(points[(i + 1) % points.length]) > 1e-6);
    for (let i = 1; i + 1 < corners.length; i++) {
      for (const q of [corners[0], corners[i], corners[i + 1]]) {
        this.position.push(q.x, q.y, q.z);
        this.color.push(c.r, c.g, c.b);
      }
    }
  }

  /** A roof plane given by its underside, counter-clockwise from above, thickened upwards. */
  slab(under: THREE.Vector3[], top: THREE.Color, edge: THREE.Color, soffit: THREE.Color): void {
    const over = under.map((p) => p.clone().setZ(p.z + ROOF_THICKNESS));
    this.poly(over, top);
    this.poly([...under].reverse(), soffit);
    under.forEach((p, i) => {
      const j = (i + 1) % under.length;
      this.poly([p, under[j], over[j], over[i]], edge);
    });
  }

  /** A box standing on z0, without a floor. */
  box(cx: number, cy: number, sx: number, sy: number, z0: number, z1: number, side: THREE.Color, top: THREE.Color): void {
    const ring = [[-1, -1], [1, -1], [1, 1], [-1, 1]].map(([x, y]) => [cx + (x * sx) / 2, cy + (y * sy) / 2]);
    ring.forEach(([x, y], i) => {
      const [nx, ny] = ring[(i + 1) % 4];
      this.poly([new THREE.Vector3(x, y, z0), new THREE.Vector3(nx, ny, z0), new THREE.Vector3(nx, ny, z1), new THREE.Vector3(x, y, z1)], side);
    });
    this.poly(ring.map(([x, y]) => new THREE.Vector3(x, y, z1)), top);
  }

  geometry(): THREE.BufferGeometry {
    const geo = new THREE.BufferGeometry()
      .setAttribute('position', new THREE.Float32BufferAttribute(this.position, 3))
      .setAttribute('color', new THREE.Float32BufferAttribute(this.color, 3));
    geo.computeVertexNormals();
    return geo;
  }
}

/** Sorted cut positions inside (0, end), with both ends. */
function cuts(values: number[], end: number): number[] {
  const inside = values.filter((v) => v > 1e-3 && v < end - 1e-3).sort((a, b) => a - b);
  return [0, ...inside.filter((v, i) => i === 0 || v - inside[i - 1] > 1e-3), end];
}

export function building(kind: BuildingKind, style: BuildingStyle, { width, depth, wall, pitch, seed = 1, plain = false }: Footprint): THREE.BufferGeometry {
  const k = KINDS[kind];
  const rand = random(seed);
  const pick = (list: string[]) => hex(list[Math.floor(rand() * list.length)]);
  const { palette } = style;
  const render = pick(palette.render);
  const wood = pick(palette.wood);
  const roof = k.flat ? hex(FLAT_ROOF) : pick(palette.roof);
  const [sill, lintel] = k.opening ?? [0.35, 0.8];
  // A barn door is planks, darker than the barn's own boards.
  const door = kind === 'barn' ? wood.clone().multiplyScalar(0.55) : pick(palette.door);
  const shutter = pick(palette.shutter);
  const [plinth, chimney, glass] = [hex(palette.plinth), hex(palette.chimney), hex(GLASS)];
  const timber = kind === 'block' ? 'none' : kind === 'barn' && style.timber !== 'none' ? 'all' : style.timber;
  const shutters = style.shutters && kind !== 'barn';

  const [L, W] = width >= depth ? [width, depth] : [depth, width];
  const height = wall ?? k.storey * k.storeys;
  const storeys = Math.max(1, Math.round(height / k.storey));
  const storey = height / storeys;
  const [ww, spacing] = k.window;
  const out = new Faces();

  // Walls counter-clockwise from above, the first one the front with the door.
  const corners = [[-L / 2, -W / 2], [L / 2, -W / 2], [L / 2, W / 2], [-L / 2, W / 2]];
  corners.forEach(([ax, ay], w) => {
    const [bx, by] = corners[(w + 1) % 4];
    const len = Math.hypot(bx - ax, by - ay);
    const at = (u: number, z: number) => new THREE.Vector3(ax + ((bx - ax) * u) / len, ay + ((by - ay) * u) / len, z);
    const n = Math.max(1, Math.floor(len / spacing));
    const windows = Array.from({ length: n }, (_, i) => (len * (i + 0.5)) / n);
    const front = w === 0;
    const doorAt = n % 2 ? windows[(n - 1) / 2] : len / 2;
    const shutterHalf = ww / 2 + (shutters ? 0.45 * ww : 0);
    const edges = windows.flatMap((c) => [c - ww / 2, c + ww / 2, c - shutterHalf, c + shutterHalf]);
    const us = plain ? [0, len] : cuts(front ? [...edges, doorAt - k.door / 2, doorAt + k.door / 2] : edges, len);
    const levels = Array.from({ length: storeys }, (_, s) => [s * storey, (s + sill) * storey, (s + lintel) * storey]).flat();
    const zs = cuts(plain ? [PLINTH, storey] : [PLINTH, ...levels], height);
    const cell = (u: number, z: number) => {
      const s = Math.min(storeys - 1, Math.floor(z / storey));
      const opening = !plain && z > (s + sill) * storey && z < (s + lintel) * storey;
      if (!plain && front && Math.abs(u - doorAt) < k.door / 2 && z < lintel * storey) return door;
      if (opening && windows.some((c) => Math.abs(u - c) < ww / 2)) return glass;
      if (opening && windows.some((c) => Math.abs(u - c) < shutterHalf)) return shutter;
      if (z < PLINTH) return plinth;
      return timber === 'all' || (timber === 'upper' && z > storey) ? wood : render;
    };
    for (let i = 0; i + 1 < us.length; i++) {
      for (let j = 0; j + 1 < zs.length; j++) {
        const [u0, u1, z0, z1] = [us[i], us[i + 1], zs[j], zs[j + 1]];
        out.poly([at(u0, z0), at(u1, z0), at(u1, z1), at(u0, z1)], cell((u0 + u1) / 2, (z0 + z1) / 2));
      }
    }
  });

  const v = (x: number, y: number, z: number) => new THREE.Vector3(x, y, z);
  const soffit = wood.clone().multiplyScalar(0.75);
  const edge = roof.clone().multiplyScalar(0.8);
  if (k.flat) {
    const [x, y] = [L / 2 + 0.2, W / 2 + 0.2];
    out.slab([v(-x, -y, height), v(x, -y, height), v(x, y, height), v(-x, y, height)], roof, edge, soffit);
  } else {
    const t = Math.tan(THREE.MathUtils.degToRad(pitch ?? THREE.MathUtils.lerp(style.pitch[0], style.pitch[1], rand())));
    const Y = W / 2 + style.eave;
    const xEnd = L / 2 + style.verge;
    const ridge = height + (W / 2) * t;
    const eave = height - style.eave * t;
    // The hipped ends rise at the roof's own pitch from the hip line to the shortened ridge.
    const hipZ = ridge - style.hip * (ridge - eave);
    const hipY = style.hip * Y;
    const xRidge = Math.max(0, xEnd - style.hip * Y);
    out.slab([v(-xEnd, -Y, eave), v(xEnd, -Y, eave), v(xEnd, -hipY, hipZ), v(xRidge, 0, ridge), v(-xRidge, 0, ridge), v(-xEnd, -hipY, hipZ)], roof, edge, soffit);
    out.slab([v(xEnd, Y, eave), v(-xEnd, Y, eave), v(-xEnd, hipY, hipZ), v(-xRidge, 0, ridge), v(xRidge, 0, ridge), v(xEnd, hipY, hipZ)], roof, edge, soffit);
    if (style.hip > 0) {
      out.slab([v(xEnd, -hipY, hipZ), v(xEnd, hipY, hipZ), v(xRidge, 0, ridge)], roof, edge, soffit);
      out.slab([v(-xEnd, hipY, hipZ), v(-xEnd, -hipY, hipZ), v(-xRidge, 0, ridge)], roof, edge, soffit);
    }
    // Gable walls reach up to the roof's underside, flat-topped where a hip covers them.
    const topZ = Math.min(ridge, hipZ + style.verge * t);
    const topY = (ridge - topZ) / t;
    const gable = timber === 'none' ? render : wood;
    for (const s of [1, -1]) {
      const x = (s * L) / 2;
      out.poly([v(x, -s * W / 2, height), v(x, s * W / 2, height), v(x, s * topY, topZ), v(x, -s * topY, topZ)], gable);
    }
    const stacks = kind === 'barn' ? [] : style.chimneys === 2 ? [-1, 1] : style.chimneys === 1 ? [1] : [];
    for (const s of stacks) {
      const x = s * Math.min(L / 2 - 0.6, Math.max(0, xRidge - 0.5));
      out.box(x, 0, 0.7, 0.55, ridge - 1, ridge + ROOF_THICKNESS + 0.9, chimney, plinth);
    }
  }

  const geo = out.geometry();
  return width >= depth ? geo : geo.rotateZ(Math.PI / 2);
}
