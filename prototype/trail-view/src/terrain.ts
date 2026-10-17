/**
 * The terrain: one static polar mesh around the eye, Peak View's rays turned into triangles.
 * Rings are spaced geometrically, so screen density is constant and there is no LOD. The eye is
 * the origin, x east, y north, z up, so float32 is precise everywhere.
 */

import * as THREE from 'three';
import type { Bake } from './bake';
import { CURVATURE } from './skyline';

const AZIMUTHS = 2048;
const WEDGES = 32;
const PER_WEDGE = AZIMUTHS / WEDGES;
const WEDGE_DEG = 360 / WEDGES;
const R0 = 1;
const R1 = 450_000;
const RINGS = 1270;
const Q = (R1 / R0) ** (1 / (RINGS - 1));

/** The near pass draws rings out to here; the far pass starts here and its near plane hides the rest. */
const NEAR_RING = Math.ceil(Math.log(3_000 / R0) / Math.log(Q));
const FAR_RING = Math.floor(Math.log(100 / R0) / Math.log(Q));
export const NEAR_PASS = { near: 0.5, far: 6_000 };
export const FAR_PASS = { near: 150, far: 500_000 };

const vertexShader = /* glsl */ `
precision highp sampler2DArray;
uniform sampler2DArray heights;
uniform float half0;
uniform float levels;
uniform float dtmLevels;
uniform bool glo30Only;
uniform float lift;
uniform float curvature;

out vec3 vWorld;
out vec3 vNormal;
out float vInside;

float levelHeight(float k, vec2 p) {
  float half_ = half0 * exp2(k);
  vec2 uv = vec2(p.x, -p.y) / (2.0 * half_) + 0.5;
  float layer = glo30Only && k < dtmLevels ? levels + k : k;
  return textureLod(heights, vec3(uv, layer), 0.0).r;
}

// The same rule as Surface.level in skyline.ts.
float heightOn(float k, vec2 p) {
  float h = levelHeight(k, p);
  if (k < levels - 1.0) {
    float w = smoothstep(0.9, 1.0, max(abs(p.x), abs(p.y)) / (half0 * exp2(k)));
    if (w > 0.0) h = mix(h, levelHeight(k + 1.0, p), w);
  }
  return h;
}

void main() {
  vec2 p = (modelMatrix * vec4(position, 1.0)).xy;
  float r = length(p);
  float m = max(abs(p.x), abs(p.y));
  float k = max(0.0, ceil(log2(max(m, 1.0) / half0)));
  vInside = k < levels ? 1.0 : 0.0;
  k = min(k, levels - 1.0);
  float h = heightOn(k, p);
  // The normal at the mesh's own scale: a ring step or a level pixel, whichever is larger.
  float e = max(2.0 * half0 * exp2(k) / 1024.0, 0.0103 * r);
  float dx = heightOn(k, p + vec2(e, 0.0)) - heightOn(k, p - vec2(e, 0.0));
  float dy = heightOn(k, p + vec2(0.0, e)) - heightOn(k, p - vec2(0.0, e));
  vNormal = normalize(vec3(-dx, -dy, 2.0 * e));
  vWorld = vec3(p, h - lift - curvature * r * r);
  gl_Position = projectionMatrix * viewMatrix * vec4(vWorld, 1.0);
}
`;

const fragmentShader = /* glsl */ `
uniform vec3 ground;
uniform vec3 horizon;
uniform vec3 light;
uniform float shade;
uniform float hazeKm;

in vec3 vWorld;
in vec3 vNormal;
in float vInside;
out vec4 colour;

void main() {
  if (vInside < 0.5) discard;
  float lambert = max(dot(normalize(vNormal), light), 0.0);
  vec3 c = ground * mix(1.0, 0.35 + 0.9 * lambert, shade);
  float haze = 1.0 - exp(-length(vWorld) / (hazeKm * 1000.0));
  colour = vec4(mix(c, horizon, haze), 1.0);
}
`;

const skyVertex = /* glsl */ `
out vec2 vNdc;
void main() {
  vNdc = position.xy;
  gl_Position = vec4(position.xy, 1.0, 1.0);
}
`;

const skyFragment = /* glsl */ `
uniform mat4 inverseProjection;
uniform mat4 cameraWorld;
uniform vec3 horizon;
uniform vec3 zenith;
in vec2 vNdc;
out vec4 colour;
void main() {
  vec4 view = inverseProjection * vec4(vNdc, 1.0, 1.0);
  vec3 dir = normalize((cameraWorld * vec4(view.xyz / view.w, 0.0)).xyz);
  float up = clamp(dir.z, 0.0, 1.0);
  colour = vec4(mix(horizon, zenith, sqrt(up)), 1.0);
}
`;

