/**
 * The terrain: one static polar mesh around the eye, Peak View's rays turned into triangles.
 * Rings are spaced geometrically, so screen density is constant and there is no LOD. The eye is
 * the origin, x east, y north, z up, so float32 is precise everywhere.
 */

import * as THREE from 'three';
import type { Bake } from './bake';
import type { Light } from './light';
import { NearField } from './nearfield';
import { ROADS, ROCK, SHORE } from './palette';
import { fullscreenVertex, shadowFragment, skyFragment, terrainFragment, terrainVertex } from './shaders';
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
  /** 0 takes the ground's chroma from Sentinel-2, 1 paints the class palette alone. */
  snap: number;
  /** Visibility at sea level, in km; the fog thins with altitude. */
  hazeKm: number;
  /** Scale height of the fog, in metres. */
  hazeHeight: number;
  /** Top of the valley mist above sea level, in metres; 0 is none. */
  mistTop: number;
  /** Width of the steps between the three light bands. */
  softness: number;
  /** How much of the smooth sunlight term is kept beside the bands. */
  gradient: number;
}

export class Terrain {
  readonly far = new THREE.Scene();
  readonly near = new THREE.Scene();
  /** Triangles one wedge draws over both passes. */
  private readonly perWedge: number;
  private readonly uniforms: Record<string, THREE.IUniform>;
  private readonly sky: THREE.ShaderMaterial;
  private readonly wedges: THREE.Mesh[] = [];
  private texture?: THREE.DataArrayTexture;
  private classes?: THREE.DataArrayTexture;
  private colour?: THREE.DataArrayTexture;
  private canopy?: THREE.DataArrayTexture;
  private roads?: THREE.DataArrayTexture;
  readonly nearField: NearField;
  private readonly shadows = new THREE.WebGLRenderTarget(AZIMUTHS, RINGS, {
    format: THREE.RedFormat,
    type: THREE.UnsignedByteType,
    wrapS: THREE.RepeatWrapping,
    minFilter: THREE.LinearFilter,
    magFilter: THREE.LinearFilter,
    depthBuffer: false,
  });
  private readonly shadowPass = new THREE.Scene();
  private shadowsStale = true;

