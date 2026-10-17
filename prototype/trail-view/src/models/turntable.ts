/**
 * The model sheet behind models.html: each model of a pack, at each level of detail, on its own
 * turntable in the toon light of the marmot scene. One canvas behind the page draws every view
 * box in turn.
 */

import * as THREE from 'three';
import { BUILDING_KINDS, building, buildingLabel, type BuildingKind } from './buildings';
import { PACK_NAMES, packNamed } from '../regions';
import { plant, plantLabel } from './plants';
import { hex, paint, soft, triangles } from './shape';
import { LOD_RANGE_M, SEASONS, tree, treeLabel, type Lod, type Season } from './trees';

/** Width, depth and wall height in metres for each kind on the sheet. */
const FOOTPRINTS: Record<BuildingKind, [number, number, number?]> = {
  cottage: [9, 5.5],
  house: [10, 8],
  farmhouse: [18, 13],
  barn: [12, 8],
  block: [22, 12, 12],
};

const MEADOW: Record<Season, string> = { winter: '#b4b08e', spring: '#a8c46a', summer: '#a6c66d', autumn: '#b8b272' };

interface Cell {
  view: HTMLElement;
  model: THREE.Group;
  /** The ground's height and radius in the turntable's frame. */
  base: number;
  disc: number;
}

const canvas = document.getElementById('gl') as HTMLCanvasElement;
const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
renderer.shadowMap.enabled = true;

// Three light bands give the flat, painted look; the hemisphere light keeps the dark band warm.
const bands = new THREE.DataTexture(new Uint8Array([80, 175, 255]), 3, 1, THREE.RedFormat);
bands.minFilter = bands.magFilter = THREE.NearestFilter;
bands.needsUpdate = true;
const toon = new THREE.MeshToonMaterial({ vertexColors: true, gradientMap: bands, side: THREE.DoubleSide });
// Thin plants keep their up-leaning normals on both faces, as the terrain's object shader does.
toon.onBeforeCompile = (shader) => {
  const begin = THREE.ShaderChunk.normal_fragment_begin.replace('normal *= faceDirection;', '');
  shader.fragmentShader = shader.fragmentShader.replace('#include <normal_fragment_begin>', begin);
};

const scene = new THREE.Scene();
const sky = new THREE.HemisphereLight('#d7ecff', '#9a7650', 1.3);
sky.position.set(0, 0, 1);
const sun = new THREE.DirectionalLight('#fff0d8', 2.2);
sun.position.set(4, -5, 7).setLength(8);
sun.castShadow = true;
sun.shadow.mapSize.set(1024, 1024);
Object.assign(sun.shadow.camera, { left: -1.6, right: 1.6, top: 1.6, bottom: -1.6, near: 2, far: 14 });
sun.shadow.radius = 4;
sun.shadow.normalBias = 0.02;
sun.shadow.intensity = 0.7;
const ground = new THREE.Mesh(new THREE.BufferGeometry(), toon);
ground.receiveShadow = true;
scene.add(sky, sun, ground);

const camera = new THREE.PerspectiveCamera(28, 1, 0.1, 50);
camera.up.set(0, 0, 1);
camera.position.set(0.55, -0.75, 0.37).setLength(1.08 / Math.sin(THREE.MathUtils.degToRad(14)));
camera.lookAt(0, 0, 0);

const sheet = document.getElementById('sheet')!;
let cells: Cell[] = [];

/** A view box with its label, holding one model scaled to fit a unit sphere. */
function cell(row: HTMLElement, geo: THREE.BufferGeometry, title: string, detail: string): void {
  const box = row.appendChild(document.createElement('div'));
  box.className = 'cell';
  const view = box.appendChild(document.createElement('div'));
  view.className = 'view';
  const label = box.appendChild(document.createElement('div'));
  label.className = 'label';
  label.append(Object.assign(document.createElement('b'), { textContent: title }));
  label.append(Object.assign(document.createElement('span'), { textContent: `${detail} · ${triangles(geo).toLocaleString('en')} triangles` }));

  geo.computeBoundingBox();
  const { min, max } = geo.boundingBox!;
  const centre = min.clone().add(max).multiplyScalar(0.5);
  const radius = min.distanceTo(max) / 2;
  const mesh = new THREE.Mesh(geo, toon);
  mesh.castShadow = mesh.receiveShadow = true;
  mesh.position.copy(centre).negate();
  const model = new THREE.Group().add(mesh);
  model.scale.setScalar(1 / radius);
  model.visible = false;
  scene.add(model);
  const reach = Math.hypot(max.x - min.x, max.y - min.y) / 2 / radius;
  cells.push({ view, model, base: (min.z - centre.z) / radius, disc: Math.min(1.1, Math.max(0.6, 1.3 * reach)) });
}

