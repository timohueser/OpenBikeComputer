/**
 * What the near pass draws besides the terrain, from the region's pack: trees within 1 km at three
 * levels of detail, shrubs, rocks and ground plants near the eye, OSM buildings as the pack's houses
 * and landmarks, and OSM walls and hedges.
 * Where each object stands comes from the data; the pack decides what it looks like.
 */

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { HABITATS, type Bake, type Building, type Habitat } from './bake';
import { building, type BuildingKind, type BuildingStyle } from './models/buildings';
import type { Pack, Region } from './regions';
import { lighthouse, ruin } from './models/landmarks';
import { HEDGE, STONE } from './palette';
import { plant, type PlantName } from './models/plants';
import { animal, type AnimalName } from './models/animals';
import { rock, type RockName } from './models/rocks';
import { random } from './models/shape';
import { LOD_RANGE_M, tree, type Lod, type Season, type TreeName } from './models/trees';
import { buildingVertex, depthFragment, objectFragment, treeVertex } from './shaders';

/** WorldCover classes as the page's palette indexes them (CLASSES in bake.py). */
const CLASS = { tree: 0, shrub: 1, grass: 2, crop: 3, bare: 5, moss: 10 } as const;

/**
 * Where each ground plant grows, how tall it stands, and the share of grid points it takes there.
 * In a region with moor, heather also grows on grass that the satellite sees as moor, and grass only
 * off it: the shader decides per plant from the same ground the terrain paints.
 */
const GROUND: Record<PlantName, { on: number[]; height: [number, number]; share: number; habitat?: 1 | -1 }> = {
  grass: { on: [CLASS.grass, CLASS.crop], height: [0.25, 0.5], share: 1, habitat: -1 },
  'alpine-flowers': { on: [CLASS.grass], height: [0.2, 0.35], share: 0.15 },
  fern: { on: [CLASS.tree], height: [0.5, 0.9], share: 0.3 },
  heather: { on: [CLASS.shrub, CLASS.moss, CLASS.bare], height: [0.2, 0.4], share: 0.4, habitat: 1 },
  'heather-bloom': { on: [CLASS.shrub, CLASS.moss], height: [0.2, 0.4], share: 0.15, habitat: 1 },
  huckleberry: { on: [CLASS.tree], height: [0.4, 0.8], share: 0.25 },
};
/** The eye stands in a clearing this wide, so it is never inside a crown. */
const CLEAR_M = 6;
/** Ground plants on a jittered 1 m grid within this many metres. */
const GROUND_RADIUS_M = 60;
/** Shrub species on half the points of a jittered grid on shrubland within this many metres. */
const SHRUB_RADIUS_M = 500;
const SHRUB_SPACING_M = 5;
const SHRUB_SHARE = 0.5;
const SHRUB_HEIGHT_M: [number, number] = [0.8, 1.8];
/**
 * Rocks on a jittered grid within this many metres, on open ground that is steep, on a ridge or
 * by the sea, as many as the region's outcrops say: the same ground the terrain paints with slabs.
 */
const ROCK_RADIUS_M = 400;
const ROCK_SPACING_M = 4;
const ROCK_HEIGHT_M: [number, number] = [0.25, 1.3];
/** Rocks this near would fill the view; small stones may lie closer. */
const ROCK_CLEAR_M = 10;
/** Habitats of open ground: a fine land cover map that says so overrules a WorldCover tree pixel. */
const TREELESS = new Set<Habitat>(['rock', 'heath', 'bog', 'bracken', 'rough', 'pasture', 'sand', 'saltmarsh']);
/** Herds graze within this many metres, each spread over HERD_SPREAD_M; an animal stands this tall. */
const HERD_RADIUS_M = 400;
const HERD_SPREAD_M = 14;
const ANIMAL_M = 0.95;
/** A crest within this many metres drops more than CREST_DROP_M within CREST_ARM_M to both sides. */
const CREST_RADIUS_M = 1500;
const CREST_ARM_M = 20;
const CREST_DROP_M = 7;
/** Crags from the DEM stand no nearer the eye than this, where they would hide the view. */
const CRAG_CLEAR_M = 40;
/** Crags along a crest, about this far apart and this tall. */
const CRAG_SPACING_M = 5;
const CRAG_HEIGHT_M: [number, number] = [2, 6];
/** Small stones on a jittered grid within this many metres, on mapped bare rock. */
const STONE_RADIUS_M = 60;
const STONE_SPACING_M = 1.2;
/** WorldCover classes of open, unfarmed ground, as the shader's `rough`. */
const ROUGH = new Set<number>([CLASS.shrub, CLASS.grass, CLASS.bare, 8, CLASS.moss]);

