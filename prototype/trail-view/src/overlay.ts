/** What is drawn over the terrain: skyline polylines, peak labels and the compass strip. */

import * as THREE from 'three';
import type { Sighted } from './skyline';

const point = new THREE.Vector3();
const forward = new THREE.Vector3();

/** Screen pixels of a direction, or null behind the camera. */
export function project(camera: THREE.PerspectiveCamera, bearing: number, elevation: number, width: number, height: number) {
  const b = (bearing * Math.PI) / 180;
  const e = (elevation * Math.PI) / 180;
  point.set(Math.sin(b) * Math.cos(e), Math.cos(b) * Math.cos(e), Math.sin(e));
  if (point.dot(camera.getWorldDirection(forward)) <= 0.01) return null;
  point.project(camera);
  return [((point.x + 1) / 2) * width, ((1 - point.y) / 2) * height] as const;
}

export interface Line {
  points: ArrayLike<readonly [number, number]>;
  colour: string;
}

/** Polylines of [bearing, elevation], broken where the bearings jump by more than half a degree. */
export function drawLines(
  ctx: CanvasRenderingContext2D,
  camera: THREE.PerspectiveCamera,
  width: number,
  height: number,
  lines: Line[],
): void {
  ctx.lineWidth = 1.5 * Math.max(1, width / 1200);
  ctx.lineJoin = 'round';
  for (const line of lines) {
    ctx.strokeStyle = line.colour;
    ctx.beginPath();
    let previous: number | null = null;
    for (let i = 0; i < line.points.length; i++) {
      const [bearing, elevation] = line.points[i];
      const at = project(camera, bearing, elevation, width, height);
      if (!at) {
        previous = null;
        continue;
      }
      if (previous === null || Math.abs(bearing - previous) > 0.5) ctx.moveTo(at[0], at[1]);
      else ctx.lineTo(at[0], at[1]);
      previous = bearing;
    }
    ctx.stroke();
  }
}

/** Where peak labels go: best first, skipping any that would overlap one already placed. */
function labelSpots(camera: THREE.PerspectiveCamera, width: number, height: number, sighted: Sighted[], gap: number) {
  const spots: { x: number; y: number; s: Sighted }[] = [];
  for (const s of [...sighted].sort((a, b) => b.angle - a.angle)) {
    const at = project(camera, s.bearing, s.angle, width, height);
    if (!at || at[0] < 0 || at[0] > width || at[1] < 30 || at[1] > height) continue;
    if (spots.some((spot) => Math.abs(spot.x - at[0]) < gap)) continue;
    spots.push({ x: at[0], y: at[1], s });
  }
  return spots;
}

const kilometres = (metres: number) =>
  metres >= 10_000 ? `${Math.round(metres / 1000)} km` : `${(metres / 1000).toFixed(1)} km`;

export function placeLabels(layer: HTMLElement, camera: THREE.PerspectiveCamera, sighted: Sighted[]): void {
  layer.innerHTML = labelSpots(camera, layer.clientWidth, layer.clientHeight, sighted, 16)
    .map(({ x, y, s }) => {
      const ele = s.peak.ele === null ? '' : ` <small>${Math.round(s.peak.ele)}</small>`;
      return `<div class="peak" style="left:${x.toFixed(1)}px;top:${y.toFixed(1)}px" title="${kilometres(s.distance)}">` +
        `<span>${escape(s.peak.name)}${ele}</span></div>`;
    })
    .join('');
}

/** The same labels drawn into a saved image, with their distance. */
export function drawLabels(ctx: CanvasRenderingContext2D, camera: THREE.PerspectiveCamera, width: number, height: number, sighted: Sighted[]): void {
  const scale = Math.max(1, width / 1200);
  ctx.font = `${12 * scale}px system-ui, sans-serif`;
  ctx.lineWidth = scale;
  for (const { x, y, s } of labelSpots(camera, width, height, sighted, 16 * scale)) {
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.8)';
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x, y - 18 * scale);
    ctx.stroke();
    ctx.save();
    ctx.translate(x + 2 * scale, y - 20 * scale);
    ctx.rotate((-50 * Math.PI) / 180);
    const text = `${s.peak.name}${s.peak.ele === null ? '' : ` ${Math.round(s.peak.ele)}`} · ${kilometres(s.distance)}`;
    ctx.strokeStyle = 'rgba(0, 0, 0, 0.7)';
    ctx.lineWidth = 3 * scale;
    ctx.strokeText(text, 0, 0);
    ctx.fillStyle = '#fff';
    ctx.fillText(text, 0, 0);
    ctx.restore();
  }
}

/** Bearing ticks every 5°, names every 45°, and a mark for each sighted peak. */
export function drawCompass(strip: HTMLElement, camera: THREE.PerspectiveCamera, sighted: Sighted[], pitch: number): void {
  const width = strip.clientWidth;
  const height = camera.aspect > 0 ? width / camera.aspect : 0;
  const x = (bearing: number) => project(camera, bearing, pitch, width, height)?.[0];
  const names = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'];
  const parts: string[] = [];
  for (let bearing = 0; bearing < 360; bearing += 5) {
    const at = x(bearing);
    if (at === undefined || at < -20 || at > width + 20) continue;
    const name = bearing % 45 === 0 ? names[bearing / 45] : bearing % 15 === 0 ? `${bearing}` : '';
    parts.push(`<i class="${name ? 'major' : ''}" style="left:${at.toFixed(1)}px">${name}</i>`);
  }
  for (const s of sighted) {
    const at = x(s.bearing);
    if (at !== undefined && at >= 0 && at <= width) parts.push(`<b style="left:${at.toFixed(1)}px"></b>`);
  }
  strip.innerHTML = parts.join('');
}

const escape = (text: string) => text.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
