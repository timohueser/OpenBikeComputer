/** The compass strip over the terrain. */

import * as THREE from 'three';

const point = new THREE.Vector3();
const forward = new THREE.Vector3();

/** Screen pixels of a direction, or null behind the camera. */
function project(camera: THREE.PerspectiveCamera, bearing: number, elevation: number, width: number, height: number) {
  const b = (bearing * Math.PI) / 180;
  const e = (elevation * Math.PI) / 180;
  point.set(Math.sin(b) * Math.cos(e), Math.cos(b) * Math.cos(e), Math.sin(e));
  if (point.dot(camera.getWorldDirection(forward)) <= 0.01) return null;
  point.project(camera);
  return [((point.x + 1) / 2) * width, ((1 - point.y) / 2) * height] as const;
}

/** Bearing ticks every 5° and names every 45°. */
export function drawCompass(strip: HTMLElement, camera: THREE.PerspectiveCamera, pitch: number): void {
  const width = strip.clientWidth;
  const height = camera.aspect > 0 ? width / camera.aspect : 0;
  const names = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'];
  const parts: string[] = [];
  for (let bearing = 0; bearing < 360; bearing += 5) {
    const at = project(camera, bearing, pitch, width, height)?.[0];
    if (at === undefined || at < -20 || at > width + 20) continue;
    const name = bearing % 45 === 0 ? names[bearing / 45] : bearing % 15 === 0 ? `${bearing}` : '';
    parts.push(`<i class="${name ? 'major' : ''}" style="left:${at.toFixed(1)}px">${name}</i>`);
  }
  strip.innerHTML = parts.join('');
}
