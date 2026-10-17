/**
 * What the near pass draws besides the terrain, from the place's kit: trees within 1 km at three
 * levels of detail, shrubs and ground plants near the eye, and OSM buildings as the kit's houses.
 * Where each object stands comes from the data; the kit decides what it looks like.
 */

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import type { Bake, Building } from './bake';
import { building, type BuildingKind, type BuildingStyle } from './models/buildings';
import type { Kit } from './models/kits';
import { plant, type PlantName } from './models/plants';
import { random } from './models/shape';
import { LOD_RANGE_M, tree, type Lod, type Season, type TreeName } from './models/trees';
import { buildingVertex, depthFragment, objectFragment, treeVertex } from './shaders';

/** WorldCover classes as the page's palette indexes them (CLASSES in bake.py). */
const CLASS = { tree: 0, shrub: 1, grass: 2, crop: 3, bare: 5, moss: 10 } as const;

/** Where each ground plant grows, how tall it stands, and the share of grid points it takes there. */
const GROUND: Record<PlantName, { on: number[]; height: [number, number]; share: number }> = {
  grass: { on: [CLASS.grass, CLASS.crop], height: [0.25, 0.5], share: 1 },
  'alpine-flowers': { on: [CLASS.grass], height: [0.2, 0.35], share: 0.15 },
  fern: { on: [CLASS.tree], height: [0.5, 0.9], share: 0.3 },
  heather: { on: [CLASS.shrub, CLASS.moss, CLASS.bare], height: [0.25, 0.45], share: 0.7 },
  'heather-bloom': { on: [CLASS.shrub, CLASS.moss], height: [0.25, 0.45], share: 0.35 },
  huckleberry: { on: [CLASS.tree], height: [0.4, 0.8], share: 0.25 },
};
/** The eye stands in a clearing this wide, so it is never inside a crown. */
const CLEAR_M = 6;
/** Ground plants on a jittered 1 m grid within this many metres. */
const GROUND_RADIUS_M = 60;
/** Shrub species on a jittered grid on shrubland within this many metres. */
const SHRUB_RADIUS_M = 500;
const SHRUB_SPACING_M = 5;
const SHRUB_HEIGHT_M: [number, number] = [1, 2.2];

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

/**
 * Trees and buildings within this many metres cast into the near shadow map, 0.2 m a texel. The
 * sun camera stands SUN_M up the sun's direction and sees twice as deep.
 */
const SHADOW_HALF_M = 200;
const SHADOW_SIZE = 2048;
const SUN_M = 2000;

/** Per instance: x, y, height, heading, tint. */
type Instances = number[];

/** The kit house for an OSM building value and its footprint area. */
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

/** Walls and a flat roof along an outline, for footprints no kit house fits. */
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

