/**
 * What the near pass draws besides the terrain: instanced low-poly trees within 1 km and extruded
 * buildings with flat roofs. Every object comes from data: trees on tree pixels, buildings from
 * OSM footprints.
 */

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import type { Bake, Building } from './bake';
import { buildingVertex, objectFragment, treeVertex } from './shaders';

const BARK = '#6e5b4b';
const WALL = '#d6cfc0';
const ROOF = '#8a6b58';

/** One flat-shaded piece of a unit-height tree: `part` 0 is bark, 1 is foliage. */
function piece(geometry: THREE.BufferGeometry, part: number): THREE.BufferGeometry {
  const flat = geometry.index ? geometry.toNonIndexed() : geometry;
  flat.deleteAttribute('uv');
  flat.computeVertexNormals();
  flat.setAttribute('part', new THREE.Float32BufferAttribute(new Float32Array(flat.getAttribute('position').count).fill(part), 1));
  return flat;
}

// three's cones and cylinders stand on y; the terrain's up is z.
const upright = (geometry: THREE.BufferGeometry) => geometry.rotateX(Math.PI / 2);

/** Three stacked open cones on a short trunk. */
function conifer(): THREE.BufferGeometry {
  const trunk = piece(upright(new THREE.CylinderGeometry(0.025, 0.035, 0.2, 5, 1, true)).translate(0, 0, 0.1), 0);
  const tiers = [
    [0.12, 0.3, 0.44],
    [0.36, 0.23, 0.38],
    [0.58, 0.15, 0.42],
  ].map(([z, radius, height]) => piece(upright(new THREE.ConeGeometry(radius, height, 7, 1, true)).translate(0, 0, z + height / 2), 1));
  return mergeGeometries([trunk, ...tiers])!;
}

/** A low icosahedron crown on a trunk. */
function broadleaf(): THREE.BufferGeometry {
  const trunk = piece(upright(new THREE.CylinderGeometry(0.03, 0.045, 0.5, 5, 1, true)).translate(0, 0, 0.25), 0);
  const crown = piece(new THREE.IcosahedronGeometry(0.3, 0).scale(1, 1, 0.9).translate(0, 0, 0.62), 1);
  return mergeGeometries([trunk, crown])!;
}

/** Five thin blades fanned out, their normals up so they light like the meadow under them. */
function tuft(): THREE.BufferGeometry {
  const position: number[] = [];
  for (let blade = 0; blade < 5; blade++) {
    const a = (blade / 5) * 2 * Math.PI;
    const [c, s] = [Math.cos(a), Math.sin(a)];
    position.push(-0.04 * s, 0.04 * c, 0, 0.04 * s, -0.04 * c, 0, 0.35 * c, 0.35 * s, 1);
  }
  const count = position.length / 3;
  return new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
    .setAttribute('normal', new THREE.Float32BufferAttribute(new Array(count).fill([0, 0, 1]).flat(), 3))
    .setAttribute('part', new THREE.Float32BufferAttribute(new Array(count).fill(1), 1));
}

/** Grass tufts on grass and crop texels within this many metres, on a jittered 1 m grid. */
const GRASS_RADIUS_M = 60;

/** One instance per tree: x, y, height and a tint from 0 to 1. */
function forest(model: THREE.BufferGeometry, trees: number[]): THREE.InstancedBufferGeometry {
  const geometry = new THREE.InstancedBufferGeometry();
  for (const name of ['position', 'normal', 'part']) geometry.setAttribute(name, model.getAttribute(name));
  geometry.setAttribute('tree', new THREE.InstancedBufferAttribute(new Float32Array(trees), 4));
  geometry.instanceCount = trees.length / 4;
  return geometry;
}

/** Walls and flat roofs in one geometry, heights relative to the eye's. */
function town(buildings: Building[], eye: number): THREE.BufferGeometry {
  const position: number[] = [];
  const normal: number[] = [];
  const tint: number[] = [];
  const [wall, roof] = [new THREE.Color(WALL), new THREE.Color(ROOF)];
  const vertex = (x: number, y: number, z: number, n: number[], c: THREE.Color) => {
    position.push(x, y, z);
    normal.push(...n);
    tint.push(c.r, c.g, c.b);
  };
  for (const [base, height, ring] of buildings) {
    const outline: THREE.Vector2[] = [];
    for (let i = 0; i < ring.length; i += 2) outline.push(new THREE.Vector2(ring[i], ring[i + 1]));
    if (THREE.ShapeUtils.isClockWise(outline)) outline.reverse();
    const [z0, z1] = [base - eye, base + height - eye];
    outline.forEach((a, i) => {
      const b = outline[(i + 1) % outline.length];
      const length = a.distanceTo(b) || 1;
      const out = [(b.y - a.y) / length, (a.x - b.x) / length, 0];
      for (const [p, z] of [[a, z0], [b, z0], [b, z1], [a, z0], [b, z1], [a, z1]] as const) vertex(p.x, p.y, z, out, wall);
    });
    for (const face of THREE.ShapeUtils.triangulateShape(outline, [])) {
      for (const i of face) vertex(outline[i].x, outline[i].y, z1, [0, 0, 1], roof);
    }
  }
  return new THREE.BufferGeometry()
    .setAttribute('position', new THREE.Float32BufferAttribute(position, 3))
    .setAttribute('normal', new THREE.Float32BufferAttribute(normal, 3))
    .setAttribute('tint', new THREE.Float32BufferAttribute(tint, 3));
}