  constructor() {
    const geometry = wedgeGeometries();
    this.perWedge = (geometry.near.drawRange.count + geometry.far.drawRange.count) / 3;
    // One set of uniform objects, shared by every pass.
    const u = (this.uniforms = {
      heights: { value: null },
      half0: { value: 1250 },
      levels: { value: 9 },
      dtmLevels: { value: 0 },
      glo30Only: { value: false },
      lift: { value: 0 },
      curvature: { value: CURVATURE },
      r0: { value: R0 },
      ringStep: { value: Math.log(Q) },
      rings: { value: RINGS },
      azimuths: { value: AZIMUTHS },
      shadows: { value: this.shadows.texture },
      sunDir: { value: new THREE.Vector3(0, 0, 1) },
      sunColour: { value: new THREE.Color() },
      ambient: { value: new THREE.Color() },
      zenith: { value: new THREE.Color() },
      horizon: { value: new THREE.Color() },
      glow: { value: 0 },
      night: { value: 0 },
      classes: { value: null },
      s2: { value: null },
      s2Levels: { value: 0 },
      ramps: { value: [] as THREE.Color[] },
      snap: { value: 0.4 },
      rock: { value: new THREE.Color(ROCK) },
      shore: { value: new THREE.Color(SHORE) },
      canopy: { value: null },
      canopyLevels: { value: 0 },
      roads: { value: null },
      roadLevels: { value: 0 },
      roadColours: { value: ROADS.map((hex) => new THREE.Color(hex)) },
      softness: { value: 0.08 },
      gradient: { value: 0.35 },
      eyeAltitude: { value: 0 },
      hazeKm: { value: 35 },
      hazeHeight: { value: 2500 },
      mistTop: { value: 0 },
      inverseProjection: { value: new THREE.Matrix4() },
      cameraWorld: { value: new THREE.Matrix4() },
    });
    const material = (vertexShader: string, fragmentShader: string, flat = false) =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        vertexShader,
        fragmentShader,
        uniforms: u,
        depthTest: !flat,
        depthWrite: !flat,
      });
    const terrain = material(terrainVertex, terrainFragment);
    this.sky = material(fullscreenVertex, skyFragment, true);
    const quad = (shader: THREE.ShaderMaterial) => {
      const mesh = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), shader);
      mesh.frustumCulled = false;
      return mesh;
    };
    const sky = quad(this.sky);
    sky.renderOrder = -1;
    this.far.add(sky);
    this.shadowPass.add(quad(material(fullscreenVertex, shadowFragment, true)));
    this.nearField = new NearField(u);
    this.near.add(this.nearField.trees, this.nearField.buildings);
    for (const [scene, wedge] of [
      [this.far, geometry.far],
      [this.near, geometry.near],
    ] as const) {
      for (let w = 0; w < WEDGES; w++) {
        const mesh = new THREE.Mesh(wedge, terrain);
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
    const u = this.uniforms;
    u.heights.value = texture;
    u.half0.value = half0_m;
    u.levels.value = levels;
    u.dtmLevels.value = dtm_levels;
    this.classes?.dispose();
    this.classes = arrayTexture(bake.classes, size, levels, THREE.RGFormat, 'RG8');
    u.classes.value = this.classes;
    this.canopy?.dispose();
    this.canopy = dtm_levels ? arrayTexture(bake.canopy, size, dtm_levels, THREE.RedFormat, 'R8') : undefined;
    u.canopy.value = this.canopy ?? this.classes;
    u.canopyLevels.value = dtm_levels;
    this.roads?.dispose();
    this.roads = arrayTexture(bake.roads, size, bake.meta.road_levels, THREE.RGFormat, 'RG8');
    u.roads.value = this.roads;
    u.roadLevels.value = bake.meta.road_levels;
    this.nearField.set(bake);
    this.setColour(null, 0);
    this.shadowsStale = true;
  }

  /** One season's Sentinel-2 levels, or none: the class palette alone. */
  setColour(data: Uint8Array | null, levels: number): void {
    this.colour?.dispose();
    this.colour = data ? arrayTexture(data, 1024, levels, THREE.RGBAFormat, 'RGBA8') : undefined;
    this.uniforms.s2.value = this.colour ?? this.classes;
    this.uniforms.s2Levels.value = data ? levels : 0;
  }

  setPalette(ramps: THREE.Color[]): void {
    this.uniforms.ramps.value = ramps;
  }

  /**
   * Which levels to draw, how far their values sit above heights relative to the eye, and the
   * eye's altitude above sea level.
   */
  setSurface(glo30Only: boolean, lift: number, eyeAltitude: number): void {
    this.uniforms.glo30Only.value = glo30Only;
    this.uniforms.lift.value = lift;
    this.uniforms.eyeAltitude.value = eyeAltitude;
    this.shadowsStale = true;
  }

  setLight(light: Light): void {
    const u = this.uniforms;
    if (!u.sunDir.value.equals(light.sun)) this.shadowsStale = true;
    u.sunDir.value.copy(light.sun);
    u.sunColour.value.copy(light.sunColour);
    u.ambient.value.copy(light.ambient);
    u.zenith.value.copy(light.zenith);
    u.horizon.value.copy(light.horizon);
    u.glow.value = light.glow;
    u.night.value = light.night;
  }

  setLook(look: Look): void {
    const u = this.uniforms;
    u.snap.value = look.snap;
    u.hazeKm.value = look.hazeKm;
    u.hazeHeight.value = look.hazeHeight;
    u.mistTop.value = look.mistTop;
    u.softness.value = look.softness;
    u.gradient.value = look.gradient;
  }

  /** March every vertex toward the sun again, if the sun or the surface moved. */
  shade(renderer: THREE.WebGLRenderer, camera: THREE.Camera): boolean {
    if (!this.shadowsStale || !this.texture) return false;
    this.shadowsStale = false;
    renderer.setRenderTarget(this.shadows);
    renderer.render(this.shadowPass, camera);
    renderer.setRenderTarget(null);
    return true;
  }

  /** Hide the wedges outside the view, and aim the sky. Returns the triangles both passes draw. */
  prepare(camera: THREE.PerspectiveCamera): number {
    camera.updateMatrixWorld();
    this.uniforms.inverseProjection.value.copy(camera.projectionMatrixInverse);
    this.uniforms.cameraWorld.value.copy(camera.matrixWorld);
    const span = azimuthSpan(camera);
    let drawn = 0;
    for (const mesh of this.wedges) {
      const centre = (mesh.userData.wedge + 0.5) * WEDGE_DEG;
      mesh.visible = !span || Math.abs(wrap(centre - span.mid)) <= span.half + WEDGE_DEG / 2 + 1;
      if (mesh.visible) drawn++;
    }
    const objects = this.nearField.trees.visible || this.nearField.buildings.visible ? this.nearField.triangles : 0;
    return (drawn / 2) * this.perWedge + objects;
  }
}

function arrayTexture(data: Uint8Array, size: number, depth: number, format: THREE.PixelFormat, internal: THREE.PixelFormatGPU) {
  const texture = new THREE.DataArrayTexture(data, size, size, depth);
  texture.format = format;
  texture.internalFormat = internal;
  texture.minFilter = texture.magFilter = THREE.LinearFilter;
  texture.generateMipmaps = false;
  texture.needsUpdate = true;
  return texture;
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
