import GUI from 'lil-gui';
import * as THREE from 'three';
import { clock } from 'planner-sun';
import { loadBake, loadQuarter, type Bake, type Photo } from './bake';
import { lightAt, SEASONS } from './light';
import { drawCompass } from './overlay';
import { SEASONS as SEASON_NAMES } from './models/trees';
import { DECIDUOUS, ramps } from './palette';
import { regionNamed, validate, type Region } from './regions';
import { CURVATURE, surface, type Surface } from './surface';
import { WEATHERS, weatherLight } from './weather';
import { FAR_PASS, NEAR_PASS, Terrain, type Look } from './terrain';

const element = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const stage = element('#stage');
const frame = element('#frame');
const photo = element<HTMLImageElement>('#photo');
const compass = element('#compass');
const status = element('#status');

/**
 * Pixels per CSS pixel at most. The terrain shader is bound by fill, and a painted look keeps its
 * character a little softer: 1.5 on a 2× screen draws 44 % fewer pixels than 2.
 */
const MAX_PIXEL_RATIO = 1.5;
const renderer = new THREE.WebGLRenderer({ canvas: element<HTMLCanvasElement>('#gl'), antialias: true });
renderer.setPixelRatio(Math.min(devicePixelRatio, MAX_PIXEL_RATIO));
renderer.autoClear = false;
const camera = new THREE.PerspectiveCamera(60, 1, FAR_PASS.near, FAR_PASS.far);
camera.up.set(0, 0, 1);
const terrain = new Terrain();

/** Data errors stay in the status line after every load, as npm run check prints them. */
const problems = validate();
if (problems.length) console.error(problems.join('\n'));
const idle = problems.length ? `${problems.length} data errors (npm run check): ${problems[0]}` : '';

/** The spots of each region, from its file spots/<region>.json. */
const REGION_SPOTS = Object.fromEntries(
  Object.entries(import.meta.glob<{ id: string; name: string }[]>('../spots/*.json', { eager: true, import: 'default' })).map(
    ([path, spots]) => [path.split('/').pop()!.replace(/\.json$/, ''), spots],
  ),
);
const SPOTS = Object.entries(REGION_SPOTS).flatMap(([region, spots]) => spots.map((spot) => ({ ...spot, region })));
/** The tune's status of each region, which only its coordinator writes. A region it does not name is not started. */
const STATUS: Record<string, string> = await fetch('/status.json', { cache: 'no-store' })
  .then((response) => response.json())
  .catch(() => ({}));

const DTM = 'national DTM';
const GLO30 = 'GLO-30 only';
const state = {
  region: SPOTS[0].region,
  preset: SPOTS[0].id,
  surface: DTM,
  photoCamera: true,
  /** The reference photograph in a corner of the view. */
  reference: true,
  trees: true,
  buildings: true,
  season: 'summer (15 Aug)',
  hour: 16,
  weather: 'sunny',
  /** Metres the eye rises above the ground, to see as a drone would. */
  rise: 0,
  /** Free movement, and metres east and north of the spot the eye has walked. */
  walk: false,
  x: 0,
  y: 0,
  yaw: 0,
  pitch: 0,
  vfov: 60,
  /** A false-colour view of one input, 0 for none. */
  debug: 0,
};
/** The views of `debugColour` in shaders.ts. */
const DEBUG_VIEWS = {
  off: 0,
  'WorldCover class': 1,
  'NDVI (red), annual max (green)': 2,
  barren: 3,
  'steep (red), skirt (blue)': 4,
  habitat: 5,
  'snow (red below the snowline)': 6,
  water: 7,
};
const look: Look = { snap: 0.4, hazeKm: 35, hazeHeight: 2500, mistTop: 0, softness: 0.08, gradient: 0.35 };
const readout = { region: '–', sun: '–', colour: '–', near: '–', dtm: '–', eye: '–', bytes: '–', triangles: '–', frame: '–' };

