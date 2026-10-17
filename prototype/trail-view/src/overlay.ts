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

/** Peak labels, best first, skipping any that would overlap one already placed. */
export function placeLabels(layer: HTMLElement, camera: THREE.PerspectiveCamera, sighted: Sighted[]): void {
  const { clientWidth: width, clientHeight: height } = layer;
  const placed: number[] = [];
  const labels: string[] = [];
  for (const s of [...sighted].sort((a, b) => b.angle - a.angle)) {
    const at = project(camera, s.bearing, s.angle, width, height);
    if (!at || at[0] < 0 || at[0] > width || at[1] < 30 || at[1] > height) continue;
    if (placed.some((x) => Math.abs(x - at[0]) < 16)) continue;
    placed.push(at[0]);
    const ele = s.peak.ele === null ? '' : ` <small>${Math.round(s.peak.ele)}</small>`;
    const km = s.distance >= 10_000 ? `${Math.round(s.distance / 1000)} km` : `${(s.distance / 1000).toFixed(1)} km`;
    labels.push(
      `<div class="peak" style="left:${at[0].toFixed(1)}px;top:${at[1].toFixed(1)}px" title="${km}">` +
        `<span>${escape(s.peak.name)}${ele}</span></div>`,
    );
  }
  layer.innerHTML = labels.join('');
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
