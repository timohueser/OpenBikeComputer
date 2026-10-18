/**
 * The model sheet behind models.html: every model with the regions that draw it, the census of
 * species that only a stand-in draws, or each model of one pack at each level of detail. Each model
 * turns on its own turntable in the toon light of the marmot scene. One canvas behind the page
 * draws every view box in turn. Under reduced motion each view holds a still picture instead, so a
 * full-page screenshot shows every model.
 */

import * as THREE from 'three';
import { BUILDING_KINDS, building, buildingLabel, type BuildingKind } from './buildings';
import { regionsOf, standIns, USES } from './census';
import { PACK_NAMES, packNamed } from '../regions';
import { animal, ANIMALS, plant, PLANTS, tree, TREES } from './library';
import { hex, paint, soft, triangles } from './shape';
import { LOD_RANGE_M, SEASONS, type Lod, type Season } from './trees';

/** Width, depth and wall height in metres for each kind on the sheet. */
const FOOTPRINTS: Record<BuildingKind, [number, number, number?]> = {
  cottage: [9, 5.5],
  house: [10, 8],
  farmhouse: [18, 13],
  barn: [12, 8],
  block: [22, 12, 12],
};

const MEADOW: Record<Season, string> = { winter: '#b4b08e', spring: '#a8c46a', summer: '#a6c66d', autumn: '#b8b272' };

/** The sheets that are not a pack's, by their chip labels. Their chips come first. */
const SHEETS: Record<string, string> = { all: 'All models', missing: 'Missing' };

/** Each kind of model, with its nearest level of detail in a season. */
const KINDS: { title: string; models: Record<string, { name: string; latin: string }>; build: (id: string, season: Season) => THREE.BufferGeometry }[] = [
  { title: 'Trees', models: TREES, build: (id, season) => tree(id, season, 0) },
  { title: 'Plants', models: PLANTS, build: (id, season) => plant(id, season) },
  { title: 'Animals', models: ANIMALS, build: (id) => animal(id) },
];
const kindOf = (id: string) => KINDS.find(({ models }) => id in models)!;

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