/** OSM building values that draw nothing, draw as a barn or shed, or draw as a block. */
const SKIPPED = new Set(['roof', 'ruins', 'construction', 'carport', 'storage_tank', 'bunker']);
const SHEDS = new Set(['barn', 'farm_auxiliary', 'shed', 'garage', 'garages', 'stable', 'cowshed', 'hut', 'sty', 'greenhouse', 'boathouse']);
const BLOCKS = new Set([
  'apartments', 'residential', 'hotel', 'commercial', 'retail', 'office', 'industrial', 'warehouse', 'school', 'public', 'civic',
  'hospital', 'university', 'college', 'terrace', 'dormitory', 'train_station', 'supermarket', 'kindergarten', 'sports_hall',
  'government', 'manufacture', 'service', 'church',
]);
/** Blocks of these kinds are one tall storey or two, not flats. */
const LOW = new Set(['industrial', 'warehouse', 'supermarket', 'retail', 'manufacture', 'sports_hall', 'service']);
const STOREY_M = 2.8;
/** A footprint that fills less of its rectangle than this, or is larger, keeps its outline. */
const RECTANGULAR = 0.7;
const LARGEST_M2 = 2500;
const FLAT_ROOF = '#7a766e';
/** Beyond this, a house is too far for its windows to show. */
const PLAIN_M = 400;
/** Width and height of a dry-stone wall and of a hedge; both sink a little into the ground. */
const WALL = { width: 0.6, height: 1.1, colours: STONE };
const HEDGE_SIZE = { width: 1.2, height: 1.8, colours: HEDGE };
const SUNK_M = 0.3;

/**
 * Trees and buildings within this many metres cast into the near shadow map, 0.2 m a texel. The
 * sun camera stands SUN_M up the sun's direction and sees twice as deep.
 */
const SHADOW_HALF_M = 200;
const SHADOW_SIZE = 2048;
const SUN_M = 2000;

/** Per instance: x, y, height, heading, tint. */
type Instances = number[];

/** The pack house for an OSM building value and its footprint area. */
function kindOf(tag: string, area: number): BuildingKind {
  if (SHEDS.has(tag)) return 'barn';
  if (tag === 'farm') return 'farmhouse';
  if (BLOCKS.has(tag)) return area < 260 ? 'house' : 'block';
  if (area < 30) return 'barn';
  if (area < 110) return 'cottage';
  if (area < 260) return 'house';
  if (area < 700) return 'farmhouse';
  return 'block';
}

/** Whether the eye, at the origin, is inside an outline. */
function inside(outline: THREE.Vector2[]): boolean {
  let crossings = 0;
  outline.forEach((a, i) => {
    const b = outline[(i + 1) % outline.length];
    if (a.y > 0 !== b.y > 0 && a.x + ((0 - a.y) * (b.x - a.x)) / (b.y - a.y) > 0) crossings++;
  });
  return crossings % 2 === 1;
}

/** The smallest rectangle around a convex-enough outline, with one side along one of its edges. */
function rectangle(outline: THREE.Vector2[]): { x: number; y: number; angle: number; width: number; depth: number } {
  let best = { area: Infinity, x: 0, y: 0, angle: 0, width: 0, depth: 0 };
  outline.forEach((a, i) => {
    const b = outline[(i + 1) % outline.length];
    const angle = Math.atan2(b.y - a.y, b.x - a.x);
    const [c, s] = [Math.cos(angle), Math.sin(angle)];
    let [u0, u1, v0, v1] = [Infinity, -Infinity, Infinity, -Infinity];
    for (const p of outline) {
      const [u, v] = [p.x * c + p.y * s, -p.x * s + p.y * c];
      [u0, u1, v0, v1] = [Math.min(u0, u), Math.max(u1, u), Math.min(v0, v), Math.max(v1, v)];
    }
    const area = (u1 - u0) * (v1 - v0);
    if (area >= best.area) return;
    const [u, v] = [(u0 + u1) / 2, (v0 + v1) / 2];
    best = { area, x: u * c - v * s, y: u * s + v * c, angle, width: u1 - u0, depth: v1 - v0 };
  });
  return best;
}