interface Loaded {
  bake: Bake;
  region: Region;
  ground: { dtm: Surface; glo30: Surface };
}
let loaded: Loaded | null = null;

const glo30Only = () => !loaded?.bake.meta.dtm_levels || state.surface === GLO30;
const active = <T>(pair: { dtm: T; glo30: T }) => (glo30Only() ? pair.glo30 : pair.dtm);
const photoLocked = () => Boolean(state.photoCamera && loaded?.bake.meta.photo && photo.naturalWidth);

let selecting = '';
async function select(id: string): Promise<void> {
  selecting = id;
  status.textContent = `Loading ${id}…`;
  loaded = null;
  frame.style.visibility = 'hidden';
  photo.removeAttribute('src');
  Object.assign(state, { preset: id, region: SPOTS.find((spot) => spot.id === id)!.region });
  regionPicker.updateDisplay();
  spotPicker.options(spotsOf(state.region));
  const bake = await loadBake(id);
  if (selecting !== id) return;
  const { meta } = bake;
  // A bake from before meta.region takes the region of its spots file.
  const region = regionNamed(meta.region ?? SPOTS.find((spot) => spot.id === id)!.region, [meta.subdivision, meta.country].filter((place): place is string => !!place));
  Object.assign(state, { x: 0, y: 0 });
  terrain.setBake(bake, region);
  const area = meta.size * meta.size;
  const canopy = [...Array(meta.dtm_levels).keys()].map((k) => Float32Array.from(bake.canopy.subarray(k * area, (k + 1) * area), (v) => v / 4));
  const dtm = surface(bake.surface, meta.size, meta.half0_m, meta.eye_above_ground_m, canopy);
  const glo30 = meta.dtm_levels
    ? surface([...bake.glo30, ...bake.surface.slice(meta.dtm_levels)], meta.size, meta.half0_m, meta.eye_above_ground_m)
    : dtm;
  loaded = { bake, region, ground: { dtm, glo30 } };
  const { snap, hazeKm, hazeHeight, mistTop, softness, gradient } = region.look;
  Object.assign(look, { snap, hazeKm, hazeHeight, mistTop, softness, gradient });
  terrain.setLook(look);
  readout.region = region.chain.join(' › ');
  const shot = meta.photo;
  if (shot) {
    // Only the photograph's size is needed. `decode()` waits for a rendered frame, which a hidden
    // page never gets.
    await new Promise((settled) => {
      photo.addEventListener('load', settled, { once: true });
      photo.addEventListener('error', settled, { once: true });
      photo.src = `${bake.base}${shot.file}`;
    });
    if (selecting !== id) return;
    Object.assign(align, { heading_deg: shot.heading_deg, pitch_deg: shot.pitch_deg, vfov_deg: photoVfov(shot), roll_deg: shot.roll_deg ?? 0 });
  }
  readout.bytes = `${(bake.bytes / 1e6).toFixed(1)} MB heights, classes, roads, canopy, trees`;
  readout.near = `${bake.trees.length / 4} trees, ${bake.buildings.length} buildings`;
  readout.dtm = meta.dtm_levels ? `${meta.dtm}, ±${(meta.half0_m * 2 ** (meta.dtm_levels - 1)) / 1000} km` : 'none';
  state.yaw = meta.photo?.heading_deg ?? meta.heading ?? 0;
  state.pitch = meta.photo?.pitch_deg ?? meta.pitch ?? 0;
  element('#credits').textContent = meta.credits.join(' · ');
  status.textContent = idle;
  frame.style.visibility = '';
  quarters.clear();
  surfaceChanged();
  await seasonChanged();
}

/** Sentinel-2 quarters of the current preset, by season index, loaded once each. */
const quarters = new Map<number, ReturnType<typeof loadQuarter>>();