/** Every building as a kit house on its footprint, in metres from the eye. */
function town(buildings: Building[], eye: number, style: BuildingStyle): THREE.BufferGeometry | null {
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
    const wall = levels ? levels * STOREY_M : height ? 0.65 * height : LOW.has(tag) ? 7 : undefined;
    const seed = Math.floor(rand() * 1e6);
    if (area / (fit.width * fit.depth) < RECTANGULAR || area > LARGEST_M2) {
      const colour = new THREE.Color(style.palette.render[seed % style.palette.render.length]);
      parts.push(outlined(outline, base - eye, wall ?? 8, colour));
      continue;
    }
    const plain = Math.hypot(fit.x, fit.y) > PLAIN_M;
    parts.push(building(kind, style, { width: fit.width, depth: fit.depth, wall, seed, plain }).rotateZ(fit.angle).translate(fit.x, fit.y, base - eye));
  }
  return parts.length ? mergeGeometries(parts) : null;
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
  /** Instances by "tree|name|lod" or "plant|name". */
  private groups = new Map<string, Instances>();
  private readonly models = new Map<string, THREE.BufferGeometry>();
  private readonly materials: Record<'tree' | 'plant' | 'building', THREE.ShaderMaterial>;
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
    const material = (vertexShader: string, tie = 0) =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        vertexShader,
        fragmentShader: objectFragment,
        // The shared uniform objects themselves, so light and look reach every object.
        uniforms: { ...uniforms, tie: { value: tie } },
        // Thin leaves and blades show both faces; their normals stay as the model lifts them.
        side: THREE.DoubleSide,
      });
    this.materials = { tree: material(treeVertex), plant: material(treeVertex, 0.7), building: material(buildingVertex) };
  }

  /** Place the bake's trees, shrubs, plants and buildings with the kit; setSeason draws them. */
  set(bake: Bake, kit: Kit): void {
    const { size, half0_m, eye_m } = bake.meta;
    const px = (2 * half0_m) / size;
    const texel = (x: number, y: number) => Math.floor((half0_m - y) / px) * size + Math.floor((x + half0_m) / px);
    const classAt = (x: number, y: number) => bake.classes[texel(x, y) * 2];
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

    const standing = kit.trees.filter((s) => s.cover === 'tree');
    const { trees } = bake;
    for (let i = 0; i < trees.length; i += 4) {
      const [x, y, height] = [trees[i], trees[i + 1], trees[i + 2]];
      if (Math.hypot(x, y) < CLEAR_M) continue;
      const leaf = trees[i + 3] > 0.5 ? 'broadleaved' : 'needleleaved';
      const elevation = bake.surface[0][texel(x, y)] + eye_m;
      const fits = standing.filter((s) => s.leaf === leaf && elevation >= s.elevation[0] && elevation <= s.elevation[1]);
      const sameLeaf = standing.filter((s) => s.leaf === leaf);
      const species = pick(fits.length ? fits : sameLeaf.length ? sameLeaf : standing).tree;
      add(`tree|${species}|${lodAt(Math.hypot(x, y))}`, x, y, height, heading(species));
    }

    const shrubs = kit.trees.filter((s) => s.cover === 'shrub');
    if (shrubs.length) {
      for (let y = -SHRUB_RADIUS_M; y < SHRUB_RADIUS_M; y += SHRUB_SPACING_M) {
        for (let x = -SHRUB_RADIUS_M; x < SHRUB_RADIUS_M; x += SHRUB_SPACING_M) {
          const [sx, sy] = [x + (rand() - 0.5) * SHRUB_SPACING_M, y + (rand() - 0.5) * SHRUB_SPACING_M];
          if (Math.hypot(sx, sy) > SHRUB_RADIUS_M || classAt(sx, sy) !== CLASS.shrub || this.offGround(bake, sx, sy, px)) continue;
          const species = pick(shrubs).tree;
          const height = SHRUB_HEIGHT_M[0] + rand() * (SHRUB_HEIGHT_M[1] - SHRUB_HEIGHT_M[0]);
          add(`tree|${species}|${lodAt(Math.hypot(sx, sy))}`, sx, sy, height, heading(species));
        }
      }
    }

    // The rarer plants first, so the common one does not take every spot.
    const ground = [...kit.ground].sort((a, b) => GROUND[a].share - GROUND[b].share);
    for (let y = -GROUND_RADIUS_M; y < GROUND_RADIUS_M; y += 1) {
      for (let x = -GROUND_RADIUS_M; x < GROUND_RADIUS_M; x += 1) {
        const [gx, gy] = [x + rand() - 0.5, y + rand() - 0.5];
        if (Math.hypot(gx, gy) > GROUND_RADIUS_M || Math.hypot(gx, gy) < 1.5 || this.offGround(bake, gx, gy, px)) continue;
        const kind = classAt(gx, gy);
        const name = ground.find((p) => GROUND[p].on.includes(kind) && rand() < GROUND[p].share);
        if (!name) continue;
        const [low, high] = GROUND[name].height;
        add(`plant|${name}`, gx, gy, low + rand() * (high - low), rand() * 2 * Math.PI);
      }
    }
    this.groups = groups;
    this.shadowsStale = true;

    for (const mesh of this.buildings.children as THREE.Mesh[]) mesh.geometry.dispose();
    this.buildings.clear();
    const houses = town(bake.buildings, eye_m, kit.style);
    if (houses) {
      const mesh = new THREE.Mesh(houses, this.materials.building);
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
    return edge < 0.2 || bake.water[Math.round(v) * size + Math.round(u)] > 64;
  }

  /** Draw every group with the season's models; ground plants stay under the snow in winter. */
  setSeason(season: Season): void {
    for (const mesh of this.trees.children as THREE.Mesh[]) mesh.geometry.dispose();
    this.trees.clear();
    this.triangles = 0;
    for (const [key, instances] of this.groups) {
      const [what, name, lod] = key.split('|');
      if (what === 'plant' && season === 'winter') continue;
      const model = this.model(key, season, () =>
        what === 'tree' ? tree(name as TreeName, season, Number(lod) as Lod) : plant(name as PlantName, season),
      );
      const mesh = new THREE.Mesh(forest(model, instances), what === 'tree' ? this.materials.tree : this.materials.plant);
      mesh.frustumCulled = false;
      // Ground plants are too small to cast a shadow worth their cost.
      mesh.userData.casts = what === 'tree';
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