/** Walls and a flat roof along an outline, for footprints no pack house fits. */
function outlined(outline: THREE.Vector2[], z0: number, height: number, wall: THREE.Color): THREE.BufferGeometry {
  const position: number[] = [];
  const color: number[] = [];
  const roof = new THREE.Color(FLAT_ROOF);
  const vertex = (x: number, y: number, z: number, c: THREE.Color) => {
    position.push(x, y, z);
    color.push(c.r, c.g, c.b);
  };
  const z1 = z0 + height;
  outline.forEach((a, i) => {
    const b = outline[(i + 1) % outline.length];
    for (const [p, z] of [[a, z0], [b, z0], [b, z1], [a, z0], [b, z1], [a, z1]] as const) vertex(p.x, p.y, z, wall);
  });
  for (const face of THREE.ShapeUtils.triangulateShape(outline, [])) for (const i of face) vertex(outline[i].x, outline[i].y, z1, roof);
  const geometry = new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(color, 3));
  geometry.computeVertexNormals();
  return geometry;
}

/** Every building as a pack house on its footprint, in metres from the eye. */
function town(buildings: Building[], eye: number, pack: Pack): THREE.BufferGeometry | null {
  const houses: BuildingStyle = { ...pack.style, ...pack.houses };
  const rand = random(3);
  const parts: THREE.BufferGeometry[] = [];
  for (const [base, tag, levels, height, ring] of buildings) {
    if (SKIPPED.has(tag)) continue;
    const outline: THREE.Vector2[] = [];
    for (let i = 0; i < ring.length; i += 2) outline.push(new THREE.Vector2(ring[i], ring[i + 1]));
    if (THREE.ShapeUtils.isClockWise(outline)) outline.reverse();
    // The eye may stand in a building, as on a lookout's deck; it does not see that building.
    if (inside(outline)) continue;
    const area = Math.abs(THREE.ShapeUtils.area(outline));
    const fit = rectangle(outline);
    const kind = kindOf(tag, area);
    // A large footprint without a kind is a hall or a barn more often than flats.
    const low = LOW.has(tag) || (kind === 'block' && !BLOCKS.has(tag));
    const wall = levels ? levels * STOREY_M : height ? 0.65 * height : low ? 7 : undefined;
    const seed = Math.floor(rand() * 1e6);
    // Landmarks draw as themselves; bake.py names them in place of the building value.
    if (tag === 'ruin' || tag === 'lighthouse') {
      const tall = height ?? (tag === 'ruin' ? Math.min(14, 6 + 0.6 * Math.sqrt(area)) : 7);
      const model = tag === 'ruin' ? ruin(fit.width, fit.depth, tall, seed) : lighthouse(fit.width, fit.depth, tall);
      parts.push(model.rotateZ(fit.angle).translate(fit.x, fit.y, base - eye));
      continue;
    }
    if (area / (fit.width * fit.depth) < RECTANGULAR || area > LARGEST_M2) {
      const colour = new THREE.Color(pack.style.palette.render[seed % pack.style.palette.render.length]);
      parts.push(outlined(outline, base - eye, wall ?? 8, colour));
      continue;
    }
    const plain = Math.hypot(fit.x, fit.y) > PLAIN_M;
    const style = kind === 'cottage' || kind === 'house' ? houses : pack.style;
    parts.push(building(kind, style, { width: fit.width, depth: fit.depth, wall, seed, plain }).rotateZ(fit.angle).translate(fit.x, fit.y, base - eye));
  }
  return parts.length ? mergeGeometries(parts) : null;
}

/** The bare ground's height above the eye at x, y, bilinear from the finest level that holds it. */
function groundHeight(bake: Bake, x: number, y: number): number {
  const { size, half0_m, levels } = bake.meta;
  const k = Math.min(Math.max(0, Math.ceil(Math.log2(Math.max(Math.abs(x), Math.abs(y), 1) / half0_m))), levels - 1);
  const half = half0_m * 2 ** k;
  const px = (2 * half) / size;
  const [u, v] = [(x + half) / px - 0.5, (half - y) / px - 0.5];
  const [i, j] = [Math.min(Math.max(Math.floor(u), 0), size - 2), Math.min(Math.max(Math.floor(v), 0), size - 2)];
  const [fu, fv] = [u - i, v - j];
  const h = (di: number, dj: number) => bake.surface[k][(j + dj) * size + i + di];
  return (h(0, 0) * (1 - fu) + h(1, 0) * fu) * (1 - fv) + (h(0, 1) * (1 - fu) + h(1, 1) * fu) * fv;
}

/** Flat-shaded triangles from positions and one colour per vertex. */
function flat(position: number[], color: number[]): THREE.BufferGeometry {
  const geometry = new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
    .setAttribute('color', new THREE.Float32BufferAttribute(color, 3));
  geometry.computeVertexNormals();
  return geometry;
}