function heading(parent: HTMLElement, text: string, level: 'h2' | 'h3' = 'h2'): void {
  parent.append(Object.assign(document.createElement(level), { textContent: text }));
}

function block(parent: HTMLElement, className: string): HTMLElement {
  return parent.appendChild(Object.assign(document.createElement('div'), { className }));
}

function show(packName: string, season: Season): void {
  for (const { model } of cells) {
    scene.remove(model);
    (model.children[0] as THREE.Mesh).geometry.dispose();
  }
  cells = [];
  sheet.replaceChildren();
  ground.geometry.dispose();
  ground.geometry = paint(soft(new THREE.CircleGeometry(1, 48)), hex(MEADOW[season]));

  const kit = packNamed(packName);
  heading(sheet, 'Trees');
  const forest = block(sheet, 'species');
  for (const species of kit.trees) {
    const [low, high] = species.elevation;
    const lods = block(forest, 'lods');
    heading(lods, `${treeLabel(species.tree)} · ${species.leaf}, on ${species.cover} · ${low}–${high} m · weight ${species.weight}`, 'h3');
    for (const lod of [0, 1, 2] as Lod[]) cell(lods, tree(species.tree, season, lod), `LOD ${lod}`, `within ${LOD_RANGE_M[lod]} m`);
  }
  heading(sheet, 'Ground plants');
  const plants = block(sheet, 'row');
  for (const name of kit.ground) cell(plants, plant(name, season), plantLabel(name), 'one LOD');
  heading(sheet, `Buildings · ${kit.style.name}`);
  const town = block(sheet, 'row');
  BUILDING_KINDS.forEach((kind, i) => {
    const [width, depth, wall] = FOOTPRINTS[kind];
    cell(town, building(kind, kit.style, { width, depth, wall, seed: i + 1 }), buildingLabel(kind), `${width} × ${depth} m`);
  });

  for (const button of document.querySelectorAll<HTMLButtonElement>('.chips button')) {
    button.setAttribute('aria-pressed', String(button.value === packName || button.value === season));
  }
  history.replaceState(null, '', `#${packName}/${season}`);
}

const [hashKit, hashSeason] = location.hash.slice(1).split('/');
const state = {
  kit: PACK_NAMES.includes(hashKit) ? hashKit : 'alps',
  season: ((SEASONS as readonly string[]).includes(hashSeason) ? hashSeason : 'summer') as Season,
};

function chips(id: string, values: readonly string[], labels: (value: string) => string, pick: (value: string) => void): void {
  const group = document.getElementById(id)!;
  for (const value of values) {
    const button = group.appendChild(Object.assign(document.createElement('button'), { value, textContent: labels(value) }));
    button.addEventListener('click', () => {
      pick(value);
      show(state.kit, state.season);
    });
  }
}
chips('kits', PACK_NAMES, (k) => packNamed(k).name, (k) => (state.kit = k));
chips('seasons', SEASONS, (s) => s[0].toUpperCase() + s.slice(1), (s) => (state.season = s as Season));
show(state.kit, state.season);

const still = matchMedia('(prefers-reduced-motion: reduce)').matches;

function draw(time: number): void {
  const [width, height] = [innerWidth, innerHeight];
  const ratio = renderer.getPixelRatio();
  if (canvas.width !== Math.floor(width * ratio) || canvas.height !== Math.floor(height * ratio)) renderer.setSize(width, height, false);
  renderer.setScissorTest(false);
  renderer.clear();
  renderer.setScissorTest(true);
  for (const { view, model, base, disc } of cells) {
    const r = view.getBoundingClientRect();
    if (r.bottom < 0 || r.top > height || r.right < 0 || r.left > width) continue;
    renderer.setViewport(r.left, height - r.bottom, r.width, r.height);
    renderer.setScissor(r.left, height - r.bottom, r.width, r.height);
    camera.aspect = r.width / r.height;
    camera.updateProjectionMatrix();
    model.rotation.z = still ? 0.6 : 0.6 + time * 0.0004;
    model.visible = true;
    ground.position.z = base;
    ground.scale.setScalar(disc);
    renderer.render(scene, camera);
    model.visible = false;
  }
}
renderer.setAnimationLoop(draw);