/** The season's palette and light at once, then its Sentinel-2 quarter when it arrives. */
async function seasonChanged(): Promise<void> {
  if (!loaded) return;
  const { bake } = loaded;
  const season = Object.keys(SEASONS).indexOf(state.season);
  const { region } = loaded;
  terrain.setPalette(ramps(season, DECIDUOUS[region.leaf], region.look), SEASON_NAMES[season] === 'winter' ? -1e4 : region.look.snowline);
  terrain.nearField.setSeason(SEASON_NAMES[season]);
  lightChanged();
  if (!quarters.has(season)) quarters.set(season, loadQuarter(bake, season + 1));
  const colour = await quarters.get(season)!;
  if (loaded?.bake !== bake || Object.keys(SEASONS).indexOf(state.season) !== season) return;
  terrain.setColour(colour.data, bake.meta.s2_levels, bake.meta.barren_median?.[season]);
  readout.colour = `${(colour.bytes / 1e6).toFixed(1)} MB for this season`;
  requestRender();
}

function surfaceChanged(): void {
  if (!loaded) return;
  const lift = active(loaded.ground).lift + state.rise;
  terrain.setSurface(glo30Only(), lift, loaded.bake.meta.eye_m + lift);
  readout.eye = `${(loaded.bake.meta.eye_m + lift).toFixed(1)} m`;
  layout();
}

function lightChanged(refreshShadows = true): void {
  if (!loaded) return;
  const { lat, lon, tz } = loaded.bake.meta;
  const regional = loaded.region.look;
  const weather = { ...WEATHERS[state.weather], ...regional.weather?.[state.weather as 'sunny'] };
  const light = lightAt(lat, lon, SEASONS[state.season], state.hour, tz ?? 'UTC');
  terrain.setLight(weatherLight(light, weather), refreshShadows);
  terrain.setWeather(weather, state.hour, loaded.bake.meta.eye_m + state.rise);
  const bearing = ((Math.atan2(light.sun.x, light.sun.y) * 180) / Math.PI + 360) % 360;
  readout.sun = `${clock(Math.round(state.hour * 60))} · altitude ${light.altitude.toFixed(1)}°, bearing ${bearing.toFixed(0)}°`;
  requestRender();
}

/** The frame takes the photo's aspect while its camera is in use. */
function layout(): void {
  let width = stage.clientWidth;
  let height = stage.clientHeight;
  if (photoLocked()) {
    const aspect = photo.naturalWidth / photo.naturalHeight;
    if (width / height > aspect) width = height * aspect;
    else height = width / aspect;
  }
  frame.style.width = `${width}px`;
  frame.style.height = `${height}px`;
  renderer.setSize(width, height, false);
  camera.aspect = width / height;
  requestRender();
}

/** The photograph's vertical field: aligned, or from a 35 mm equivalent lens on the long side. */
function photoVfov(shot: Photo): number {
  if (shot.vfov_deg) return shot.vfov_deg;
  const focal = (shot.lens_mm! / 36) * Math.max(photo.naturalWidth, photo.naturalHeight);
  return (2 * Math.atan(photo.naturalHeight / 2 / focal) * 180) / Math.PI;
}

/**
 * The eye over (x, y), in the frame the terrain is drawn in: the spot's eye at the origin, and the
 * drawn ground lowered by the rise. Off the spot, the eye stands as high over the ground there.
 */
function eyeAt(x: number, y: number): THREE.Vector3 {
  if (!loaded || (x === 0 && y === 0)) return new THREE.Vector3();
  const ground = active(loaded.ground);
  const at = ground.at(x, y);
  if (Number.isNaN(at)) return new THREE.Vector3(x, y, 0);
  return new THREE.Vector3(x, y, at + loaded.bake.meta.eye_above_ground_m - CURVATURE * (x * x + y * y));
}

