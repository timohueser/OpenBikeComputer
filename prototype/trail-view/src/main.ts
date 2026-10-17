import GUI from 'lil-gui';
import * as THREE from 'three';
import presets from '../presets.json';
import { clock } from 'planner-sun';
import { loadBake, loadQuarter, type Bake, type Photo } from './bake';
import { lightAt, SEASONS } from './light';
import { drawCompass, drawLabels, drawLines, placeLabels, type Line } from './overlay';
import { KITS, type KitName } from './models/kits';
import { SEASONS as SEASON_NAMES } from './models/trees';
import { DECIDUOUS, ramps } from './palette';
import { BINS, rms, sightedPeaks, skyline, surface, type Sighted, type Skyline, type Surface } from './skyline';
import { FAR_PASS, NEAR_PASS, Terrain, type Look } from './terrain';

const element = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const stage = element('#stage');
const frame = element('#frame');
const photo = element<HTMLImageElement>('#photo');
const lines = element<HTMLCanvasElement>('#lines');
const labels = element('#labels');
const compass = element('#compass');
const status = element('#status');

const renderer = new THREE.WebGLRenderer({ canvas: element<HTMLCanvasElement>('#gl'), antialias: true });
renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
renderer.autoClear = false;
const camera = new THREE.PerspectiveCamera(60, 1, FAR_PASS.near, FAR_PASS.far);
camera.up.set(0, 0, 1);
const terrain = new Terrain();

const DTM = 'national DTM';
const GLO30 = 'GLO-30 only';
const state = {
  preset: presets[0].id,
  surface: DTM,
  photoCamera: true,
  wipe: 0.5,
  photoSkyline: true,
  castSkyline: true,
  peaks: true,
  trees: true,
  buildings: true,
  season: 'summer (15 Aug)',
  hour: 16,
  yaw: 0,
  pitch: 0,
  vfov: 60,
};
const look: Look = { snap: 0.4, hazeKm: 35, hazeHeight: 2500, mistTop: 0, softness: 0.08, gradient: 0.35 };
const readout = { sun: '–', colour: '–', near: '–', dtm: '–', eye: '–', rmsDtm: '–', rmsGlo30: '–', limit: '–', peaks: '–', bytes: '–', triangles: '–', frame: '–' };

interface Loaded {
  bake: Bake;
  ground: { dtm: Surface; glo30: Surface };
  sky: { dtm: Skyline; glo30: Skyline };
  sighted: Sighted[];
}
let loaded: Loaded | null = null;

const glo30Only = () => !loaded?.bake.meta.dtm_levels || state.surface === GLO30;
const active = <T>(pair: { dtm: T; glo30: T }) => (glo30Only() ? pair.glo30 : pair.dtm);
const photoLocked = () => Boolean(state.photoCamera && loaded?.bake.meta.photo && photo.naturalWidth);

const presetOf = (id: string) => presets.find((p) => p.id === id)!;

let selecting = '';
async function select(id: string): Promise<void> {
  selecting = id;
  status.textContent = `Loading ${id}…`;
  loaded = null;
  photo.removeAttribute('src');
  const bake = await loadBake(id);
  if (selecting !== id) return;
  const { meta } = bake;
  terrain.setBake(bake, KITS[presetOf(id).kit as KitName]);
  const area = meta.size * meta.size;
  const canopy = [...Array(meta.dtm_levels).keys()].map((k) => Float32Array.from(bake.canopy.subarray(k * area, (k + 1) * area), (v) => v / 4));
  const dtm = surface(bake.surface, meta.size, meta.half0_m, meta.eye_above_ground_m, canopy);
  const glo30 = meta.dtm_levels
    ? surface([...bake.glo30, ...bake.surface.slice(meta.dtm_levels)], meta.size, meta.half0_m, meta.eye_above_ground_m)
    : dtm;
  const dtmSky = skyline(dtm, meta.size);
  const sky = { dtm: dtmSky, glo30: glo30 === dtm ? dtmSky : skyline(glo30, meta.size) };
  loaded = { bake, ground: { dtm, glo30 }, sky, sighted: [] };
  const shot = meta.photo;
  const columns = shot?.columns;
  readout.rmsDtm = columns && meta.dtm_levels ? `${rms(sky.dtm, columns).toFixed(3)}°` : '–';
  readout.rmsGlo30 = columns ? `${rms(sky.glo30, columns).toFixed(3)}°` : '–';
  readout.limit = columns ? `${shot!.rms_limit_deg!.toFixed(2)}° (${columns.length} columns)` : '–';
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
  status.textContent = '';
  quarters.clear();
  surfaceChanged();
  seasonChanged();
}

/** Sentinel-2 quarters of the current preset, by season index, loaded once each. */
const quarters = new Map<number, ReturnType<typeof loadQuarter>>();