/** A view box in `parent`, holding one model scaled to fit a unit sphere. */
function turntable(parent: HTMLElement, geo: THREE.BufferGeometry): void {
  const view = block(parent, 'view');
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

/** A view box over its label: the title, then the detail with the model's triangles. */
function cell(row: HTMLElement, geo: THREE.BufferGeometry, title: string, detail: string): HTMLElement {
  const box = block(row, 'cell');
  turntable(box, geo);
  const label = block(box, 'label');
  label.append(el('b', title), el('span', `${detail} · ${triangles(geo).toLocaleString('en')} triangles`));
  return label;
}

const el = (tag: string, text: string) => Object.assign(document.createElement(tag), { textContent: text });

/** A line of a label: its lead, then the regions. */
function drawnIn(label: HTMLElement, lead: HTMLElement, regions: string[]): void {
  label.appendChild(document.createElement('p')).append(lead, ` ${regions.join(', ') || 'no region'}`);
}

function heading(parent: HTMLElement, text: string, level: 'h2' | 'h3' = 'h2'): void {
  parent.append(el(level, text));
}

function block(parent: HTMLElement, className: string): HTMLElement {
  return parent.appendChild(Object.assign(document.createElement('div'), { className }));
}

/** Every model by kind: the regions that draw it as itself, and the species it stands in for. */
function allModels(season: Season): void {
  for (const { title, models, build } of KINDS) {
    heading(sheet, `${title} · ${Object.keys(models).length} models`);
    const row = block(sheet, 'row start');
    for (const [id, { name, latin }] of Object.entries(models)) {
      const uses = USES.filter((use) => use.model === id);
      const label = cell(row, build(id, season), name, latin);
      drawnIn(label, el('strong', 'Drawn in'), regionsOf(uses.filter((use) => !use.latin)));
      const stands = standIns(uses);
      if (stands.length) label.appendChild(document.createElement('p')).append(el('strong', 'Stands in for'));
      for (const [species, of] of stands) drawnIn(label, el('i', species), regionsOf(of));
    }
  }
}

/** One card per species that only a stand-in draws: its aliases, the stand-in models and the regions. */
function missing(season: Season): void {
  const census = standIns(USES);
  for (const kind of KINDS) {
    const species = census.filter(([, uses]) => kindOf(uses[0].model) === kind);
    if (!species.length) continue;
    heading(sheet, `${kind.title} · ${species.length} species`);
    const row = block(sheet, 'row start');
    for (const [latin, uses] of species) {
      const box = block(row, 'cell');
      const views = block(box, 'views');
      for (const model of new Set(uses.map((use) => use.model))) turntable(views, kindOf(model).build(model, season));
      const label = block(box, 'label');
      label.appendChild(document.createElement('b')).append(el('i', latin));
      for (const alias of new Set(uses.map((use) => `${use.id} → ${kindOf(use.model).models[use.model].name}`))) label.append(el('span', alias));
      drawnIn(label, el('strong', 'Drawn in'), regionsOf(uses));
    }
  }
}

/** Each model of a pack at each level of detail, and its buildings. */
function packSheet(packName: string, season: Season): void {
  const kit = packNamed(packName);
  heading(sheet, 'Trees');
  const forest = block(sheet, 'species');
  for (const species of kit.trees) {
    const [low, high] = species.elevation;
    const lods = block(forest, 'lods');
    const name = species.tree;
    const isTree = name in TREES;
    heading(lods, `${(isTree ? TREES : PLANTS)[name].name} (${species.id}) · ${species.leaf}, on ${species.cover} · ${low}–${high} m · weight ${species.weight}`, 'h3');
    if (!isTree) cell(lods, plant(name, season), 'shrub', 'one LOD');
    else for (const lod of [0, 1, 2] as Lod[]) cell(lods, tree(name, season, lod), `LOD ${lod}`, `within ${LOD_RANGE_M[lod]} m`);
  }
  heading(sheet, 'Ground plants');
  const plants = block(sheet, 'row');
  for (const { id, plant: name } of kit.ground) cell(plants, plant(name, season), `${PLANTS[name].name} (${id})`, 'one LOD');
  if (kit.herds.length) {
    heading(sheet, 'Herds');
    const herds = block(sheet, 'species');
    for (const { id, animal: name, groups, size, on } of kit.herds) {
      const coats = block(herds, 'lods');
      heading(coats, `${ANIMALS[name].name} (${id}) · ${groups} groups of ${size[0]}–${size[1]} · on ${on.join(', ')}`, 'h3');
      for (const seed of [0, 1, 2]) cell(coats, animal(name, seed), `Seed ${seed}`, `${ANIMALS[name].height_m} m tall`);
    }
  }
  heading(sheet, `Buildings · ${kit.style.name}`);
  const town = block(sheet, 'row');
  BUILDING_KINDS.forEach((kind, i) => {
    const [width, depth, wall] = FOOTPRINTS[kind];
    cell(town, building(kind, kit.style, { width, depth, wall, seed: i + 1 }), buildingLabel(kind), `${width} × ${depth} m`);
  });
}

function show(name: string, season: Season): void {
  for (const { model } of cells) {
    scene.remove(model);
    (model.children[0] as THREE.Mesh).geometry.dispose();
  }
  cells = [];
  sheet.replaceChildren();
  ground.geometry.dispose();
  ground.geometry = paint(soft(new THREE.CircleGeometry(1, 48)), hex(MEADOW[season]));
  if (name === 'all') allModels(season);
  else if (name === 'missing') missing(season);
  else packSheet(name, season);

  for (const button of document.querySelectorAll<HTMLButtonElement>('.chips button')) {
    button.setAttribute('aria-pressed', String(button.value === name || button.value === season));
  }
  history.replaceState(null, '', `#${name}/${season}`);
  if (still) cells.forEach(picture);
}

const KITS = [...Object.keys(SHEETS), ...PACK_NAMES];
const [hashKit, hashSeason] = location.hash.slice(1).split('/');
const state = {
  kit: KITS.includes(hashKit) ? hashKit : 'all',
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
/** One cell's model on the turntable, turned `turn` radians, into the current viewport. */
function render({ model, base, disc }: Cell, aspect: number, turn: number): void {
  camera.aspect = aspect;
  camera.updateProjectionMatrix();
  model.rotation.z = 0.6 + turn;
  model.visible = true;
  ground.position.z = base;
  ground.scale.setScalar(disc);
  renderer.render(scene, camera);
  model.visible = false;
}

/** Renders a cell once and keeps the picture in its view box. */
function picture(cell: Cell): void {
  const { width, height } = cell.view.getBoundingClientRect();
  renderer.setSize(width, height, false);
  render(cell, width / height, 0);
  const image = cell.view.appendChild(Object.assign(document.createElement('canvas'), { width: canvas.width, height: canvas.height }));
  image.getContext('2d')!.drawImage(canvas, 0, 0);
}

function draw(time: number): void {
  const [width, height] = [innerWidth, innerHeight];
  const ratio = renderer.getPixelRatio();
  if (canvas.width !== Math.floor(width * ratio) || canvas.height !== Math.floor(height * ratio)) renderer.setSize(width, height, false);
  renderer.setScissorTest(false);
  renderer.clear();
  renderer.setScissorTest(true);
  for (const cell of cells) {
    const r = cell.view.getBoundingClientRect();
    if (r.bottom < 0 || r.top > height || r.right < 0 || r.left > width) continue;
    renderer.setViewport(r.left, height - r.bottom, r.width, r.height);
    renderer.setScissor(r.left, height - r.bottom, r.width, r.height);
    render(cell, r.width / r.height, time * 0.0004);
  }
}

const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
chips('kits', KITS, (k) => SHEETS[k] ?? packNamed(k).name, (k) => (state.kit = k));
chips('seasons', SEASONS, (s) => s[0].toUpperCase() + s.slice(1), (s) => (state.season = s as Season));
show(state.kit, state.season);
if (!still) renderer.setAnimationLoop(draw);
