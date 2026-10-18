/**
 * The terrain: one static polar mesh around the eye, Peak View's rays turned into triangles.
 * Rings are spaced geometrically, so screen density is constant and there is no LOD. The eye is
 * the origin, x east, y north, z up, so float32 is precise everywhere.
 */

import * as THREE from 'three';
import { HABITATS, type Bake } from './bake';
import type { Light } from './light';
import type { Region } from './regions';
import { NearField } from './nearfield';
import { ROAD_LINE, ROADS, VARIEGATED } from './palette';
import { cloudMapFragment, deriveFragment, fullscreenVertex, rainFragment, shadowFragment, skyFragment, terrainFragment, terrainVertex } from './shaders';
import { CURVATURE } from './surface';
import type { Weather } from './weather';

/** Open water upwind beyond this counts as the open ocean. */
const FETCH_M = 8000;

/**
 * Per water texel at sea level and level, 0–255: how far open water reaches toward the swell, up
 * to FETCH_M, averaged over the swell's bearing and 45° to each side, since swell bends round
 * headlands. Coarse levels first, so a fine level's walk continues past its edge on the next.
 * Without a swell every shore counts as half exposed.
 */
function exposure(bake: Bake, swell: number | undefined): Uint8Array {
  const { size, water_levels } = bake.meta;
  const out = new Uint8Array(water_levels * size * size);
  if (swell === undefined) return out.fill(128);
  const sum = new Float32Array(out.length);
  for (const bearing of [swell - 45, swell, swell + 45]) {
    const fetch = fetchToward(bake, bearing);
    for (let t = 0; t < sum.length; t++) sum[t] += fetch[t] / 3;
  }
  for (let t = 0; t < sum.length; t++) out[t] = Math.round((255 * sum[t]) / FETCH_M);
  return out;
}

/** Metres of open sea from each sea texel toward a bearing, up to FETCH_M; 0 elsewhere. */
function fetchToward(bake: Bake, bearing: number): Float32Array {
  const { size, water_levels, half0_m, eye_m } = bake.meta;
  const a = (bearing * Math.PI) / 180;
  const [di, dj] = [Math.round(Math.sin(a)), Math.round(-Math.cos(a))];
  const fetch = new Float32Array(water_levels * size * size);
  for (let k = water_levels - 1; k >= 0; k--) {
    const step = (Math.hypot(di, dj) * 2 * half0_m * 2 ** k) / size;
    const base = k * size * size;
    const coarser = (i: number, j: number) => {
      if (k + 1 >= water_levels) return FETCH_M;
      const [ci, cj] = [Math.round((i - size / 2) / 2 + size / 2), Math.round((j - size / 2) / 2 + size / 2)];
      return fetch[(k + 1) * size * size + cj * size + ci];
    };
    for (let jj = 0; jj < size; jj++) {
      const j = dj > 0 ? size - 1 - jj : jj;
      for (let ii = 0; ii < size; ii++) {
        const i = di > 0 ? size - 1 - ii : ii;
        const t = base + j * size + i;
        if (bake.classes[t * 2 + 1] < 128 || bake.surface[k][j * size + i] + eye_m > 2) continue;
        const [ni, nj] = [i + di, j + dj];
        const ahead = ni < 0 || nj < 0 || ni >= size || nj >= size ? coarser(ni, nj) : fetch[base + nj * size + ni];
        fetch[t] = Math.min(FETCH_M, ahead + step);
      }
    }
  }
  return fetch;
}

/** How far the clouds drift in an hour of the day, in metres east and north: slow enough for a day played in 30 s. */
const DRIFT_PER_HOUR = [2000, 700];

/** The cloud layer is drawn at least this far above the eye: a deck below it is not drawn. */
const CLOUD_ABOVE_EYE_M = 200;
/** The cloud map covers this many metres around the eye, 1024 texels across. */
const CLOUD_REACH_M = 40_000;

/** The standard normal quantile, Tukey's lambda approximation. */
const probit = (p: number) => 4.91 * (p ** 0.14 - (1 - p) ** 0.14);
// Mean and spread of the cloud noise (five octaves of value noise), measured over 400,000
// samples; with them a cover of 0.14, 0.62 or 0.97 puts 0.15, 0.61 or 0.97 of the sky under cloud.
const NOISE_MEAN = 0.486;
const NOISE_SPREAD = 0.123;