function aim(): void {
  const shot = loaded?.bake.meta.photo;
  let roll = 0;
  if (photoLocked() && shot) {
    state.yaw = shot.heading_deg;
    state.pitch = shot.pitch_deg;
    state.vfov = photoVfov(shot);
    roll = shot.roll_deg ?? 0;
  }
  camera.fov = state.vfov;
  const yaw = (state.yaw * Math.PI) / 180;
  const pitch = (state.pitch * Math.PI) / 180;
  camera.position.copy(eyeAt(state.x, state.y));
  camera.lookAt(camera.position.clone().add(new THREE.Vector3(Math.sin(yaw) * Math.cos(pitch), Math.cos(yaw) * Math.cos(pitch), Math.sin(pitch))));
  camera.rotateZ((roll * Math.PI) / 180);
}

let pending = false;
function requestRender(): void {
  if (pending || animating) return;
  pending = true;
  requestAnimationFrame(render);
}

function render(): void {
  pending = false;
  if (!loaded) return;
  aim();
  terrain.shade(renderer, camera);
  renderer.clear();
  for (const [scene, pass] of [
    [terrain.far, FAR_PASS],
    [terrain.near, NEAR_PASS],
  ] as const) {
    camera.near = pass.near;
    camera.far = pass.far;
    camera.updateProjectionMatrix();
    if (scene === terrain.far) readout.triangles = `${(terrain.prepare(camera) / 1e6).toFixed(2)} M per frame, ${(terrain.nearField.triangles / 1e6).toFixed(2)} M near`;
    else renderer.clearDepth();
    renderer.render(scene, camera);
  }
  if (WEATHERS[state.weather].rain) renderer.render(terrain.overlay, camera);
  photo.hidden = !state.reference || !photo.naturalWidth;
  drawCompass(compass, camera, state.pitch);
  writeHash();
}

/** The view in the address, so a reload or a link shows it again: spot, season, hour, weather, camera, debug view. */
function writeHash(): void {
  const camera: Record<string, string> = photoLocked() ? { photo: '1' } : { yaw: state.yaw.toFixed(1), pitch: state.pitch.toFixed(1), vfov: state.vfov.toFixed(1) };
  const debug: Record<string, string> = state.debug ? { debug: String(state.debug) } : {};
  const hash = new URLSearchParams({ spot: state.preset, season: state.season.split(' ')[0], hour: state.hour.toFixed(2), weather: state.weather, ...camera, ...debug }).toString();
  if (location.hash.slice(1) !== hash) history.replaceState(null, '', `#${hash}`);
}

/** The played day runs from DAY[0] to DAY[1] o'clock in DAY_S seconds, and starts over. */
const DAY = [4.5, 22.5];
const DAY_S = 36;
let playing = false;
let animating = false;
let lastTick = 0;
let ticks = 0;


/** Rain falls and the day plays frame by frame; everything else renders on demand. */
function animate(now: number): void {
  if (!playing && !WEATHERS[state.weather].rain && !(state.walk && held.size)) {
    animating = false;
    return;
  }
  const dt = Math.min((now - lastTick) / 1000, 0.1);
  lastTick = now;
  terrain.setTime(now / 1000);
  walk(dt);
  if (playing) {
    state.hour += (dt * (DAY[1] - DAY[0])) / DAY_S;
    if (state.hour > DAY[1]) state.hour = DAY[0];
    // The cast shadows follow the sun every third frame: they cost more than a frame.
    lightChanged(++ticks % 3 === 0);
  }
  render();
  requestAnimationFrame(animate);
}

function startAnimation(): void {
  if (animating) return;
  animating = true;
  lastTick = performance.now();
  requestAnimationFrame(animate);
}

/** The photograph beside the render from its camera, at the photograph's size, written by the dev server. */
async function saveSideBySide(): Promise<string | null> {
  if (!loaded?.bake.meta.photo || !photo.naturalWidth) return null;
  const { naturalWidth: width, naturalHeight: height } = photo;
  state.photoCamera = true;
  renderer.setPixelRatio(1);
  renderer.setSize(width, height, false);
  camera.aspect = width / height;
  render();
  const out = document.createElement('canvas');
  out.width = 2 * width;
  out.height = height;
  const ctx = out.getContext('2d')!;
  ctx.drawImage(photo, 0, 0, width, height);
  ctx.drawImage(renderer.domElement, width, 0);
  renderer.setPixelRatio(Math.min(devicePixelRatio, MAX_PIXEL_RATIO));
  layout();
  return save(out, `-${clock(Math.round(state.hour * 60)).replace(':', '')}`);
}