/** The season's palette and light at once, then its Sentinel-2 quarter when it arrives. */
async function seasonChanged(): Promise<void> {
  if (!loaded) return;
  const { bake } = loaded;
  const season = Object.keys(SEASONS).indexOf(state.season);
  terrain.setPalette(ramps(season, DECIDUOUS[presetOf(bake.meta.id).default_leaf]));
  terrain.nearField.setSeason(SEASON_NAMES[season]);
  lightChanged();
  if (!quarters.has(season)) quarters.set(season, loadQuarter(bake, season + 1));
  const colour = await quarters.get(season)!;
  if (loaded?.bake !== bake || Object.keys(SEASONS).indexOf(state.season) !== season) return;
  terrain.setColour(colour.data, bake.meta.s2_levels);
  readout.colour = `${(colour.bytes / 1e6).toFixed(1)} MB for this season`;
  requestRender();
}

function surfaceChanged(): void {
  if (!loaded) return;
  const lift = active(loaded.ground).lift;
  terrain.setSurface(glo30Only(), lift, loaded.bake.meta.eye_m + lift);
  readout.eye = `${(loaded.bake.meta.eye_m + lift).toFixed(1)} m`;
  loaded.sighted = sightedPeaks(loaded.bake.peaks, active(loaded.ground), active(loaded.sky));
  readout.peaks = `${loaded.sighted.length} of ${loaded.bake.peaks.length} named peaks sighted`;
  layout();
}

function lightChanged(): void {
  if (!loaded) return;
  const { lat, lon, id } = loaded.bake.meta;
  const timezone = presetOf(id).tz;
  const light = lightAt(lat, lon, SEASONS[state.season], state.hour, timezone);
  terrain.setLight(light);
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
  lines.width = Math.round(width * renderer.getPixelRatio());
  lines.height = Math.round(height * renderer.getPixelRatio());
  camera.aspect = width / height;
  requestRender();
}

/** The photograph's vertical field: aligned, or from a 35 mm equivalent lens on the long side. */
function photoVfov(shot: Photo): number {
  if (shot.vfov_deg) return shot.vfov_deg;
  const focal = (shot.lens_mm! / 36) * Math.max(photo.naturalWidth, photo.naturalHeight);
  return (2 * Math.atan(photo.naturalHeight / 2 / focal) * 180) / Math.PI;
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
  camera.lookAt(Math.sin(yaw) * Math.cos(pitch), Math.cos(yaw) * Math.cos(pitch), Math.sin(pitch));
  camera.rotateZ((roll * Math.PI) / 180);
}

let pending = false;
function requestRender(): void {
  if (pending) return;
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
    if (scene === terrain.far) readout.triangles = `${(terrain.prepare(camera) / 1e6).toFixed(2)} M per frame`;
    else renderer.clearDepth();
    renderer.render(scene, camera);
  }
  const locked = photoLocked();
  photo.style.display = locked ? '' : 'none';
  photo.style.clipPath = `inset(0 ${(100 * (1 - state.wipe)).toFixed(2)}% 0 0)`;
  const ctx = lines.getContext('2d')!;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, lines.width, lines.height);
  ctx.setTransform(renderer.getPixelRatio(), 0, 0, renderer.getPixelRatio(), 0, 0);
  drawLines(ctx, camera, lines.clientWidth, lines.clientHeight, skylines(state.castSkyline, state.photoSkyline));
  if (state.peaks) placeLabels(labels, camera, loaded.sighted);
  else labels.innerHTML = '';
  drawCompass(compass, camera, loaded.sighted, state.pitch);
}

function skylines(cast: boolean, photographed: boolean): Line[] {
  const shown: Line[] = [];
  if (!loaded) return shown;
  if (cast) {
    const angle = active(loaded.sky).angle;
    shown.push({ points: Array.from({ length: BINS + 1 }, (_, b) => [(b * 360) / BINS, angle[b % BINS]] as const), colour: '#22e0ff' });
  }
  const shot = loaded.bake.meta.photo;
  if (photographed && shot?.columns) shown.push({ points: shot.columns, colour: '#ff3b30' });
  return shown;
}

/** The photograph beside the render from its camera, at the photograph's size, written by the dev server. */
async function saveSideBySide(): Promise<void> {
  if (!loaded?.bake.meta.photo || !photo.naturalWidth) return;
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
  for (const [left, cast] of [
    [0, false],
    [width, true],
  ] as const) {
    ctx.save();
    ctx.translate(left, 0);
    ctx.beginPath();
    ctx.rect(0, 0, width, height);
    ctx.clip();
    drawLines(ctx, camera, width, height, skylines(cast, true));
    ctx.restore();
  }
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
  layout();
  await save(out, `-${clock(Math.round(state.hour * 60)).replace(':', '')}`);
}