// A steep face seen head-on shows the ring spacing, about 0.67 % of the range: 20 m at 3 km, where
// the national DTM has 5 to 10 m texels. Coarser rings smear crags and gullies.
const AZIMUTHS = 3072;
const WEDGES = 32;
const PER_WEDGE = AZIMUTHS / WEDGES;
const WEDGE_DEG = 360 / WEDGES;
const R0 = 1;
// The last level's inscribed circle, so every fragment has data and none is discarded: a
// discard turns off the GPU's hidden-surface removal, and the terrain shader is costly.
const R1 = 320_000;
const RINGS = 1900;
const Q = (R1 / R0) ** (1 / (RINGS - 1));

/**
 * The near pass draws rings out to 3 km over the far pass, so the far pass starts just inside
 * that: every far fragment nearer than this would be shaded only to be drawn over. Its near plane
 * sits inside the nearest far ring's view depth at the corner of a wide view.
 */
const NEAR_RING = Math.ceil(Math.log(3_000 / R0) / Math.log(Q));
const FAR_RING = Math.floor(Math.log(2_500 / R0) / Math.log(Q));
export const NEAR_PASS = { near: 0.5, far: 6_000 };
export const FAR_PASS = { near: 800, far: 500_000 };

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
  /** Drawn over both passes: the rain. */
  readonly overlay = new THREE.Scene();
  /** Triangles one wedge draws over both passes. */
  private readonly perWedge: number;
  private readonly uniforms: Record<string, THREE.IUniform>;
  private readonly sky: THREE.ShaderMaterial;
  private readonly wedges: THREE.Mesh[] = [];
  private texture?: THREE.DataArrayTexture;
  private classes?: THREE.DataArrayTexture;
  private ndvimax?: THREE.DataArrayTexture;
  private colour?: THREE.DataArrayTexture;
  private canopy?: THREE.DataArrayTexture;
  private roads?: THREE.DataArrayTexture;
  private water?: THREE.DataArrayTexture;
  private exposed?: THREE.DataArrayTexture;
  private rockMap?: THREE.DataArrayTexture;
  private habitatMap?: THREE.DataArrayTexture;
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
  /** Slope and openness per level texel of the active surface. */
  private normals?: THREE.WebGLArrayRenderTarget;
  private readonly derivePass = new THREE.Scene();
  private surfaceStale = true;
  private look?: Look;
  private weather?: Weather;
  private readonly cloudMap = new THREE.WebGLRenderTarget(1024, 1024, {
    minFilter: THREE.LinearFilter,
    magFilter: THREE.LinearFilter,
    depthBuffer: false,
  });
  private readonly cloudPass = new THREE.Scene();
  private cloudsStale = true;

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
      ndviMax: { value: null },
      ndviMaxLevels: { value: 0 },
      barrenMedian: { value: 0 },
      turf: { value: 0.55 },
      debugView: { value: 0 },
      ramps: { value: Array.from({ length: 33 }, () => new THREE.Color()) },
      snap: { value: 0.4 },
      rock: { value: new THREE.Color() },
      cliffRock: { value: new THREE.Color() },
      snowline: { value: -1e4 },
      seaDeep: { value: new THREE.Color() },
      seaShallow: { value: new THREE.Color() },
      shoreReach: { value: 600 },
      moorColour: { value: new THREE.Color() },
      speckleColour: { value: new THREE.Color() },
      speckle: { value: new THREE.Vector2() },
      moorOn: { value: 0 },
      heather: { value: new THREE.Color() },
      heatherShare: { value: 0 },
      outcrops: { value: 0 },
      bedding: { value: new THREE.Vector3(0, 0, 1) },
      habitatMap: { value: null },
      habitatLevels: { value: 0 },
      habitatPaint: { value: HABITATS.map(() => new THREE.Vector4()) },
      canopy: { value: null },
      canopyLevels: { value: 0 },
      roads: { value: null },
      roadLevels: { value: 0 },
      roadColours: { value: ROADS.map((hex) => new THREE.Color(hex)) },
      roadLine: { value: new THREE.Color(ROAD_LINE) },
      normals: { value: null },
      nearShadow: { value: null },
      nearShadowMatrix: { value: new THREE.Matrix4() },
      deriveLevel: { value: 0 },
      waterMap: { value: null },
      exposureMap: { value: null },
      waterLevels: { value: 0 },
      rockMap: { value: null },
      rockLevels: { value: 0 },
      variegated: { value: VARIEGATED.map((hex) => new THREE.Color(hex)) },
      softness: { value: 0.08 },
      gradient: { value: 0.35 },
      eyeAltitude: { value: 0 },
      hazeKm: { value: 35 },
      hazeHeight: { value: 2500 },
      mistTop: { value: 0 },
      cloudCover: { value: 0 },
      cloudEdge: { value: 1 },
      cloudBase: { value: 2000 },
      cloudDark: { value: 0 },
      cloudDrift: { value: new THREE.Vector2() },
      wet: { value: 0 },
      towers: { value: 0 },
      gradeShadow: { value: new THREE.Vector3() },
      gradeLight: { value: new THREE.Vector3() },
      saturation: { value: 1 },
      cloudMap: { value: this.cloudMap.texture },
      cloudReach: { value: CLOUD_REACH_M },
      rain: { value: 0 },
      time: { value: 0 },
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
    this.derivePass.add(quad(material(fullscreenVertex, deriveFragment, true)));
    this.cloudPass.add(quad(material(fullscreenVertex, cloudMapFragment, true)));
    const rain = material(fullscreenVertex, rainFragment, true);
    rain.transparent = true;
    this.overlay.add(quad(rain));
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
  setBake(bake: Bake, region: Region): void {
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
    const ndviMaxLevels = bake.meta.barren_median ? bake.meta.s2_levels : 0;
    this.ndvimax?.dispose();
    this.ndvimax = ndviMaxLevels ? arrayTexture(bake.ndvimax, size, ndviMaxLevels, THREE.RedFormat, 'R8') : undefined;
    u.ndviMax.value = this.ndvimax ?? this.classes;
    u.ndviMaxLevels.value = ndviMaxLevels;
    this.canopy?.dispose();
    this.canopy = dtm_levels ? arrayTexture(bake.canopy, size, dtm_levels, THREE.RedFormat, 'R8') : undefined;
    u.canopy.value = this.canopy ?? this.classes;
    u.canopyLevels.value = dtm_levels;
    this.roads?.dispose();
    this.roads = arrayTexture(bake.roads, size, bake.meta.road_levels, THREE.RGBFormat, 'RGB8');
    u.roads.value = this.roads;
    u.roadLevels.value = bake.meta.road_levels;
    this.water?.dispose();
    this.water = arrayTexture(bake.water, size, bake.meta.water_levels, THREE.RGFormat, 'RG8');
    u.waterMap.value = this.water;
    this.exposed?.dispose();
    this.exposed = arrayTexture(exposure(bake, region.look.swell), size, bake.meta.water_levels, THREE.RedFormat, 'R8');
    u.exposureMap.value = this.exposed;
    u.waterLevels.value = bake.meta.water_levels;
    u.shoreReach.value = bake.meta.shore_reach_m;
    const land = region.look;
    u.rock.value.set(land.rock);
    (land.roads ?? ROADS).forEach((hex, i) => u.roadColours.value[i].set(hex));
    u.roadLine.value.set(land.roadLine ?? ROAD_LINE);
    u.cliffRock.value.set(land.cliff ?? land.rock);
    u.seaDeep.value.set(land.sea.deep);
    u.seaShallow.value.set(land.sea.shallow);
    u.moorColour.value.set(land.moor?.colour ?? land.rock);
    u.moorOn.value = land.moor ? 1 : 0;
    u.speckleColour.value.set(land.speckle?.colour ?? '#000');
    u.speckle.value.set(land.speckle?.share ?? 0, land.speckle?.size_m ?? 1);
    u.heather.value.set(land.moor?.heather ?? land.rock);
    u.heatherShare.value = land.moor?.share ?? 0;
    u.outcrops.value = land.outcrops;
    u.turf.value = Math.cos((land.turf_deg * Math.PI) / 180);
    const { grade } = land;
    u.gradeShadow.value.set(...grade.shadow);
    u.gradeLight.value.set(...grade.light);
    u.saturation.value = grade.saturation;
    // The bedding plane's normal: up, tipped by the dip toward the dip direction, 90° right of the strike.
    const { strike, dip } = land.bedding ?? { strike: 0, dip: 0 };
    const [toward, tilt] = [((strike + 90) * Math.PI) / 180, (dip * Math.PI) / 180];
    u.bedding.value.set(Math.sin(toward) * Math.sin(tilt), Math.cos(toward) * Math.sin(tilt), Math.cos(tilt));
    const habitatLevels = land.habitats ? (bake.meta.habitat_levels ?? 0) : 0;
    this.habitatMap?.dispose();
    this.habitatMap = habitatLevels ? arrayTexture(bake.habitat, size, habitatLevels, THREE.RedFormat, 'R8') : undefined;
    u.habitatMap.value = this.habitatMap ?? this.classes;
    u.habitatLevels.value = habitatLevels;
    HABITATS.forEach((name, i) => {
      const hex = land.habitats?.[name];
      const c = new THREE.Color(hex ?? '#000');
      u.habitatPaint.value[i].set(c.r, c.g, c.b, hex ? 1 : 0);
    });
    const { rock_levels } = bake.meta;
    this.rockMap?.dispose();
    this.rockMap = rock_levels ? arrayTexture(bake.rock, size, rock_levels, THREE.RGBAFormat, 'SRGB8_ALPHA8') : undefined;
    u.rockMap.value = this.rockMap ?? this.classes;
    u.rockLevels.value = rock_levels;
    this.normals?.dispose();
    this.normals = new THREE.WebGLArrayRenderTarget(size, size, levels, {
      type: THREE.HalfFloatType,
      minFilter: THREE.LinearFilter,
      magFilter: THREE.LinearFilter,
      depthBuffer: false,
    });
    u.normals.value = this.normals.texture;
    this.nearField.set(bake, region);
    this.setColour(null, 0);
    this.shadowsStale = this.surfaceStale = true;
  }

  /** One season's Sentinel-2 levels and its barren_median, or none: the class palette alone. */
  setColour(data: Uint8Array | null, levels: number, barrenMedian?: number | null): void {
    this.colour?.dispose();
    this.colour = data ? arrayTexture(data, 1024, levels, THREE.RGBAFormat, 'RGBA8') : undefined;
    this.uniforms.s2.value = this.colour ?? this.classes;
    this.uniforms.s2Levels.value = data ? levels : 0;
    this.uniforms.barrenMedian.value = barrenMedian ?? 0;
  }

  /** A false-colour view of one input (DEBUG_VIEWS in main.ts), 0 for none. */
  setDebug(view: number): void {
    this.uniforms.debugView.value = view;
  }

  /** The season's palette, and the altitude below which white is no snow (none in winter). */
  setPalette(ramps: THREE.Color[], snowline: number): void {
    this.uniforms.ramps.value = ramps;
    this.uniforms.snowline.value = snowline;
  }

  /**
   * Which levels to draw, how far their values sit above heights relative to the eye, and the
   * eye's altitude above sea level.
   */
  setSurface(glo30Only: boolean, lift: number, eyeAltitude: number): void {
    this.uniforms.glo30Only.value = glo30Only;
    this.uniforms.lift.value = lift;
    this.uniforms.eyeAltitude.value = eyeAltitude;
    this.shadowsStale = this.surfaceStale = this.nearField.shadowsStale = true;
  }

  /** The light; while the day plays, the shadows may follow the sun only every few frames. */
  setLight(light: Light, refreshShadows = true): void {
    const u = this.uniforms;
    if (refreshShadows && !u.sunDir.value.equals(light.sun)) this.shadowsStale = this.nearField.shadowsStale = true;
    u.sunDir.value.copy(light.sun);
    u.sunColour.value.copy(light.sunColour);
    u.ambient.value.copy(light.ambient);
    u.zenith.value.copy(light.zenith);
    u.horizon.value.copy(light.horizon);
    u.glow.value = light.glow;
    u.night.value = light.night;
  }

  /** Seconds, for the falling rain. */
  setTime(seconds: number): void {
    this.uniforms.time.value = seconds;
  }

  /** The cloud layer, wet and rain; the clouds stand where the hour has drifted them. */
  setWeather(weather: Weather, hour: number, eyeAltitude: number): void {
    const u = this.uniforms;
    this.weather = weather;
    u.cloudCover.value = weather.cover;
    u.cloudEdge.value = NOISE_MEAN + NOISE_SPREAD * probit(1 - weather.cover);
    u.cloudBase.value = Math.max(weather.base - eyeAltitude, CLOUD_ABOVE_EYE_M);
    u.cloudDark.value = weather.dark;
    u.cloudDrift.value.set(DRIFT_PER_HOUR[0] * hour, DRIFT_PER_HOUR[1] * hour);
    u.wet.value = weather.wet;
    u.towers.value = weather.towers ?? 0;
    u.rain.value = weather.rain;
    this.cloudsStale = true;
    if (this.look) this.setLook(this.look);
  }

  setLook(look: Look): void {
    const u = this.uniforms;
    this.look = look;
    u.snap.value = look.snap;
    u.hazeKm.value = look.hazeKm * (this.weather?.haze ?? 1);
    u.hazeHeight.value = look.hazeHeight;
    u.mistTop.value = look.mistTop;
    u.softness.value = look.softness;
    u.gradient.value = look.gradient;
  }

  /**
   * Derive the slopes again if the surface changed, and march every vertex toward the sun again
   * if the sun or the surface moved.
   */
  shade(renderer: THREE.WebGLRenderer, camera: THREE.Camera): boolean {
    if (!this.texture || !this.normals) return false;
    this.nearField.shade(renderer);
    if (this.cloudsStale) {
      this.cloudsStale = false;
      renderer.setRenderTarget(this.cloudMap);
      renderer.render(this.cloudPass, camera);
      renderer.setRenderTarget(null);
    }
    if (this.surfaceStale) {
      this.surfaceStale = false;
      for (let k = 0; k < this.normals.depth; k++) {
        this.uniforms.deriveLevel.value = k;
        renderer.setRenderTarget(this.normals, k);
        renderer.render(this.derivePass, camera);
      }
    }
    if (!this.shadowsStale) {
      renderer.setRenderTarget(null);
      return false;
    }
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
    const eye = camera.position;
    const away = Math.hypot(eye.x, eye.y) > 1;
    let drawn = 0;
    for (const mesh of this.wedges) {
      const centre = (mesh.userData.wedge + 0.5) * WEDGE_DEG;
      if (!span) mesh.visible = true;
      else if (!away) mesh.visible = Math.abs(wrap(centre - span.mid)) <= span.half + WEDGE_DEG / 2 + 1;
      else {
        // Off the spot, a wedge shows if any of its rays, out to its far rings, lies in the view
        // as seen from the eye.
        mesh.visible = false;
        for (const edge of [0, 0.5, 1]) {
          const azimuth = ((mesh.userData.wedge + edge) * WEDGE_DEG * Math.PI) / 180;
          for (let r = R0; r < R1 && !mesh.visible; r *= 3) {
            const bearing = (Math.atan2(r * Math.sin(azimuth) - eye.x, r * Math.cos(azimuth) - eye.y) * 180) / Math.PI;
            mesh.visible = Math.abs(wrap(bearing - span.mid)) <= span.half + 2;
          }
        }
      }
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
    const ndc = new THREE.Vector3(0, 0, pole).add(camera.position).applyMatrix4(camera.matrixWorldInverse);
    if (ndc.z >= 0) continue;
    ndc.applyMatrix4(camera.projectionMatrix);
    if (Math.abs(ndc.x) <= 1 && Math.abs(ndc.y) <= 1) return null;
  }
  const forward = camera.getWorldDirection(new THREE.Vector3());
  const yaw = (Math.atan2(forward.x, forward.y) * 180) / Math.PI;
  let low = 0;
  let high = 0;
  for (const [x, y] of [[-1, -1], [1, -1], [-1, 1], [1, 1], [-1, 0], [1, 0]]) {
    const ray = new THREE.Vector3(x, y, 0.5).unproject(camera).sub(camera.position);
    const delta = wrap((Math.atan2(ray.x, ray.y) * 180) / Math.PI - yaw);
    low = Math.min(low, delta);
    high = Math.max(high, delta);
  }
  return { mid: yaw + (low + high) / 2, half: (high - low) / 2 };
}