/** OSM walls and hedges as low boxes along their pieces, each end on the ground. */
function walls(bake: Bake): THREE.BufferGeometry | null {
  const position: number[] = [];
  const color: number[] = [];
  const rand = random(5);
  const { walls } = bake;
  for (let n = 0; n < walls.length; n += 5) {
    const [x0, y0, x1, y1, kind] = walls.subarray(n, n + 5);
    const length = Math.hypot(x1 - x0, y1 - y0);
    // Crests stand as crags, which set() places.
    if (kind === 2 || length < 0.1 || Math.min(Math.hypot(x0, y0), Math.hypot(x1, y1)) < CLEAR_M / 2) continue;
    const { width, height, colours } = kind ? HEDGE_SIZE : WALL;
    const [nx, ny] = [((y0 - y1) / length) * (width / 2), ((x1 - x0) / length) * (width / 2)];
    const [g0, g1] = [groundHeight(bake, x0, y0) - SUNK_M, groundHeight(bake, x1, y1) - SUNK_M];
    // Left and right, bottom and top, at each end.
    const corner = (end: 0 | 1, side: number, top: boolean) =>
      end ? [x1 + side * nx, y1 + side * ny, g1 + (top ? height : 0)] : [x0 + side * nx, y0 + side * ny, g0 + (top ? height : 0)];
    const c = new THREE.Color(colours[Math.floor(rand() * colours.length)]);
    const quad = (a: number[], b: number[], d: number[], e: number[]) => {
      for (const v of [a, b, d, b, e, d]) {
        position.push(...v);
        color.push(c.r, c.g, c.b);
      }
    };
    quad(corner(0, -1, false), corner(1, -1, false), corner(0, -1, true), corner(1, -1, true));
    quad(corner(1, 1, false), corner(0, 1, false), corner(1, 1, true), corner(0, 1, true));
    quad(corner(0, -1, true), corner(1, -1, true), corner(0, 1, true), corner(1, 1, true));
  }
  return position.length ? flat(position, color) : null;
}

/** One model drawn once per instance. */
function forest(model: THREE.BufferGeometry, instances: Instances): THREE.InstancedBufferGeometry {
  const geometry = new THREE.InstancedBufferGeometry();
  geometry.setIndex(model.index);
  for (const name of ['position', 'normal', 'color']) geometry.setAttribute(name, model.getAttribute(name));
  const data = new Float32Array(instances);
  const packed = new THREE.InstancedInterleavedBuffer(data, 5);
  geometry.setAttribute('tree', new THREE.InterleavedBufferAttribute(packed, 4, 0));
  geometry.setAttribute('tint', new THREE.InterleavedBufferAttribute(packed, 1, 4));
  geometry.instanceCount = instances.length / 5;
  return geometry;
}

/** A depth texture the shader compares against in one filtered lookup. */
function depthMap(): THREE.DepthTexture {
  const depth = new THREE.DepthTexture(SHADOW_SIZE, SHADOW_SIZE);
  depth.compareFunction = THREE.LessEqualCompare;
  depth.minFilter = depth.magFilter = THREE.LinearFilter;
  return depth;
}

const lodAt = (distance: number): Lod => (distance < LOD_RANGE_M[0] ? 0 : distance < LOD_RANGE_M[1] ? 1 : 2);

export class NearField {
  readonly trees = new THREE.Group();
  readonly buildings = new THREE.Group();
  /** Triangles the trees, plants and buildings add to a frame. */
  triangles = 0;
  /** Instances by "tree|name|lod", "plant|name", "rock|name" or "animal|name". */
  private groups = new Map<string, Instances>();
  /** Whether a habitat map placed the moor and lush plants, so the shader need not check them. */
  private placedByMap = false;
  /** Species that keep their summer leaves all year. */
  private evergreen = new Set<string>();
  private readonly models = new Map<string, THREE.BufferGeometry>();
  private readonly materials: Record<'tree' | 'plant' | 'moorPlant' | 'lushPlant' | 'building', THREE.ShaderMaterial>;
  /** Set when the sun, the spot, the season or the surface moved. */
  shadowsStale = true;
  private readonly shadowMap = new THREE.WebGLRenderTarget(SHADOW_SIZE, SHADOW_SIZE, { depthTexture: depthMap() });
  private readonly sun = new THREE.OrthographicCamera(-SHADOW_HALF_M, SHADOW_HALF_M, SHADOW_HALF_M, -SHADOW_HALF_M, 1, 2 * SUN_M);
  /** Each object material's depth-only twin, with the same vertex shader. */
  private readonly casters = new Map<THREE.ShaderMaterial, THREE.ShaderMaterial>();
  private readonly uniforms: Record<string, THREE.IUniform>;

