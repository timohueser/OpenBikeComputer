/**
 * What the near pass draws besides the terrain, from the region's pack: trees within 1 km at three
 * levels of detail, shrubs, rocks and ground plants near the eye, OSM buildings as the pack's houses
 * and landmarks, and OSM walls and hedges.
 * Where each object stands comes from the data; the pack decides what it looks like.
 */

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { groundAt, heightAtTexel, snowAt, type Bake, type Building } from './bake';
import { seasonOf, snowStep } from './calendar';
import { COVER, COVER_ORDER, HARDNESS, LITHOLOGIES, MATERIALS, MOOR, ROCKY, ROUGH, WATERY } from './materials';
import { building, type BuildingKind, type BuildingStyle } from './models/buildings';
import { packNamed, type Pack, type Region } from './regions';
import { bridges } from './bridges';
import { lifts } from './lifts';
import { lighthouse, ruin } from './models/landmarks';
import { HEDGE, STONE } from './palette';
import { animal, ANIMALS, plant, PLANTS, tree, TREES } from './models/library';
import { rock, ROCK_GREY, type RockName } from './models/rocks';
import { random, smooth } from './models/shape';
import { LOD_RANGE_M, type Lod, type Season } from './models/trees';
import { buildingVertex, depthFragment, objectFragment, treeVertex } from './shaders';

/** Each material code's cover, by name. */
const COVER_OF = COVER.map((cover) => COVER_ORDER[cover]);

/** Each ground plant comes in this many shapes, so a stand does not repeat one clump. */
const PLANT_SEEDS = 3;
/** Wavy patches 0 to 1, a few tens of metres across, different for each seed. */
const patchAt = (x: number, y: number, seed: number) =>
  0.5 + 0.25 * (Math.sin(x / 13 + seed) * Math.sin(y / 17 - 1.7 * seed) + Math.sin((x + y) / 23 + 2.3 * seed) * Math.sin((x - y) / 19 + seed));
/** The eye stands in a clearing this wide, so it is never inside a crown. */
const CLEAR_M = 6;
/** Ground plants on a jittered 1 m grid within this many metres, all of them within THIN_M. */
const GROUND_RADIUS_M = 60;
const THIN_M = 20;
/** Shrub species on half the points of a jittered grid on shrubland within this many metres, a third as many on sparse ground. */
const SHRUB_RADIUS_M = 500;
const SHRUB_SPACING_M = 5;
const SHRUB_SHARE = 0.5;
/** A tree model standing as a shrub stands this tall; a plant model stands as tall as its plant file says. */
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
/** Herds graze within this many metres, each spread over HERD_SPREAD_M. */
const HERD_RADIUS_M = 400;
const HERD_SPREAD_M = 14;
/** Ground within this many metres of the sea and below this height above it is strand: bare. */
const STRAND_M: [number, number] = [25, 6];
/** Lifts within the near pass's far plane. */
const LIFT_REACH_M = 6000;
/** A crest within this many metres drops more than CREST_DROP_M within CREST_ARM_M to both sides. */
const CREST_RADIUS_M = 1500;
const CREST_ARM_M = 20;
const CREST_DROP_M = 7;
/** Crags from the DEM stand no nearer the eye than this, where they would hide the view. */
const CRAG_CLEAR_M = 40;
/** Crags along a crest, about this far apart and this tall. */
const CRAG_SPACING_M = 5;
const CRAG_HEIGHT_M: [number, number] = [1.5, 3.5];
/** Small stones on a jittered grid within this many metres, on exposed rock. */
const STONE_RADIUS_M = 60;
const STONE_SPACING_M = 1.2;