export class NearField {
  readonly trees = new THREE.Group();
  readonly buildings = new THREE.Group();
  /** Triangles the trees and buildings add to a frame. */
  triangles = 0;
  private readonly models = { conifer: conifer(), broadleaf: broadleaf(), grass: tuft() };
  private readonly materials: {
    conifer: THREE.ShaderMaterial;
    broadleaf: THREE.ShaderMaterial;
    grass: THREE.ShaderMaterial;
    building: THREE.ShaderMaterial;
  };

  constructor(uniforms: Record<string, THREE.IUniform>) {
    const material = (vertexShader: string, own: Record<string, THREE.IUniform> = {}) =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        vertexShader,
        fragmentShader: objectFragment,
        // The shared uniform objects, plus this material's own.
        uniforms: { ...uniforms, ...own },
        side: THREE.DoubleSide,
      });
    const tree = () => ({ foliage: { value: new THREE.Color() }, bark: { value: new THREE.Color(BARK) } });
    this.materials = {
      conifer: material(treeVertex, tree()),
      broadleaf: material(treeVertex, tree()),
      grass: material(treeVertex, tree()),
      building: material(buildingVertex),
    };
  }

  set(bake: Bake): void {
    for (const group of [this.trees, this.buildings]) {
      for (const mesh of group.children as THREE.Mesh[]) mesh.geometry.dispose();
      group.clear();
    }
    const kinds: Record<'conifer' | 'broadleaf', number[]> = { conifer: [], broadleaf: [] };
    const { trees } = bake;
    for (let i = 0; i < trees.length; i += 4) {
      // A steady tint per tree, so the same forest looks the same every time.
      const tint = Math.abs(Math.sin(trees[i] * 12.9898 + trees[i + 1] * 78.233) * 43758.5453) % 1;
      kinds[trees[i + 3] > 0.5 ? 'broadleaf' : 'conifer'].push(trees[i], trees[i + 1], trees[i + 2], tint);
    }
    this.triangles = 0;
    for (const kind of ['conifer', 'broadleaf'] as const) {
      if (!kinds[kind].length) continue;
      const mesh = new THREE.Mesh(forest(this.models[kind], kinds[kind]), this.materials[kind]);
      mesh.frustumCulled = false;
      this.trees.add(mesh);
      this.triangles += ((kinds[kind].length / 4) * this.models[kind].getAttribute('position').count) / 3;
    }
    const tufts = meadow(bake);
    if (tufts.length) {
      const mesh = new THREE.Mesh(forest(this.models.grass, tufts), this.materials.grass);
      mesh.frustumCulled = false;
      mesh.name = 'grass';
      this.trees.add(mesh);
      this.triangles += ((tufts.length / 4) * this.models.grass.getAttribute('position').count) / 3;
    }
    if (bake.buildings.length) {
      const mesh = new THREE.Mesh(town(bake.buildings, bake.meta.eye_m), this.materials.building);
      mesh.frustumCulled = false;
      this.buildings.add(mesh);
      this.triangles += mesh.geometry.getAttribute('position').count / 3;
    }
  }

  /** The season's needle, broadleaf and grass colours; no tufts stand in winter. */
  setFoliage(needle: THREE.Color, broad: THREE.Color, grass: THREE.Color | null): void {
    this.materials.conifer.uniforms.foliage.value.copy(needle);
    this.materials.broadleaf.uniforms.foliage.value.copy(broad);
    if (grass) this.materials.grass.uniforms.foliage.value.copy(grass);
    const tufts = this.trees.getObjectByName('grass');
    if (tufts) tufts.visible = grass !== null;
  }
}

/** x, y, height, tint per tuft on level 0's grass and crop texels, off the roads. */
function meadow(bake: Bake): number[] {
  const { size, half0_m } = bake.meta;
  const px = (2 * half0_m) / size;
  // The road edge distance filtered like the shader's, so tufts stop where the painted road starts.
  const edge = (x: number, y: number) => {
    const [u, v] = [(x + half0_m) / px - 0.5, (half0_m - y) / px - 0.5];
    const [i, j] = [Math.floor(u), Math.floor(v)];
    const [fu, fv] = [u - i, v - j];
    const r = (di: number, dj: number) => bake.roads[((j + dj) * size + i + di) * 2] / 10 - 5;
    return (r(0, 0) * (1 - fu) + r(1, 0) * fu) * (1 - fv) + (r(0, 1) * (1 - fu) + r(1, 1) * fu) * fv;
  };
  const tufts: number[] = [];
  let seed = 7;
  const random = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
  for (let y = -GRASS_RADIUS_M; y < GRASS_RADIUS_M; y += 1) {
    for (let x = -GRASS_RADIUS_M; x < GRASS_RADIUS_M; x += 1) {
      const [tx, ty] = [x + random() - 0.5, y + random() - 0.5];
      if (Math.hypot(tx, ty) > GRASS_RADIUS_M || Math.hypot(tx, ty) < 1.5) continue;
      const texel = Math.floor((half0_m - ty) / px) * size + Math.floor((tx + half0_m) / px);
      const kind = bake.classes[texel * 2];
      if ((kind !== 2 && kind !== 3) || edge(tx, ty) < 0.2 || bake.water[texel] > 64) continue;
      tufts.push(tx, ty, 0.25 + 0.25 * random(), random());
    }
  }
  return tufts;
}