/** The current view, at the canvas's own resolution. */
async function saveView(): Promise<string | null> {
  render();
  const out = document.createElement('canvas');
  out.width = renderer.domElement.width;
  out.height = renderer.domElement.height;
  out.getContext('2d')!.drawImage(renderer.domElement, 0, 0);
  return save(out, `-${Math.round(state.yaw)}-${clock(Math.round(state.hour * 60)).replace(':', '')}`);
}

/** Written by the dev server into the bakes' shots/; the file name, or null when that failed. */
async function save(canvas: HTMLCanvasElement, suffix: string): Promise<string | null> {
  const blob = await new Promise<Blob>((resolve) => canvas.toBlob((b) => resolve(b!), 'image/png'));
  const { id, dtm_levels, half0_m } = loaded!.bake.meta;
  const reach = glo30Only() ? 'glo30' : `dtm${(half0_m * 2 ** (dtm_levels - 1)) / 1000}km`;
  const weather = state.weather === 'sunny' ? '' : `-${state.weather}`;
  const name = `${id}-${reach}-q${Object.keys(SEASONS).indexOf(state.season) + 1}${weather}${suffix}.png`;
  const response = await fetch(`/__shot/${name}`, { method: 'POST', body: blob });
  readout.frame = response.ok ? `saved shots/${name}` : `saving failed: ${response.status}`;
  return response.ok ? name : null;
}

function unlock(): void {
  if (!state.photoCamera) return;
  state.photoCamera = false;
  layout();
}

let drag: { x: number; y: number } | null = null;
frame.addEventListener('pointerdown', (event) => {
  drag = { x: event.clientX, y: event.clientY };
  frame.setPointerCapture(event.pointerId);
});
frame.addEventListener('pointermove', (event) => {
  if (!drag) return;
  unlock();
  const degreesPerPixel = state.vfov / frame.clientHeight;
  state.yaw = (state.yaw - (event.clientX - drag.x) * degreesPerPixel + 360) % 360;
  state.pitch = Math.min(89, Math.max(-89, state.pitch + (event.clientY - drag.y) * degreesPerPixel));
  drag = { x: event.clientX, y: event.clientY };
  requestRender();
});
frame.addEventListener('pointerup', () => (drag = null));
// A click enlarges the reference photograph to the frame, and another puts it back in its corner.
photo.addEventListener('pointerdown', (event) => event.stopPropagation());
photo.addEventListener('click', () => photo.classList.toggle('large'));
frame.addEventListener(
  'wheel',
  (event) => {
    event.preventDefault();
    unlock();
    state.vfov = Math.min(120, Math.max(3, state.vfov * Math.exp(event.deltaY * 0.001)));
    requestRender();
  },
  { passive: false },
);
addEventListener('resize', layout);

/**
 * Free movement: W A S D (or the arrows) walk over the ground, Q and E lower and raise the eye,
 * Shift runs. Keys held move the eye every frame.
 */
const held = new Set<string>();
const WALK_M_S = 6;
const RUN = 12;
addEventListener('keydown', (event) => {
  if (!state.walk || (event.target as HTMLElement).closest?.('.lil-gui')) return;
  const key = event.key.toLowerCase();
  if (!['w', 'a', 's', 'd', 'q', 'e', 'arrowup', 'arrowdown', 'arrowleft', 'arrowright', 'shift'].includes(key)) return;
  event.preventDefault();
  held.add(key);
  unlock();
  startAnimation();
});
addEventListener('keyup', (event) => held.delete(event.key.toLowerCase()));
addEventListener('blur', () => held.clear());