/** One wedge, in its own frame: azimuths 0 to 11.25° clockwise from north, and a centre fan. */
function wedgeGeometries(): { near: THREE.BufferGeometry; far: THREE.BufferGeometry } {
  const columns = PER_WEDGE + 1;
  const positions = new Float32Array((1 + RINGS * columns) * 3);
  let v = 3;
  for (let i = 0; i < RINGS; i++) {
    const r = R0 * Q ** i;
    for (let j = 0; j < columns; j++) {
      const azimuth = (j / AZIMUTHS) * 2 * Math.PI;
      positions[v++] = r * Math.sin(azimuth);
      positions[v++] = r * Math.cos(azimuth);
      positions[v++] = 0;
    }
  }
  const vertex = (ring: number, j: number) => 1 + ring * columns + j;
  const index = new Uint32Array(PER_WEDGE * 3 + (RINGS - 1) * PER_WEDGE * 6);
  let n = 0;
  // Counter-clockwise from above: azimuth runs clockwise.
  for (let j = 0; j < PER_WEDGE; j++) index.set([0, vertex(0, j + 1), vertex(0, j)], (n += 3) - 3);
  for (let i = 0; i < RINGS - 1; i++) {
    for (let j = 0; j < PER_WEDGE; j++) {
      const a = vertex(i, j);
      const b = vertex(i, j + 1);
      const c = vertex(i + 1, j);
      const d = vertex(i + 1, j + 1);
      index.set([a, b, c, b, d, c], n);
      n += 6;
    }
  }
  const position = new THREE.BufferAttribute(positions, 3);
  const indices = new THREE.BufferAttribute(index, 1);
  const ringStart = (ring: number) => PER_WEDGE * 3 + ring * PER_WEDGE * 6;
  const near = new THREE.BufferGeometry().setAttribute('position', position).setIndex(indices);
  near.setDrawRange(0, ringStart(NEAR_RING));
  const far = new THREE.BufferGeometry().setAttribute('position', position).setIndex(indices);
  far.setDrawRange(ringStart(FAR_RING), index.length - ringStart(FAR_RING));
  return { near, far };
}

export interface Look {
  ground: string;
  horizon: string;
  zenith: string;
  shade: number;
  hazeKm: number;
}

export class Terrain {
  readonly far = new THREE.Scene();
  readonly near = new THREE.Scene();
  /** Triangles one wedge draws over both passes. */
  private readonly perWedge: number;
  private readonly material: THREE.ShaderMaterial;
  private readonly sky: THREE.ShaderMaterial;
  private readonly wedges: THREE.Mesh[] = [];
  private texture?: THREE.DataArrayTexture;