/** The current view with its lines, at the canvas's own resolution. */
async function saveView(): Promise<void> {
  render();
  const out = document.createElement('canvas');
  out.width = lines.width;
  out.height = lines.height;
  const ctx = out.getContext('2d')!;
  ctx.drawImage(renderer.domElement, 0, 0);
  ctx.drawImage(lines, 0, 0);
  if (state.peaks && loaded) drawLabels(ctx, camera, out.width, out.height, loaded.sighted);
  await save(out, `-${Math.round(state.yaw)}-${clock(Math.round(state.hour * 60)).replace(':', '')}`);
}

async function save(canvas: HTMLCanvasElement, suffix: string): Promise<void> {
  const blob = await new Promise<Blob>((resolve) => canvas.toBlob((b) => resolve(b!), 'image/png'));
  const { id, dtm_levels, half0_m } = loaded!.bake.meta;
  const reach = glo30Only() ? 'glo30' : `dtm${(half0_m * 2 ** (dtm_levels - 1)) / 1000}km`;
  const name = `${id}-${reach}-q${Object.keys(SEASONS).indexOf(state.season) + 1}${suffix}.png`;
  const response = await fetch(`/__shot/${name}`, { method: 'POST', body: blob });
  readout.frame = response.ok ? `saved shots/${name}` : `saving failed: ${response.status}`;
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
const spots = Object.fromEntries(presets.map((p) => [`${KITS[p.kit as KitName].name} · ${p.name}`, p.id]));
gui.add(state, 'preset', spots).name('spot').onChange(select);
gui.add(state, 'season', Object.keys(SEASONS)).onChange(seasonChanged);
gui.add(state, 'hour', 0, 24, 0.05).name('time of day').onChange(lightChanged);
gui.add(state, 'trees').name('trees and plants').onChange((on: boolean) => {
  terrain.nearField.trees.visible = on;
  requestRender();
});
gui.add(state, 'buildings').onChange((on: boolean) => {
  terrain.nearField.buildings.visible = on;
  requestRender();
});
gui.add(state, 'peaks').onChange(requestRender);
const shot = gui.addFolder('Photo').close();
shot.add(state, 'surface', [DTM, GLO30]).onChange(surfaceChanged);
shot.add(state, 'photoCamera').name('photo camera').listen().onChange(layout);
shot.add(state, 'wipe', 0, 1, 0.01).onChange(requestRender);
shot.add(state, 'photoSkyline').name('photo skyline (red)').onChange(requestRender);
shot.add(state, 'castSkyline').name('cast skyline (cyan)').onChange(requestRender);
// Align a photograph's camera here, then save it back to presets.json.
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
style.add(look, 'snap', 0, 1, 0.01).name('palette snap');
style.add(look, 'hazeKm', 5, 300, 1).name('haze at sea level (km)');
style.add(look, 'hazeHeight', 500, 8000, 100).name('haze scale height (m)');
style.add(look, 'mistTop', 0, 3000, 10).name('mist top (m)');
style.add(look, 'softness', 0.005, 0.3, 0.005).name('band softness');
style.add(look, 'gradient', 0, 1, 0.01).name('smooth light kept');
style.onChange(() => {
  terrain.setLook(look);
  requestRender();
});
style.add({ copy: () => navigator.clipboard.writeText(JSON.stringify(look)) }, 'copy').name('copy look JSON');
const facts = gui.addFolder('Readout').close();
facts.add(readout, 'sun').listen().disable();
facts.add(readout, 'dtm').name('DTM').listen().disable();
facts.add(readout, 'eye').listen().disable();
facts.add(readout, 'rmsDtm').name('RMS, DTM').listen().disable();
facts.add(readout, 'rmsGlo30').name('RMS, GLO-30').listen().disable();
facts.add(readout, 'limit').name('Peak View limit').listen().disable();
facts.add(readout, 'peaks').name('peaks').listen().disable();
facts.add(readout, 'bytes').name('download').listen().disable();
facts.add(readout, 'colour').name('colour').listen().disable();
facts.add(readout, 'near').name('near field').listen().disable();
facts.add(readout, 'triangles').name('triangles').listen().disable();
facts.add(readout, 'frame').name('frame time').listen().disable();
const tools = gui.addFolder('Tools').close();
tools.add({ measure }, 'measure').name('measure 60 frames');
tools.add({ saveSideBySide }, 'saveSideBySide').name('save side by side');
tools.add({ saveView }, 'saveView').name('save view');
// For scripted checks from the console.
Object.assign(window, { trailView: { gui, state, look, align, render, layout, terrain, loaded: () => loaded } });

terrain.setLook(look);
select(state.preset).catch((error: Error) => (status.textContent = error.message));
