import GUI from 'lil-gui';
import * as THREE from 'three';
import presets from '../presets.json';
import { loadBake, type Bake } from './bake';
import { drawCompass, drawLines, placeLabels, type Line } from './overlay';
import { BINS, rms, sightedPeaks, skyline, surface, type Sighted, type Skyline, type Surface } from './skyline';
import { FAR_PASS, NEAR_PASS, Terrain, type Look } from './terrain';

// Colours are display values; nothing here is lit physically.
THREE.ColorManagement.enabled = false;

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
  yaw: 0,
  pitch: 0,
  vfov: 60,
};
const look: Look = { ground: '#76825f', horizon: '#c4d2df', zenith: '#6d98cc', shade: 0.85, hazeKm: 45 };
const readout = { dtm: '–', eye: '–', rmsDtm: '–', rmsGlo30: '–', limit: '–', peaks: '–', bytes: '–', triangles: '–', frame: '–' };

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

let selecting = '';
async function select(id: string): Promise<void> {
  selecting = id;
  status.textContent = `Loading ${id}…`;
  loaded = null;
  photo.removeAttribute('src');
  const bake = await loadBake(id);
  if (selecting !== id) return;
  const { meta } = bake;
  terrain.setBake(bake);
  const dtm = surface(bake.surface, meta.size, meta.half0_m);
  const glo30 = meta.dtm_levels ? surface([...bake.glo30, ...bake.surface.slice(meta.dtm_levels)], meta.size, meta.half0_m) : dtm;
  const dtmSky = skyline(dtm, meta.size);
  const sky = { dtm: dtmSky, glo30: glo30 === dtm ? dtmSky : skyline(glo30, meta.size) };
  loaded = { bake, ground: { dtm, glo30 }, sky, sighted: [] };
  if (meta.photo) {
    readout.rmsDtm = meta.dtm_levels ? `${rms(sky.dtm, meta.photo.columns).toFixed(3)}°` : '–';
    readout.rmsGlo30 = `${rms(sky.glo30, meta.photo.columns).toFixed(3)}°`;
    readout.limit = `${meta.photo.rms_limit_deg.toFixed(2)}° (${meta.photo.columns.length} columns)`;
    photo.src = `${bake.base}${meta.photo.file}`;
    // A newer selection clears the source, which rejects this decode.
    await photo.decode().catch(() => undefined);
    if (selecting !== id) return;
  } else {
    readout.rmsDtm = readout.rmsGlo30 = readout.limit = '–';
  }
  readout.bytes = `${(bake.bytes / 1e6).toFixed(1)} MB heights`;
  readout.dtm = meta.dtm_levels ? `${meta.dtm}, ±${(meta.half0_m * 2 ** (meta.dtm_levels - 1)) / 1000} km` : 'none';
  state.yaw = meta.photo?.heading_deg ?? meta.heading ?? 0;
  state.pitch = meta.photo?.pitch_deg ?? meta.pitch ?? 0;
  element('#credits').textContent = meta.credits.join(' · ');
  status.textContent = '';
  surfaceChanged();
}

function surfaceChanged(): void {
  if (!loaded) return;
  const lift = active(loaded.ground).lift;
  terrain.setSurface(glo30Only(), lift);
  readout.eye = `${(loaded.bake.meta.eye_m + lift).toFixed(1)} m`;
  loaded.sighted = sightedPeaks(loaded.bake.peaks, active(loaded.ground), active(loaded.sky));
  readout.peaks = `${loaded.sighted.length} of ${loaded.bake.peaks.length} named peaks sighted`;
  layout();
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

function aim(): void {
  const shot = loaded?.bake.meta.photo;
  if (photoLocked() && shot) {
    // A 35 mm equivalent lens, measured on the long side as the photographs' fit did.
    const focal = (shot.lens_mm / 36) * Math.max(photo.naturalWidth, photo.naturalHeight);
    state.yaw = shot.heading_deg;
    state.pitch = shot.pitch_deg;
    state.vfov = (2 * Math.atan(photo.naturalHeight / 2 / focal) * 180) / Math.PI;
  }
  camera.fov = state.vfov;
  const yaw = (state.yaw * Math.PI) / 180;
  const pitch = (state.pitch * Math.PI) / 180;
  camera.lookAt(Math.sin(yaw) * Math.cos(pitch), Math.cos(yaw) * Math.cos(pitch), Math.sin(pitch));
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
  if (photographed && shot) shown.push({ points: shot.columns, colour: '#ff3b30' });
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
  await save(out, '');
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
  await save(out, `-${Math.round(state.yaw)}`);
}

async function save(canvas: HTMLCanvasElement, suffix: string): Promise<void> {
  const blob = await new Promise<Blob>((resolve) => canvas.toBlob((b) => resolve(b!), 'image/png'));
  const { id, dtm_levels, half0_m } = loaded!.bake.meta;
  const reach = glo30Only() ? 'glo30' : `dtm${(half0_m * 2 ** (dtm_levels - 1)) / 1000}km`;
  const name = `${id}-${reach}${suffix}.png`;
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

const gui = new GUI({ title: 'Trail View' });
gui.add(state, 'preset', presets.map((p) => p.id)).onChange(select);
gui.add(state, 'surface', [DTM, GLO30]).onChange(surfaceChanged);
const shot = gui.addFolder('Photo');
shot.add(state, 'photoCamera').name('photo camera').listen().onChange(layout);
shot.add(state, 'wipe', 0, 1, 0.01).onChange(requestRender);
shot.add(state, 'photoSkyline').name('photo skyline (red)').onChange(requestRender);
shot.add(state, 'castSkyline').name('cast skyline (cyan)').onChange(requestRender);
gui.add(state, 'peaks').onChange(requestRender);
const style = gui.addFolder('Look').close();
style.addColor(look, 'ground');
style.addColor(look, 'horizon');
style.addColor(look, 'zenith');
style.add(look, 'shade', 0, 1, 0.01).name('relief shading');
style.add(look, 'hazeKm', 5, 300, 1).name('haze (km)');
style.onChange(() => {
  terrain.setLook(look);
  requestRender();
});
const facts = gui.addFolder('Readout');
facts.add(readout, 'dtm').name('DTM').listen().disable();
facts.add(readout, 'eye').listen().disable();
facts.add(readout, 'rmsDtm').name('RMS, DTM').listen().disable();
facts.add(readout, 'rmsGlo30').name('RMS, GLO-30').listen().disable();
facts.add(readout, 'limit').name('Peak View limit').listen().disable();
facts.add(readout, 'peaks').name('peaks').listen().disable();
facts.add(readout, 'bytes').name('download').listen().disable();
facts.add(readout, 'triangles').name('triangles').listen().disable();
facts.add(readout, 'frame').name('frame time').listen().disable();
facts.add({ measure }, 'measure').name('measure 60 frames');
gui.add({ saveSideBySide }, 'saveSideBySide').name('save side by side');
gui.add({ saveView }, 'saveView').name('save view');
// For scripted checks from the console.
Object.assign(window, { trailView: { gui, state, look, render, layout } });

terrain.setLook(look);
select(state.preset).catch((error: Error) => (status.textContent = error.message));