/** One frame's walk; returns whether the eye moved. */
function walk(dt: number): boolean {
  if (!state.walk || !held.size) return false;
  const speed = WALK_M_S * (held.has('shift') ? RUN : 1) * dt;
  const ahead = (held.has('w') || held.has('arrowup') ? 1 : 0) - (held.has('s') || held.has('arrowdown') ? 1 : 0);
  const right = (held.has('d') || held.has('arrowright') ? 1 : 0) - (held.has('a') || held.has('arrowleft') ? 1 : 0);
  const up = (held.has('e') ? 1 : 0) - (held.has('q') ? 1 : 0);
  const yaw = (state.yaw * Math.PI) / 180;
  state.x += (Math.sin(yaw) * ahead + Math.cos(yaw) * right) * speed;
  state.y += (Math.cos(yaw) * ahead - Math.sin(yaw) * right) * speed;
  if (up) {
    state.rise = Math.max(0, state.rise + up * speed);
    surfaceChanged();
  }
  return ahead !== 0 || right !== 0 || up !== 0;
}

/** Back-to-back frames of the current view, as a steady-state frame time. */
function measure(): void {
  const frames = 60;
  let count = 0;
  let start = 0;
  const tick = (now: number) => {
    if (count === 1) start = now;
    if (count > frames) {
      const ms = (now - start) / frames;
      readout.frame = `${ms.toFixed(1)} ms (${(1000 / ms).toFixed(0)} fps)`;
      return;
    }
    count++;
    render();
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

const gui = new GUI({ title: 'Trail View', width: 340 });
const shown = (error: Error) => (status.textContent = error.message);
const spotsOf = (region: string) => Object.fromEntries(REGION_SPOTS[region].map((spot) => [spot.name, spot.id]));
const regions = Object.fromEntries(Object.keys(REGION_SPOTS).map((short) => [`${short} · ${STATUS[short] ?? 'not started'}`, short]));
const regionPicker = gui.add(state, 'region', regions).onChange((short: string) => {
  const first = REGION_SPOTS[short][0];
  if (first) return select(first.id).catch(shown);
  selecting = '';
  loaded = null;
  state.preset = '';
  spotPicker.options({});
  frame.style.visibility = 'hidden';
  status.textContent = `${short} has no spots yet`;
});
const spotPicker = gui.add(state, 'preset', spotsOf(state.region)).name('spot').onChange((id: string) => select(id).catch(shown));
gui.add(state, 'reference').name('reference photo').onChange(requestRender);
gui.add(state, 'season', Object.keys(SEASONS)).onChange(seasonChanged);
gui.add(state, 'hour', 0, 24, 0.05).name('time of day').listen().onChange(() => lightChanged());
gui.add(state, 'weather', Object.keys(WEATHERS)).onChange(() => {
  lightChanged();
  startAnimation();
});
const play = gui.add({ play: () => {
  playing = !playing;
  play.name(playing ? 'pause' : 'play day');
  if (playing) startAnimation();
  else lightChanged();
} }, 'play').name('play day');
gui.add(state, 'walk').name('free movement (WASD, Q/E, shift)').listen().onChange((on: boolean) => {
  if (on) unlock();
  requestRender();
});
gui.add({ home: () => {
  Object.assign(state, { x: 0, y: 0, rise: 0 });
  surfaceChanged();
} }, 'home').name('back to the spot');
gui.add(state, 'trees').name('trees and plants').onChange((on: boolean) => {
  terrain.nearField.trees.visible = on;
  requestRender();
});
gui.add(state, 'buildings').onChange((on: boolean) => {
  terrain.nearField.buildings.visible = on;
  requestRender();
});
const shot = gui.addFolder('Photo').close();
shot.add(state, 'surface', [DTM, GLO30]).onChange(surfaceChanged);
shot.add(state, 'photoCamera').name('photo camera').listen().onChange(layout);
// Align a photograph's camera here, then save it back to its spot in spots/.
const align = { heading_deg: 0, pitch_deg: 0, vfov_deg: 40, roll_deg: 0 };
const aligned = shot.addFolder('align camera').close();
aligned.add(align, 'heading_deg', 0, 360, 0.05).name('heading').listen();
aligned.add(align, 'pitch_deg', -30, 30, 0.05).name('pitch').listen();
aligned.add(align, 'vfov_deg', 5, 90, 0.05).name('vertical field').listen();
aligned.add(align, 'roll_deg', -10, 10, 0.05).name('roll').listen();
aligned.onChange(() => {
  const photographed = loaded?.bake.meta.photo;
  if (!photographed) return;
  Object.assign(photographed, align);
  requestRender();
});
aligned.add({ copy: () => navigator.clipboard.writeText(JSON.stringify(align)) }, 'copy').name('copy camera JSON');
const style = gui.addFolder('Look').close();
style.add(look, 'snap', 0, 1, 0.01).name('palette snap').listen();
style.add(look, 'hazeKm', 5, 300, 1).name('haze at sea level (km)').listen();
style.add(look, 'hazeHeight', 500, 8000, 100).name('haze scale height (m)').listen();
style.add(look, 'mistTop', 0, 3000, 10).name('mist top (m)').listen();
style.add(look, 'softness', 0.005, 0.3, 0.005).name('band softness').listen();
style.add(look, 'gradient', 0, 1, 0.01).name('smooth light kept').listen();
style.add(state, 'debug', DEBUG_VIEWS).name('debug view').onChange((view: number) => terrain.setDebug(view));
style.onChange(() => {
  terrain.setLook(look);
  requestRender();
});
// The six keys paste into a region's look as they are.
style.add({ copy: () => navigator.clipboard.writeText(JSON.stringify(look)) }, 'copy').name('copy look JSON');
const facts = gui.addFolder('Readout').close();
facts.add(readout, 'region').listen().disable();
facts.add(readout, 'sun').listen().disable();
facts.add(readout, 'dtm').name('DTM').listen().disable();
facts.add(readout, 'eye').listen().disable();
facts.add(readout, 'bytes').name('download').listen().disable();
facts.add(readout, 'colour').name('colour').listen().disable();
facts.add(readout, 'near').name('near field').listen().disable();
facts.add(readout, 'triangles').name('triangles').listen().disable();
facts.add(readout, 'frame').name('frame time').listen().disable();
const tools = gui.addFolder('Tools').close();
tools.add(state, 'rise', 0, 400, 5).name('raise eye (m)').onChange(surfaceChanged);
tools.add({ measure }, 'measure').name('measure 60 frames');
tools.add({ saveSideBySide }, 'saveSideBySide').name('save side by side');
tools.add({ saveView }, 'saveView').name('save view');
// For scripted shots (tune/shoot.mjs) and checks from the console.
Object.assign(window, {
  trailView: { gui, state, look, align, render, layout, terrain, renderer, loaded: () => loaded, select, seasonChanged, lightChanged, saveView, saveSideBySide, seasons: Object.keys(SEASONS) },
});

const linked = new URLSearchParams(location.hash.slice(1));
if (SPOTS.some((spot) => spot.id === linked.get('spot'))) state.preset = linked.get('spot')!;
state.season = Object.keys(SEASONS).find((s) => s.startsWith(`${linked.get('season')} `)) ?? state.season;
state.hour = Number(linked.get('hour') ?? state.hour);
if (linked.get('weather')! in WEATHERS) state.weather = linked.get('weather')!;
state.debug = Number(linked.get('debug')) || 0;
gui.controllersRecursive().forEach((c) => c.updateDisplay());
terrain.setLook(look);
terrain.setDebug(state.debug);
select(state.preset)
  .then(() => {
    if (!linked.has('yaw')) return;
    state.photoCamera = false;
    Object.assign(state, { yaw: Number(linked.get('yaw')), pitch: Number(linked.get('pitch') ?? 0), vfov: Number(linked.get('vfov') ?? state.vfov) });
    layout();
  })
  .catch(shown);