  constructor(uniforms: Record<string, THREE.IUniform>) {
    this.uniforms = uniforms;
    uniforms.nearShadow.value = this.shadowMap.depthTexture;
    const material = (vertexShader: string, tie = 0, habitat = 0) =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        vertexShader,
        fragmentShader: objectFragment,
        // The shared uniform objects themselves, so light and look reach every object.
        uniforms: { ...uniforms, tie: { value: tie }, habitat: { value: habitat } },
        // Thin leaves and blades show both faces; their normals stay as the model lifts them.
        side: THREE.DoubleSide,
      });
    this.materials = {
      tree: material(treeVertex),
      plant: material(treeVertex, 0.7),
      moorPlant: material(treeVertex, 0.35, 1),
      lushPlant: material(treeVertex, 0.7, -1),
      building: material(buildingVertex),
    };
  }

  /** Place the bake's trees, shrubs, plants and buildings with the region's pack; setSeason draws them. */
  set(bake: Bake, region: Region): void {
    const { pack, look } = region;
    const { size, half0_m, eye_m } = bake.meta;
    const px = (2 * half0_m) / size;
    const texel = (x: number, y: number) => Math.floor((half0_m - y) / px) * size + Math.floor((x + half0_m) / px);
    const classAt = (x: number, y: number) => bake.classes[texel(x, y) * 2];
    const habitats = (bake.meta.habitat_levels ?? 0) > 0;
    const habitatAt = (x: number, y: number): Habitat => (habitats ? HABITATS[bake.habitat[texel(x, y)]] : 'none');
    const rand = random(7);
    const groups = new Map<string, Instances>();
    const add = (key: string, x: number, y: number, height: number, heading: number) => {
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(x, y, height, heading, 0.88 + 0.24 * rand());
    };
    const pick = <T extends { weight: number }>(list: T[]) => {
      let r = rand() * list.reduce((sum, s) => sum + s.weight, 0);
      return list.find((s) => (r -= s.weight) < 0) ?? list[list.length - 1];
    };
    // Hawthorn leans downwind by design, so it keeps its heading.
    const heading = (name: TreeName) => (name === 'hawthorn' ? 0 : rand() * 2 * Math.PI);

    const standing = pack.trees.filter((s) => s.cover === 'tree');
    const shrubs = pack.trees.filter((s) => s.cover === 'shrub');
    const { trees } = bake;
    for (let i = 0; i < trees.length; i += 4) {
      const [x, y, height] = [trees[i], trees[i + 1], trees[i + 2]];
      if (Math.hypot(x, y) < CLEAR_M || TREELESS.has(habitatAt(x, y))) continue;
      // Scrub on the map is gorse and thorn, not trees.
      if (habitatAt(x, y) === 'scrub' && shrubs.length) {
        const species = pick(shrubs).tree;
        add(`tree|${species}|${lodAt(Math.hypot(x, y))}`, x, y, SHRUB_HEIGHT_M[1] * (0.7 + 0.3 * rand()), heading(species));
        continue;
      }
      if (trees[i + 3] > 1.5 && pack.orchard) {
        add(`tree|${pack.orchard}|${lodAt(Math.hypot(x, y))}`, x, y, height, heading(pack.orchard));
        continue;
      }
      const leaf = trees[i + 3] > 0.5 ? 'broadleaved' : 'needleleaved';
      const elevation = bake.surface[0][texel(x, y)] + eye_m;
      const fits = standing.filter((s) => s.leaf === leaf && elevation >= s.elevation[0] && elevation <= s.elevation[1]);
      const sameLeaf = standing.filter((s) => s.leaf === leaf);
      const species = pick(fits.length ? fits : sameLeaf.length ? sameLeaf : standing).tree;
      add(`tree|${species}|${lodAt(Math.hypot(x, y))}`, x, y, height, heading(species));
    }

    if (shrubs.length) {
      for (let y = -SHRUB_RADIUS_M; y < SHRUB_RADIUS_M; y += SHRUB_SPACING_M) {
        for (let x = -SHRUB_RADIUS_M; x < SHRUB_RADIUS_M; x += SHRUB_SPACING_M) {
          const [sx, sy] = [x + (rand() - 0.5) * SHRUB_SPACING_M, y + (rand() - 0.5) * SHRUB_SPACING_M];
          const scrub = classAt(sx, sy) === CLASS.shrub || habitatAt(sx, sy) === 'scrub';
          if (rand() > SHRUB_SHARE || Math.hypot(sx, sy) > SHRUB_RADIUS_M || Math.hypot(sx, sy) < ROCK_CLEAR_M || !scrub) continue;
          if (this.offGround(bake, sx, sy, px)) continue;
          const species = pick(shrubs).tree;
          const height = SHRUB_HEIGHT_M[0] + rand() * (SHRUB_HEIGHT_M[1] - SHRUB_HEIGHT_M[0]);
          add(`tree|${species}|${lodAt(Math.hypot(sx, sy))}`, sx, sy, height, heading(species));
        }
      }
    }

    const { outcrops } = look;
    if (outcrops > 0) {
      const h = (x: number, y: number) => groundHeight(bake, x, y);
      const water = (x: number, y: number) => bake.classes[texel(x, y) * 2 + 1] > 127;
      for (let y = -ROCK_RADIUS_M; y < ROCK_RADIUS_M; y += ROCK_SPACING_M) {
        for (let x = -ROCK_RADIUS_M; x < ROCK_RADIUS_M; x += ROCK_SPACING_M) {
          const [rx, ry] = [x + (rand() - 0.5) * ROCK_SPACING_M, y + (rand() - 0.5) * ROCK_SPACING_M];
          const d = Math.hypot(rx, ry);
          if (d > ROCK_RADIUS_M || d < ROCK_CLEAR_M || !ROUGH.has(classAt(rx, ry)) || this.offGround(bake, rx, ry, px)) continue;
          const slope = Math.hypot(h(rx + 3, ry) - h(rx - 3, ry), h(rx, ry + 3) - h(rx, ry - 3)) / 6;
          const ridge = h(rx, ry) - (h(rx + 12, ry) + h(rx - 12, ry) + h(rx, ry + 12) + h(rx, ry - 12)) / 4;
          const sea = [[15, 0], [-15, 0], [0, 15], [0, -15]].some(([dx, dy]) => water(rx + dx, ry + dy)) ? 1 : 0;
          const mapped = habitatAt(rx, ry) === 'rock' ? 0.9 : 0;
          const rocky = Math.max(outcrops * Math.min(Math.max(1.4 * slope + 0.45 * ridge + 0.8 * sea - 0.05, 0), 1), mapped);
          if (rand() > 0.55 * rocky) continue;
          const name: RockName = rand() < 0.7 - 0.4 * sea ? 'slab' : 'boulder';
          const height = ROCK_HEIGHT_M[0] + rand() ** 2 * (ROCK_HEIGHT_M[1] - ROCK_HEIGHT_M[0]) * (1 + sea);
          add(`rock|${name}`, rx, ry, height, rand() * 2 * Math.PI);
        }
      }
    }

    // Crags along mapped aretes and ridges give back the knife edge the DEM rounds off.
    const pieces = bake.walls;
    for (let n = 0; n < pieces.length; n += 5) {
      if (pieces[n + 4] !== 2) continue;
      const [x0, y0, x1, y1] = pieces.subarray(n, n + 4);
      for (let t = rand() * CRAG_SPACING_M; t < Math.hypot(x1 - x0, y1 - y0); t += CRAG_SPACING_M * (0.6 + 0.8 * rand())) {
        const f = t / Math.hypot(x1 - x0, y1 - y0);
        const [cx, cy] = [x0 + (x1 - x0) * f + (rand() - 0.5) * 3, y0 + (y1 - y0) * f + (rand() - 0.5) * 3];
        if (Math.hypot(cx, cy) < CLEAR_M) continue;
        add('rock|crag', cx, cy, CRAG_HEIGHT_M[0] + rand() * (CRAG_HEIGHT_M[1] - CRAG_HEIGHT_M[0]), Math.atan2(y1 - y0, x1 - x0) + (rand() - 0.5) * 0.8);
      }
    }

    // Crests the DEM rounds off: ground that drops steeply to both sides along some axis.
    if (outcrops > 0) {
      const h = (x: number, y: number) => groundHeight(bake, x, y);
      for (let y = -CREST_RADIUS_M; y < CREST_RADIUS_M; y += CRAG_SPACING_M) {
        for (let x = -CREST_RADIUS_M; x < CREST_RADIUS_M; x += CRAG_SPACING_M) {
          const [cx, cy] = [x + (rand() - 0.5) * CRAG_SPACING_M, y + (rand() - 0.5) * CRAG_SPACING_M];
          if (Math.hypot(cx, cy) > CREST_RADIUS_M || Math.hypot(cx, cy) < CRAG_CLEAR_M) continue;
          const c = h(cx, cy);
          const crest = [[1, 0], [0, 1], [0.7071, 0.7071], [0.7071, -0.7071]].some(
            ([dx, dy]) => c - h(cx + dx * CREST_ARM_M, cy + dy * CREST_ARM_M) > CREST_DROP_M && c - h(cx - dx * CREST_ARM_M, cy - dy * CREST_ARM_M) > CREST_DROP_M,
          );
          if (crest && rand() < outcrops) add('rock|crag', cx, cy, CRAG_HEIGHT_M[0] + rand() * (CRAG_HEIGHT_M[1] - CRAG_HEIGHT_M[0]), rand() * 2 * Math.PI);
        }
      }
    }

    // Herds on their ground: a centre that fits, then each animal near it that also fits.
    const grazes = (on: Habitat[], x: number, y: number) =>
      Math.hypot(x, y) > CLEAR_M && !this.offGround(bake, x, y, px) && (habitats ? on.includes(habitatAt(x, y)) : classAt(x, y) === CLASS.grass);
    for (const herd of pack.herds ?? []) {
      for (let group = 0, tries = 0; group < herd.groups && tries < 400; tries++) {
        const [hx, hy] = [(rand() * 2 - 1) * HERD_RADIUS_M, (rand() * 2 - 1) * HERD_RADIUS_M];
        if (Math.hypot(hx, hy) > HERD_RADIUS_M || !grazes(herd.on, hx, hy)) continue;
        group++;
        const size = herd.size[0] + Math.floor(rand() * (herd.size[1] - herd.size[0] + 1));
        for (let i = 0; i < size; i++) {
          const [ax, ay] = [hx + (rand() - 0.5) * HERD_SPREAD_M, hy + (rand() - 0.5) * HERD_SPREAD_M];
          if (grazes(herd.on, ax, ay)) add(`animal|${herd.animal}`, ax, ay, ANIMAL_M * (0.85 + 0.3 * rand()), rand() * 2 * Math.PI);
        }
      }
    }

    // Mapped bare rock near the eye is strewn with small stones.
    if (habitats) {
      for (let y = -STONE_RADIUS_M; y < STONE_RADIUS_M; y += STONE_SPACING_M) {
        for (let x = -STONE_RADIUS_M; x < STONE_RADIUS_M; x += STONE_SPACING_M) {
          const [sx, sy] = [x + (rand() - 0.5) * STONE_SPACING_M, y + (rand() - 0.5) * STONE_SPACING_M];
          if (Math.hypot(sx, sy) < 1.5 || Math.hypot(sx, sy) > STONE_RADIUS_M || habitatAt(sx, sy) !== 'rock' || rand() > 0.5) continue;
          add(`rock|${rand() < 0.5 ? 'slab' : 'boulder'}`, sx, sy, 0.08 + 0.25 * rand() ** 2, rand() * 2 * Math.PI);
        }
      }
    }

    // The rarer plants first, so the common one does not take every spot.
    const ground = [...pack.ground].sort((a, b) => GROUND[a].share - GROUND[b].share);
    const moor = !!look.moor;
    // A habitat map settles moor here, once per plant; without one the shader asks the satellite.
    const moorAt = (x: number, y: number) => ['heath', 'bog'].includes(habitatAt(x, y));
    const grows = (name: PlantName, kind: number, x: number, y: number) => {
      const { on, habitat } = GROUND[name];
      if (habitatAt(x, y) === 'rock') return false;
      if (habitats && moor && habitat) return (habitat === 1) === moorAt(x, y) && (on.includes(kind) || kind === CLASS.grass);
      return on.includes(kind) || (moor && habitat === 1 && kind === CLASS.grass);
    };
    for (let y = -GROUND_RADIUS_M; y < GROUND_RADIUS_M; y += 1) {
      for (let x = -GROUND_RADIUS_M; x < GROUND_RADIUS_M; x += 1) {
        const [gx, gy] = [x + rand() - 0.5, y + rand() - 0.5];
        if (Math.hypot(gx, gy) > GROUND_RADIUS_M || Math.hypot(gx, gy) < 1.5 || this.offGround(bake, gx, gy, px)) continue;
        const kind = classAt(gx, gy);
        const name = ground.find((p) => grows(p, kind, gx, gy) && rand() < GROUND[p].share);
        if (!name) continue;
        const [low, high] = GROUND[name].height;
        add(`plant|${name}`, gx, gy, low + rand() * (high - low), rand() * 2 * Math.PI);
      }
    }
    this.groups = groups;
    this.placedByMap = habitats;
    this.evergreen = new Set(pack.trees.filter((s) => s.evergreen).map((s) => s.tree));
    this.shadowsStale = true;

    for (const mesh of this.buildings.children as THREE.Mesh[]) mesh.geometry.dispose();
    this.buildings.clear();
    for (const geometry of [town(bake.buildings, eye_m, pack), walls(bake)]) {
      if (!geometry) continue;
      const mesh = new THREE.Mesh(geometry, this.materials.building);
      mesh.frustumCulled = false;
      this.buildings.add(mesh);
    }
  }

  /** On a road or in water, where nothing grows: the shader's filtered road edge, nearest water. */
  private offGround(bake: Bake, x: number, y: number, px: number): boolean {
    const { size, half0_m } = bake.meta;
    const [u, v] = [(x + half0_m) / px - 0.5, (half0_m - y) / px - 0.5];
    const [i, j] = [Math.floor(u), Math.floor(v)];
    const [fu, fv] = [u - i, v - j];
    const r = (di: number, dj: number) => bake.roads[((j + dj) * size + i + di) * 2] / 10 - 5;
    const edge = (r(0, 0) * (1 - fu) + r(1, 0) * fu) * (1 - fv) + (r(0, 1) * (1 - fu) + r(1, 1) * fu) * fv;
    return edge < 0.2 || bake.water[(Math.round(v) * size + Math.round(u)) * 2] > 64;
  }

  /** Draw every group with the season's models; ground plants stay under the snow in winter. */
  setSeason(season: Season): void {
    for (const mesh of this.trees.children as THREE.Mesh[]) mesh.geometry.dispose();
    this.trees.clear();
    this.triangles = 0;
    for (const [key, instances] of this.groups) {
      const [what, name, lod] = key.split('|');
      if (what === 'plant' && season === 'winter') continue;
      const leaves = this.evergreen.has(name) ? 'summer' : season;
      const model = this.model(key, leaves, () =>
        what === 'tree' ? tree(name as TreeName, leaves, Number(lod) as Lod)
          : what === 'rock' ? rock(name as RockName)
          : what === 'animal' ? animal(name as AnimalName)
          : plant(name as PlantName, leaves),
      );
      const habitat = what === 'plant' && !this.placedByMap ? GROUND[name as PlantName].habitat : undefined;
      const material = what !== 'plant' ? this.materials.tree : habitat === 1 ? this.materials.moorPlant : habitat === -1 ? this.materials.lushPlant : this.materials.plant;
      const mesh = new THREE.Mesh(forest(model, instances), material);
      mesh.frustumCulled = false;
      // Ground plants are too small to cast a shadow worth their cost.
      mesh.userData.casts = what !== 'plant';
      this.trees.add(mesh);
      this.triangles += (instances.length / 5) * ((model.index ?? model.getAttribute('position')).count / 3);
    }
    for (const mesh of this.buildings.children as THREE.Mesh[]) this.triangles += mesh.geometry.getAttribute('position').count / 3;
    this.shadowsStale = true;
  }

  /** Draw the trees and buildings near the eye into the shadow map from the sun, if stale. */
  shade(renderer: THREE.WebGLRenderer): void {
    if (!this.shadowsStale) return;
    this.shadowsStale = false;
    const toward: THREE.Vector3 = this.uniforms.sunDir.value;
    this.sun.position.copy(toward).multiplyScalar(SUN_M);
    this.sun.up.set(0, Math.abs(toward.z) > 0.99 ? 1 : 0, Math.abs(toward.z) > 0.99 ? 0 : 1);
    this.sun.lookAt(0, 0, 0);
    this.sun.updateMatrixWorld();
    this.uniforms.nearShadowMatrix.value.multiplyMatrices(this.sun.projectionMatrix, this.sun.matrixWorldInverse);
    const meshes = [...this.trees.children, ...this.buildings.children] as THREE.Mesh[];
    const own = meshes.map((mesh) => [mesh.material as THREE.ShaderMaterial, mesh.visible] as const);
    for (const mesh of meshes) {
      mesh.visible &&= mesh.userData.casts !== false;
      mesh.material = this.caster(mesh.material as THREE.ShaderMaterial);
    }
    renderer.setRenderTarget(this.shadowMap);
    renderer.clear();
    renderer.render(this.trees, this.sun);
    renderer.render(this.buildings, this.sun);
    renderer.setRenderTarget(null);
    meshes.forEach((mesh, i) => ([mesh.material, mesh.visible] = own[i]));
  }

  private caster(material: THREE.ShaderMaterial): THREE.ShaderMaterial {
    if (!this.casters.has(material)) {
      this.casters.set(material, new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        vertexShader: material.vertexShader,
        fragmentShader: depthFragment,
        uniforms: material.uniforms,
        side: THREE.DoubleSide,
      }));
    }
    return this.casters.get(material)!;
  }

  private model(key: string, season: Season, build: () => THREE.BufferGeometry): THREE.BufferGeometry {
    const id = `${key}|${season}`;
    if (!this.models.has(id)) this.models.set(id, build());
    return this.models.get(id)!;
  }
}