  constructor() {
    const geometry = wedgeGeometries();
    this.perWedge = (geometry.near.drawRange.count + geometry.far.drawRange.count) / 3;
    this.material = new THREE.ShaderMaterial({
      glslVersion: THREE.GLSL3,
      vertexShader,
      fragmentShader,
      uniforms: {
        heights: { value: null },
        half0: { value: 1250 },
        levels: { value: 9 },
        dtmLevels: { value: 0 },
        glo30Only: { value: false },
        lift: { value: 0 },
        curvature: { value: CURVATURE },
        ground: { value: new THREE.Color() },
        horizon: { value: new THREE.Color() },
        light: { value: new THREE.Vector3(-0.5, 0.6, 0.62).normalize() },
        shade: { value: 1 },
        hazeKm: { value: 60 },
      },
    });
    this.sky = new THREE.ShaderMaterial({
      glslVersion: THREE.GLSL3,
      vertexShader: skyVertex,
      fragmentShader: skyFragment,
      depthTest: false,
      depthWrite: false,
      uniforms: {
        inverseProjection: { value: new THREE.Matrix4() },
        cameraWorld: { value: new THREE.Matrix4() },
        horizon: { value: this.material.uniforms.horizon.value },
        zenith: { value: new THREE.Color() },
      },
    });
    const sky = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), this.sky);
    sky.frustumCulled = false;
    sky.renderOrder = -1;
    this.far.add(sky);
    for (const [scene, wedge] of [
      [this.far, geometry.far],
      [this.near, geometry.near],
    ] as const) {
      for (let w = 0; w < WEDGES; w++) {
        const mesh = new THREE.Mesh(wedge, this.material);
        mesh.rotation.z = (-w * 2 * Math.PI) / WEDGES;
        mesh.frustumCulled = false;
        mesh.userData.wedge = w;
        scene.add(mesh);
        this.wedges.push(mesh);
      }
    }
  }

  /** Upload the levels as one R16F array: the surface, then GLO-30 for the levels the DTM replaced. */
  setBake(bake: Bake): void {
    const { size, levels, half0_m, dtm_levels } = bake.meta;
    const layers = [...bake.surface, ...bake.glo30];
    const data = new Uint16Array(size * size * layers.length);
    const Float16 = (globalThis as { Float16Array?: new (a: Float32Array) => ArrayLike<number> & { buffer: ArrayBuffer } })
      .Float16Array;
    layers.forEach((layer, i) => {
      const half = Float16 ? new Uint16Array(new Float16(layer).buffer) : layer.map(THREE.DataUtils.toHalfFloat);
      data.set(half, i * size * size);
    });
    this.texture?.dispose();
    const texture = new THREE.DataArrayTexture(data, size, size, layers.length);
    texture.type = THREE.HalfFloatType;
    texture.format = THREE.RedFormat;
    texture.internalFormat = 'R16F';
    texture.minFilter = texture.magFilter = THREE.LinearFilter;
    texture.generateMipmaps = false;
    texture.needsUpdate = true;
    this.texture = texture;
    const u = this.material.uniforms;
    u.heights.value = texture;
    u.half0.value = half0_m;
    u.levels.value = levels;
    u.dtmLevels.value = dtm_levels;
  }

  /** Which levels to draw, and how far their values sit above heights relative to the eye. */
  setSurface(glo30Only: boolean, lift: number): void {
    this.material.uniforms.glo30Only.value = glo30Only;
    this.material.uniforms.lift.value = lift;
  }

  setLook(look: Look): void {
    const u = this.material.uniforms;
    u.ground.value.set(look.ground);
    u.horizon.value.set(look.horizon);
    this.sky.uniforms.zenith.value.set(look.zenith);
    u.shade.value = look.shade;
    u.hazeKm.value = look.hazeKm;
  }

  /** Hide the wedges outside the view, and aim the sky. Returns the triangles both passes draw. */
  prepare(camera: THREE.PerspectiveCamera): number {
    camera.updateMatrixWorld();
    this.sky.uniforms.inverseProjection.value.copy(camera.projectionMatrixInverse);
    this.sky.uniforms.cameraWorld.value.copy(camera.matrixWorld);
    const span = azimuthSpan(camera);
    let drawn = 0;
    for (const mesh of this.wedges) {
      const centre = (mesh.userData.wedge + 0.5) * WEDGE_DEG;
      mesh.visible = !span || Math.abs(wrap(centre - span.mid)) <= span.half + WEDGE_DEG / 2 + 1;
      if (mesh.visible) drawn++;
    }
    return (drawn / 2) * this.perWedge;
  }
}

const wrap = (degrees: number) => ((((degrees + 180) % 360) + 360) % 360) - 180;

/** The azimuths the view can see, or null when it holds the nadir or the zenith and so sees all. */
function azimuthSpan(camera: THREE.PerspectiveCamera): { mid: number; half: number } | null {
  for (const pole of [-1, 1]) {
    const ndc = new THREE.Vector3(0, 0, pole).applyMatrix4(camera.matrixWorldInverse);
    if (ndc.z >= 0) continue;
    ndc.applyMatrix4(camera.projectionMatrix);
    if (Math.abs(ndc.x) <= 1 && Math.abs(ndc.y) <= 1) return null;
  }
  const forward = camera.getWorldDirection(new THREE.Vector3());
  const yaw = (Math.atan2(forward.x, forward.y) * 180) / Math.PI;
  let low = 0;
  let high = 0;
  for (const [x, y] of [[-1, -1], [1, -1], [-1, 1], [1, 1], [-1, 0], [1, 0]]) {
    const ray = new THREE.Vector3(x, y, 0.5).unproject(camera);
    const delta = wrap((Math.atan2(ray.x, ray.y) * 180) / Math.PI - yaw);
    low = Math.min(low, delta);
    high = Math.max(high, delta);
  }
  return { mid: yaw + (low + high) / 2, half: (high - low) / 2 };
}