/** OSM building values that draw nothing, draw as a barn or shed, as a church or as a block. */
const SKIPPED = new Set(['roof', 'ruins', 'construction', 'carport', 'storage_tank', 'bunker']);
const SHEDS = new Set(['barn', 'farm_auxiliary', 'shed', 'garage', 'garages', 'stable', 'cowshed', 'hut', 'sty', 'greenhouse', 'boathouse']);
const CHURCHES = new Set(['church', 'chapel', 'cathedral']);
const BLOCKS = new Set([
  'apartments', 'residential', 'hotel', 'commercial', 'retail', 'office', 'industrial', 'warehouse', 'school', 'public', 'civic',
  'hospital', 'university', 'college', 'terrace', 'dormitory', 'train_station', 'supermarket', 'kindergarten', 'sports_hall',
  'government', 'manufacture', 'service',
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

/**
 * Whatever the near pass draws marks its pixels in the stencil buffer, and the far pass shades only
 * the pixels left unmarked.
 */
export const NEAR_MARK = { stencilWrite: true, stencilRef: 1, stencilZPass: THREE.ReplaceStencilOp };

/** Per instance: x, y, height, heading, tint. */
type Instances = number[];

/** The pack house for an OSM building value and its footprint area. */
function kindOf(tag: string, area: number): BuildingKind {
  if (SHEDS.has(tag)) return 'barn';
  if (CHURCHES.has(tag)) return 'church';
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

/** Every building as a house of its own pack and place on its footprint, in metres from the eye. */
function town(buildings: Building[], eye: number, packOf: (building: Building) => Pack): THREE.BufferGeometry | null {
  const rand = random(3);
  const parts: THREE.BufferGeometry[] = [];
  for (const house of buildings) {
    const [base, tag, levels, height, ring] = house;
    const pack = packOf(house);
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
    const style: BuildingStyle = kind === 'cottage' || kind === 'house' ? { ...pack.style, ...pack.houses } : pack.style;
    parts.push(building(kind, style, { width: fit.width, depth: fit.depth, wall, seed, plain }).rotateZ(fit.angle).translate(fit.x, fit.y, base - eye));
  }
  return parts.length ? mergeGeometries(parts) : null;
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
    const [g0, g1] = [groundAt(bake, x0, y0) - SUNK_M, groundAt(bake, x1, y1) - SUNK_M];
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
  /** Instances by "tree|model|lod", "shrub|model", "plant|species|seed", "rock|model|colour" or "animal|model|seed". */
  private groups = new Map<string, Instances>();
  /** The packs' ground plants by species. */
  private ground = new Map<string, Pack['ground'][number]>();
  /** Whether snow covers a ground plant at x, y on a day step of the snow data. */
  private buried: (x: number, y: number, step: number) => boolean = () => false;
  /** Species that keep their summer leaves all year. */
  private evergreen = new Set<string>();
  private readonly models = new Map<string, THREE.BufferGeometry>();
  private readonly materials: Record<'tree' | 'plant' | 'ownPlant' | 'building', THREE.ShaderMaterial>;
  /** Rocks by the colour of the rock they lie on. */
  private readonly rocks = new Map<string, THREE.ShaderMaterial>();
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
    this.materials = {
      tree: this.material(treeVertex),
      plant: this.material(treeVertex, 0.85),
      ownPlant: this.material(treeVertex, 0.35),
      building: this.material(buildingVertex),
    };
  }

  private material(vertexShader: string, tie = 0): THREE.ShaderMaterial {
    return new THREE.ShaderMaterial({
      glslVersion: THREE.GLSL3,
      vertexShader,
      fragmentShader: objectFragment,
      // The shared uniform objects themselves, so light and look reach every object.
      uniforms: { ...this.uniforms, tie: { value: tie }, recolour: { value: new THREE.Vector3(1, 1, 1) } },
      // Thin leaves and blades show both faces; their normals stay as the model lifts them.
      side: THREE.DoubleSide,
      ...NEAR_MARK,
    });
  }

  /**
   * Place the bake's trees, shrubs, plants and buildings, each with the pack and rules of the region
   * it stands in (`regions`, in meta.regions' order); setDate draws them.
   */
  set(bake: Bake, regions: Region[]): void {
    const { size, half0_m, eye_m } = bake.meta;
    const px = (2 * half0_m) / size;
    const texel = (x: number, y: number) => Math.floor((half0_m - y) / px) * size + Math.floor((x + half0_m) / px);
    const materialAt = (x: number, y: number) => bake.material[texel(x, y)];
    // A ground layer at x, y, from the finest level that holds it.
    const at = (layer: Uint8Array, x: number, y: number) => {
      const k = Math.min(Math.max(0, Math.ceil(Math.log2(Math.max(Math.abs(x), Math.abs(y), 1) / half0_m))), bake.meta.levels - 1);
      const half = half0_m * 2 ** k;
      const p = (2 * half) / size;
      return layer[k * size * size + Math.floor((half - y) / p) * size + Math.floor((x + half) / p)];
    };
    const regionAt = (x: number, y: number) => at(bake.region, x, y);
    const kits = regions.map(({ look, pack }) => ({
      look,
      pack,
      standing: pack.trees.filter((s) => s.cover === 'tree'),
      shrubs: pack.trees.filter((s) => s.cover === 'shrub'),
      // The rarer plants first, so the common one does not take every spot.
      plants: [...pack.ground].sort((a, b) => a.share - b.share),
    }));
    const kitAt = (x: number, y: number) => kits[regionAt(x, y)];
    // The colour of the rock at x, y: its lithology's in the region's rules, else the region's rock.
    const stoneAt = (x: number, y: number) => {
      const { look } = kitAt(x, y);
      return look.lithology?.[LITHOLOGIES[at(bake.litho, x, y)]]?.colour ?? look.rock;
    };
    // How far exposed rock shows as rock at x, y, 0 to 1, as the terrain's `exposed`.
    const exposedAt = (x: number, y: number) =>
      ROCKY.has(materialAt(x, y)) ? smooth(0, 0.6, kitAt(x, y).look.outcrops) * smooth(0.2, 0.6, HARDNESS[at(bake.litho, x, y)]) : 0;
    const groups = new Map<string, Instances>();
    // One stream of random numbers per kind of object, so placing one kind never moves another.
    let rand = random(7);
    const add = (key: string, x: number, y: number, height: number, heading: number) => {
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(x, y, height, heading, 0.88 + 0.24 * rand());
    };
    const pick = <T extends { weight: number }>(list: T[]) => {
      let r = rand() * list.reduce((sum, s) => sum + s.weight, 0);
      return list.find((s) => (r -= s.weight) < 0) ?? list[list.length - 1];
    };
    const heading = (name: string) => (TREES[name]?.keepsHeading ? 0 : rand() * 2 * Math.PI);
    // A plant model standing as a shrub has one level of detail, and its own colours.
    const shrubKey = (name: string, d: number) => (name in TREES ? `tree|${name}|${lodAt(d)}` : `shrub|${name}`);
    const shrubHeight = (name: string) => PLANTS[name]?.ground.height ?? SHRUB_HEIGHT_M;

    const seaAt = (x: number, y: number) => {
      const k = Math.min(Math.max(0, Math.ceil(Math.log2(Math.max(Math.abs(x), Math.abs(y), 1) / half0_m))), bake.meta.water_levels - 1);
      const half = half0_m * 2 ** k;
      const p = (2 * half) / size;
      const [col, row] = [Math.floor((x + half) / p), Math.floor((half - y) / p)];
      return WATERY[bake.material[k * size * size + row * size + col]] && heightAtTexel(bake, k, col, row) + eye_m < 2;
    };
    // The strand: low ground by the sea, beach or storm-washed rock, where nothing grows.
    const strand = (x: number, y: number) =>
      groundAt(bake, x, y) + eye_m < STRAND_M[1] &&
      [0, 1, 2, 3, 4, 5, 6, 7].some((a) => seaAt(x + Math.sin((a * Math.PI) / 4) * STRAND_M[0], y + Math.cos((a * Math.PI) / 4) * STRAND_M[0]));
    // Within the region's windswept_m of the sea, wind and salt keep the ground bare of trees and shrubs.
    const exposed = (x: number, y: number) => {
      const reach = kitAt(x, y).look.windswept_m ?? 0;
      return reach > 0 && [0, 1, 2, 3, 4, 5, 6, 7].some((a) => seaAt(x + Math.sin((a * Math.PI) / 4) * reach, y + Math.cos((a * Math.PI) / 4) * reach));
    };
    // Shrubs keep to their elevation band, as trees do, where any shrub fits there.
    const shrubAt = (shrubs: Pack['trees'], x: number, y: number) => {
      const elevation = groundAt(bake, x, y) + eye_m;
      const fits = shrubs.filter((s) => elevation >= s.elevation[0] && elevation <= s.elevation[1]);
      return pick(fits.length ? fits : shrubs).tree;
    };
    const { trees } = bake;
    for (let i = 0; i < trees.length; i += 4) {
      const [x, y, height] = [trees[i], trees[i + 1], trees[i + 2]];
      if (Math.hypot(x, y) < CLEAR_M || exposed(x, y)) continue;
      const { pack, standing } = kitAt(x, y);
      if (trees[i + 3] > 1.5 && pack.orchard) {
        add(`tree|${pack.orchard}|${lodAt(Math.hypot(x, y))}`, x, y, height, heading(pack.orchard));
        continue;
      }
      if (!standing.length) continue;
      const leaf = trees[i + 3] > 0.5 ? 'broadleaved' : 'needleleaved';
      const elevation = groundAt(bake, x, y) + eye_m;
      // The elevation band outranks the leaf type, which is often only the region's guess.
      const inBand = standing.filter((s) => elevation >= s.elevation[0] && elevation <= s.elevation[1]);
      const fits = inBand.filter((s) => s.leaf === leaf);
      const sameLeaf = standing.filter((s) => s.leaf === leaf);
      const species = pick(fits.length ? fits : inBand.length ? inBand : sameLeaf.length ? sameLeaf : standing).tree;
      add(`tree|${species}|${lodAt(Math.hypot(x, y))}`, x, y, height, heading(species));
    }

    rand = random(8);
    for (let y = -SHRUB_RADIUS_M; y < SHRUB_RADIUS_M; y += SHRUB_SPACING_M) {
      for (let x = -SHRUB_RADIUS_M; x < SHRUB_RADIUS_M; x += SHRUB_SPACING_M) {
        const [sx, sy] = [x + (rand() - 0.5) * SHRUB_SPACING_M, y + (rand() - 0.5) * SHRUB_SPACING_M];
        const material = materialAt(sx, sy);
        const { shrubs } = kitAt(sx, sy);
        // Bare ground is mostly "bare or sparse vegetation": scattered shrubs, a third as dense, and
        // fewer as far as it shows as exposed rock.
        const share = COVER_OF[material] === 'shrub' ? SHRUB_SHARE : COVER_OF[material] === 'bare' ? (SHRUB_SHARE / 3) * (1 - exposedAt(sx, sy)) : 0;
        if (rand() > share || !shrubs.length || Math.hypot(sx, sy) > SHRUB_RADIUS_M || Math.hypot(sx, sy) < ROCK_CLEAR_M) continue;
        if (exposed(sx, sy) || this.offGround(bake, sx, sy, px) || strand(sx, sy)) continue;
        const species = shrubAt(shrubs, sx, sy);
        const [low, high] = shrubHeight(species);
        const height = low + rand() * (high - low);
        add(shrubKey(species, Math.hypot(sx, sy)), sx, sy, height, heading(species));
      }
    }

    rand = random(9);
    const h = (x: number, y: number) => groundAt(bake, x, y);
    const water = (x: number, y: number) => WATERY[materialAt(x, y)];
    for (let y = -ROCK_RADIUS_M; y < ROCK_RADIUS_M; y += ROCK_SPACING_M) {
      for (let x = -ROCK_RADIUS_M; x < ROCK_RADIUS_M; x += ROCK_SPACING_M) {
        const [rx, ry] = [x + (rand() - 0.5) * ROCK_SPACING_M, y + (rand() - 0.5) * ROCK_SPACING_M];
        const d = Math.hypot(rx, ry);
        const { outcrops } = kitAt(rx, ry).look;
        const material = materialAt(rx, ry);
        if (!outcrops || d > ROCK_RADIUS_M || d < ROCK_CLEAR_M || !ROUGH.has(material) || this.offGround(bake, rx, ry, px)) continue;
        const slope = Math.hypot(h(rx + 3, ry) - h(rx - 3, ry), h(rx, ry + 3) - h(rx, ry - 3)) / 6;
        const ridge = h(rx, ry) - (h(rx + 12, ry) + h(rx - 12, ry) + h(rx, ry + 12) + h(rx, ry - 12)) / 4;
        const sea = [[15, 0], [-15, 0], [0, 15], [0, -15]].some(([dx, dy]) => water(rx + dx, ry + dy)) ? 1 : 0;
        // Hard rock, karst above all, breaks through even where the ground is level.
        const hardness = HARDNESS[bake.litho[texel(rx, ry)]];
        const mapped = 0.9 * exposedAt(rx, ry);
        const hard = 0.3 + 1.4 * hardness;
        // Boulders do not lie on faces steeper than about 40°.
        if (slope > 0.85) continue;
        const rocky = Math.max(outcrops * hard * Math.min(Math.max(1.4 * slope + 0.45 * ridge + 0.8 * sea + 0.6 * (hardness - 0.5) - 0.05, 0), 1), mapped);
        if (rand() > 0.55 * rocky) continue;
        const name: RockName = rand() < 0.7 - 0.4 * sea ? 'slab' : 'boulder';
        const height = ROCK_HEIGHT_M[0] + rand() ** 2 * (ROCK_HEIGHT_M[1] - ROCK_HEIGHT_M[0]) * (1 + sea);
        add(`rock|${name}|${stoneAt(rx, ry)}`, rx, ry, height, rand() * 2 * Math.PI);
      }
    }

    // Crags along mapped aretes and ridges give back the knife edge the DEM rounds off.
    rand = random(10);
    const pieces = bake.walls;
    for (let n = 0; n < pieces.length; n += 5) {
      if (pieces[n + 4] !== 2) continue;
      const [x0, y0, x1, y1] = pieces.subarray(n, n + 4);
      for (let t = rand() * CRAG_SPACING_M; t < Math.hypot(x1 - x0, y1 - y0); t += CRAG_SPACING_M * (0.6 + 0.8 * rand())) {
        const f = t / Math.hypot(x1 - x0, y1 - y0);
        const [cx, cy] = [x0 + (x1 - x0) * f + (rand() - 0.5) * 3, y0 + (y1 - y0) * f + (rand() - 0.5) * 3];
        if (Math.hypot(cx, cy) < CLEAR_M) continue;
        add(`rock|crag|${stoneAt(cx, cy)}`, cx, cy, CRAG_HEIGHT_M[0] + rand() * (CRAG_HEIGHT_M[1] - CRAG_HEIGHT_M[0]), Math.atan2(y1 - y0, x1 - x0) + (rand() - 0.5) * 0.8);
      }
    }

    // Crests the DEM rounds off: ground that drops steeply to both sides along some axis.
    rand = random(11);
    for (let y = -CREST_RADIUS_M; y < CREST_RADIUS_M; y += CRAG_SPACING_M) {
      for (let x = -CREST_RADIUS_M; x < CREST_RADIUS_M; x += CRAG_SPACING_M) {
        const [cx, cy] = [x + (rand() - 0.5) * CRAG_SPACING_M, y + (rand() - 0.5) * CRAG_SPACING_M];
        const { outcrops } = kitAt(cx, cy).look;
        if (!outcrops || Math.hypot(cx, cy) > CREST_RADIUS_M || Math.hypot(cx, cy) < CRAG_CLEAR_M) continue;
        const c = h(cx, cy);
        const crest = [[1, 0], [0, 1], [0.7071, 0.7071], [0.7071, -0.7071]].some(
          ([dx, dy]) => c - h(cx + dx * CREST_ARM_M, cy + dy * CREST_ARM_M) > CREST_DROP_M && c - h(cx - dx * CREST_ARM_M, cy - dy * CREST_ARM_M) > CREST_DROP_M,
        );
        if (crest && rand() < outcrops) add(`rock|crag|${stoneAt(cx, cy)}`, cx, cy, CRAG_HEIGHT_M[0] + rand() * (CRAG_HEIGHT_M[1] - CRAG_HEIGHT_M[0]), rand() * 2 * Math.PI);
      }
    }

    // Herds on their ground in their own region: a centre that fits, then each animal near it that also fits.
    rand = random(12);
    kits.forEach((kit, region) => {
      for (const herd of kit.pack.herds) {
        const on = new Set(herd.on.map((id) => MATERIALS.indexOf(id)));
        const grazes = (x: number, y: number) =>
          Math.hypot(x, y) > CLEAR_M && regionAt(x, y) === region && on.has(materialAt(x, y)) && !this.offGround(bake, x, y, px);
        for (let group = 0, tries = 0; group < herd.groups && tries < 400; tries++) {
          const [hx, hy] = [(rand() * 2 - 1) * HERD_RADIUS_M, (rand() * 2 - 1) * HERD_RADIUS_M];
          if (Math.hypot(hx, hy) > HERD_RADIUS_M || !grazes(hx, hy)) continue;
          group++;
          const size = herd.size[0] + Math.floor(rand() * (herd.size[1] - herd.size[0] + 1));
          for (let i = 0; i < size; i++) {
            const [ax, ay] = [hx + (rand() - 0.5) * HERD_SPREAD_M, hy + (rand() - 0.5) * HERD_SPREAD_M];
            // Most of a herd wears the breed's usual coat; the seed picks one of the model's coats.
            const coat = rand() < 0.6 ? 0 : 1 + Math.floor(rand() * 2);
            if (grazes(ax, ay)) add(`animal|${herd.animal}|${coat}`, ax, ay, ANIMALS[herd.animal].height_m * (0.85 + 0.3 * rand()), rand() * 2 * Math.PI);
          }
        }
      }
    });

    // Exposed rock near the eye is strewn with small stones.
    rand = random(13);
    for (let y = -STONE_RADIUS_M; y < STONE_RADIUS_M; y += STONE_SPACING_M) {
      for (let x = -STONE_RADIUS_M; x < STONE_RADIUS_M; x += STONE_SPACING_M) {
        const [sx, sy] = [x + (rand() - 0.5) * STONE_SPACING_M, y + (rand() - 0.5) * STONE_SPACING_M];
        if (Math.hypot(sx, sy) < 1.5 || Math.hypot(sx, sy) > STONE_RADIUS_M || rand() > 0.5 * exposedAt(sx, sy)) continue;
        add(`rock|${rand() < 0.5 ? 'slab' : 'boulder'}|${stoneAt(sx, sy)}`, sx, sy, 0.08 + 0.25 * rand() ** 2, rand() * 2 * Math.PI);
      }
    }

    // In a region with moor, moor plants grow on its heath and bog, on grass there too, and lush ones off it.
    const grows = ({ on, habitat }: Pack['ground'][number], material: number, look: Region['look']) => {
      const cover = COVER_OF[material];
      const onCover = (on as string[]).includes(cover);
      if (look.moor && habitat) return (habitat === 'moor') === MOOR.has(material) && (onCover || cover === 'grass');
      return onCover;
    };
    rand = random(14);
    for (let y = -GROUND_RADIUS_M; y < GROUND_RADIUS_M; y += 1) {
      for (let x = -GROUND_RADIUS_M; x < GROUND_RADIUS_M; x += 1) {
        const [gx, gy] = [x + rand() - 0.5, y + rand() - 0.5];
        if (Math.hypot(gx, gy) > GROUND_RADIUS_M || Math.hypot(gx, gy) < 1.5 || this.offGround(bake, gx, gy, px) || strand(gx, gy)) continue;
        // Nothing grows where exposed rock shows as rock.
        if (rand() < exposedAt(gx, gy)) continue;
        const material = materialAt(gx, gy);
        const { plants, look } = kitAt(gx, gy);
        const share = (p: Pack['ground'][number], i: number) => p.share * (p.clumped ? 2.5 * smooth(0.55, 0.7, patchAt(gx, gy, i)) : 1);
        const species = plants.find((p, i) => grows(p, material, look) && rand() < share(p, i));
        if (!species) continue;
        // Past THIN_M a share of the plants stands, a little larger, and the ground's own colour
        // fills between them: from afar the count shows less than the cost.
        const d = Math.hypot(gx, gy);
        const keep = d < THIN_M ? 1 : Math.max(0.3, THIN_M / d);
        if (rand() > keep) continue;
        const [low, high] = species.height;
        add(`plant|${species.id}|${1 + Math.floor(rand() * PLANT_SEEDS)}`, gx, gy, (low + rand() * (high - low)) / Math.sqrt(Math.sqrt(keep)), rand() * 2 * Math.PI);
      }
    }
    this.groups = groups;
    // Snow on a day step covers a ground plant as it covers the ground, by its own region's share.
    this.buried = (x, y, step) => snowAt(bake, x, y, step, regions[regionAt(x, y)].look.snow_share) > 0.5;
    this.ground = new Map(kits.flatMap(({ pack }) => pack.ground.map((p) => [p.id, p] as const)));
    this.evergreen = new Set(kits.flatMap(({ pack }) => pack.trees.filter((s) => s.evergreen).map((s) => s.tree)));
    this.shadowsStale = true;

    for (const mesh of this.buildings.children as THREE.Mesh[]) mesh.geometry.dispose();
    this.buildings.clear();
    const { solid, cables } = lifts(bake, LIFT_REACH_M);
    const decks = bridges(bake, (x, y) => regions[regionAt(x, y)]);
    // A house takes the pack of the region it stands in, styled for its own place.
    const packs = new Map<string, Pack>();
    const packOf = ([, , , , ring, place]: Building) => {
      const { pack } = kitAt(ring[0], ring[1]);
      const key = `${pack.id}|${place}`;
      if (!packs.has(key)) packs.set(key, place ? packNamed(pack.id, [place, place.split('-')[0]]) : pack);
      return packs.get(key)!;
    };
    for (const geometry of [town(bake.buildings, eye_m, packOf), walls(bake), solid, decks]) {
      if (!geometry) continue;
      const mesh = new THREE.Mesh(geometry, this.materials.building);
      mesh.frustumCulled = false;
      this.buildings.add(mesh);
    }
    if (cables) {
      // A line a pixel wide at any distance, as a cable reads; too thin to cast a shadow.
      const line = new THREE.LineSegments(cables, this.materials.building);
      line.frustumCulled = false;
      line.userData.casts = false;
      this.buildings.add(line);
    }
  }

  /** On a road or in water, where nothing grows: the shader's filtered road edge, nearest water. */
  private offGround(bake: Bake, x: number, y: number, px: number): boolean {
    const { size, half0_m } = bake.meta;
    const [u, v] = [(x + half0_m) / px - 0.5, (half0_m - y) / px - 0.5];
    const [i, j] = [Math.floor(u), Math.floor(v)];
    const [fu, fv] = [u - i, v - j];
    const r = (di: number, dj: number) => bake.roads[((j + dj) * size + i + di) * 3] / 10 - 5;
    const edge = (r(0, 0) * (1 - fu) + r(1, 0) * fu) * (1 - fv) + (r(0, 1) * (1 - fu) + r(1, 1) * fu) * fv;
    return edge < 0.2 || bake.water[(Math.round(v) * size + Math.round(u)) * 2] > 64;
  }

  /** Draw every group with the models of the day's season; ground plants under the day's snow stay hidden. */
  setDate(day: number): void {
    for (const mesh of this.trees.children as THREE.Mesh[]) mesh.geometry.dispose();
    this.trees.clear();
    this.triangles = 0;
    const [season, step] = [seasonOf(day), snowStep(day)];
    for (const [key, all] of this.groups) {
      const [what, name, lod] = key.split('|');
      const instances = what === 'plant' ? this.standing(all, step) : all;
      if (!instances.length) continue;
      const leaves = this.evergreen.has(name) ? 'summer' : season;
      const ground = what === 'plant' ? this.ground.get(name) : undefined;
      const model = this.model(key, leaves, () =>
        what === 'tree' ? tree(name, leaves, Number(lod) as Lod)
          : what === 'rock' ? rock(name as RockName)
          : what === 'animal' ? animal(name, Number(lod))
          : plant(ground?.plant ?? name, leaves, Number(lod) || 1),
      );
      const material = what === 'rock' ? this.rock(lod) : what === 'shrub' || ground?.own ? this.materials.ownPlant
        : ground ? this.materials.plant : this.materials.tree;
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

  /** The plants of a group that no snow covers on the day step. */
  private standing(plants: Instances, step: number): Instances {
    const kept: Instances = [];
    for (let i = 0; i < plants.length; i += 5) if (!this.buried(plants[i], plants[i + 1], step)) kept.push(...plants.slice(i, i + 5));
    return kept;
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

  /** The rock material that tints the models' grey to a rock's colour. */
  private rock(hex: string): THREE.ShaderMaterial {
    if (!this.rocks.has(hex)) {
      const material = this.material(treeVertex);
      const [stone, grey] = [new THREE.Color(hex), new THREE.Color(ROCK_GREY)];
      material.uniforms.recolour.value.set(stone.r / grey.r, stone.g / grey.g, stone.b / grey.b);
      this.rocks.set(hex, material);
    }
    return this.rocks.get(hex)!;
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
