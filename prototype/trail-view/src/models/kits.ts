/** The regional kits: the trees, ground plants and buildings a place draws, as data. */

import type { Habitat } from '../bake';
import type { Weather } from '../weather';
import type { BuildingStyle } from './buildings';
import type { PlantName } from './plants';
import type { TreeName } from './trees';

export interface Species {
  tree: TreeName;
  /** The OSM leaf_type it stands for, and the WorldCover class it stands on. */
  leaf: 'needleleaved' | 'broadleaved';
  cover: 'tree' | 'shrub';
  /** Elevation band in metres. */
  elevation: [number, number];
  /** Its share among the species that fit a spot. */
  weight: number;
  /** Drawn in its summer leaves all year: it stands in for an evergreen the library has no model of. */
  evergreen?: boolean;
}

/** How the ground itself reads in a region, beyond the WorldCover palette. */
export interface Land {
  /** Ramps that replace palette.ts's for these WorldCover classes, low NDVI to high. */
  ramps?: Partial<Record<'shrub' | 'grass' | 'moss', [string, string, string]>>;
  /**
   * Moorland: open ground that Sentinel-2 sees brown rather than green, painted in its own
   * colour with heather patches over a share of it. Heather plants grow there and grass does not.
   */
  moor?: { colour: string; heather: string; share: number };
  /** The look's palette snap for this region: 1 paints the palette alone. */
  snap?: number;
  /** The look's haze distance at sea level for this region, in km: washed Atlantic air sees far. */
  hazeKm?: number;
  /** What the region's climate changes in each weather preset. */
  weather?: Partial<Record<'sunny' | 'cloudy' | 'rainy', Partial<Weather>>>;
  /** Colours for the habitats of a fine land cover map, where the bake has one; others keep the palette. */
  habitats?: Partial<Record<Habitat, string>>;
  /** How much open ground breaks through as rock slabs on slopes, ridges and poor grass, 0–1. */
  outcrops?: number;
  /** Cliffs and slabs. */
  rock?: string;
  /** The rock's bedding planes in degrees, so cliffs show their strata tilted as they are. */
  bedding?: { strike: number; dip: number };
  /** Open water away from the shore, and beside it. */
  sea?: { deep: string; shallow: string };
}

export interface Kit {
  name: string;
  trees: Species[];
  ground: PlantName[];
  style: BuildingStyle;
  /** What cottages and houses change from the style, where they differ from farms. */
  houses?: Partial<BuildingStyle>;
  /** The tree that stands in OSM orchards; without one, the kit's broadleaves do. */
  orchard?: TreeName;
  land?: Land;
}

const BLACK_FOREST: Kit = {
  name: 'Black Forest',
  trees: [
    { tree: 'norway-spruce', leaf: 'needleleaved', cover: 'tree', elevation: [300, 1300], weight: 4 },
    { tree: 'silver-fir', leaf: 'needleleaved', cover: 'tree', elevation: [300, 1200], weight: 3 },
    { tree: 'european-beech', leaf: 'broadleaved', cover: 'tree', elevation: [200, 1100], weight: 5 },
  ],
  ground: ['grass', 'fern'],
  // Stands in for apple and cherry trees, small and white in spring.
  orchard: 'hawthorn',
  // Town and village houses have a plain steep gable; the half-hip belongs to the farms.
  houses: { name: 'Town house', pitch: [40, 48], hip: 0, eave: 0.5, verge: 0.4, timber: 'none', chimneys: 1 },
  style: {
    name: 'Black Forest farmhouse',
    pitch: [44, 52],
    eave: 1,
    verge: 0.7,
    hip: 0.55,
    timber: 'upper',
    shutters: true,
    chimneys: 0,
    palette: {
      render: ['#f1ece0', '#e8e0cc'],
      wood: ['#4f3626', '#5e4030', '#6b4a32'],
      roof: ['#5b5048', '#6a5d52', '#8a4a36'],
      door: ['#5e4030', '#3f5a3a'],
      shutter: ['#3f6a42', '#2f4a36', '#a8382e'],
      plinth: '#8f8a80',
      chimney: '#a39a8e',
    },
  },
};

export const KITS = {
  alps: {
    name: 'Alps',
    trees: [
      { tree: 'norway-spruce', leaf: 'needleleaved', cover: 'tree', elevation: [600, 2000], weight: 5 },
      { tree: 'silver-fir', leaf: 'needleleaved', cover: 'tree', elevation: [600, 1500], weight: 2 },
      { tree: 'european-larch', leaf: 'needleleaved', cover: 'tree', elevation: [1400, 2300], weight: 3 },
      { tree: 'sycamore-maple', leaf: 'broadleaved', cover: 'tree', elevation: [500, 1700], weight: 1 },
    ],
    ground: ['grass', 'alpine-flowers'],
    style: {
      name: 'Chalet',
      pitch: [18, 26],
      eave: 1.1,
      verge: 1.4,
      hip: 0,
      timber: 'upper',
      shutters: true,
      chimneys: 1,
      palette: {
        render: ['#efe9dc', '#e9e2d0', '#f3efe6'],
        wood: ['#6b4a32', '#5a3b28', '#7a5434'],
        roof: ['#6f6a66', '#5a4f48', '#7d7670'],
        door: ['#5a3b28', '#7a5434'],
        shutter: ['#3f6a42', '#a8382e', '#5a3b28'],
        plinth: '#9a958c',
        chimney: '#d9d3c6',
      },
    },
  },
  'black-forest': BLACK_FOREST,
  // The Vosges share the Black Forest's trees and farms across the Rhine.
  vosges: { ...BLACK_FOREST, name: 'Vosges' },
  'irish-coast': {
    name: 'Irish coast',
    trees: [
      { tree: 'hawthorn', leaf: 'broadleaved', cover: 'tree', elevation: [0, 250], weight: 1 },
      { tree: 'gorse', leaf: 'broadleaved', cover: 'shrub', elevation: [0, 350], weight: 1 },
    ],
    ground: ['grass', 'heather', 'heather-bloom'],
    // Washed Atlantic headlands: bright grass, ochre moor and heather on poor ground, grey slabs
    // breaking through, a navy sea that turns turquoise at the rocks.
    land: {
      ramps: {
        grass: ['#8f8455', '#7c8c52', '#5f8748'],
        shrub: ['#7d5e48', '#86703f', '#6f7c3e'],
        moss: ['#8a7a58', '#8a8a5a', '#7a8c56'],
      },
      moor: { colour: '#7e7152', heather: '#6e5266', share: 0.4 },
      habitats: {
        rock: '#8d8f88',
        heath: '#7e7152',
        bog: '#86704a',
        bracken: '#6f8c3a',
        rough: '#788a4e',
        pasture: '#5f8748',
        scrub: '#55652f',
        hedge: '#3b5530',
        sand: '#d6cba8',
        saltmarsh: '#7c8a60',
      },
      snap: 0.85,
      hazeKm: 70,
      // Fair weather off the Atlantic is low cumulus with a deep blue between.
      weather: { sunny: { cover: 0.32, base: 900, dark: 0.4, zenith: '#5d7fae', horizon: '#a9bfd4', grey: 0.35, towers: 0.8 } },
      outcrops: 0.55,
      rock: '#8d8f88',
      // Old Red Sandstone of the south-west peninsulas: GSI bedrock structure points around
      // Sheep's Head and Mizen give strike 38–64° and dip 50–80°.
      bedding: { strike: 55, dip: 65 },
      sea: { deep: '#0e3461', shallow: '#2a9a98' },
    },
    style: {
      name: 'Irish cottage',
      pitch: [36, 44],
      eave: 0.2,
      verge: 0.15,
      hip: 0,
      timber: 'none',
      shutters: false,
      chimneys: 2,
      palette: {
        // Mostly whitewash, now and then a colour.
        render: ['#f4f1e8', '#f4f1e8', '#efe8d2', '#e8d690', '#b9cdd6', '#e6bcb2'],
        wood: ['#5a4a3e'],
        roof: ['#4e555c', '#5b6168', '#454b52'],
        door: ['#a8322c', '#2f5a8a', '#3b6b4a', '#e2b33a'],
        shutter: ['#2e3a44'],
        plinth: '#9a958c',
        chimney: '#f0ece2',
      },
    },
  },
  cascades: {
    name: 'Cascades',
    trees: [
      { tree: 'douglas-fir', leaf: 'needleleaved', cover: 'tree', elevation: [0, 1300], weight: 4 },
      // Stands in for mountain hemlock up to the treeline.
      { tree: 'western-hemlock', leaf: 'needleleaved', cover: 'tree', elevation: [0, 1800], weight: 4 },
      { tree: 'western-red-cedar', leaf: 'needleleaved', cover: 'tree', elevation: [0, 1100], weight: 2 },
    ],
    ground: ['fern', 'huckleberry', 'grass'],
    style: {
      name: 'Wood cabin',
      pitch: [26, 36],
      eave: 0.6,
      verge: 0.5,
      hip: 0,
      timber: 'all',
      shutters: false,
      chimneys: 1,
      palette: {
        render: ['#d9d0bf'],
        wood: ['#8a5a3a', '#a8774c', '#6b4c36', '#7a5a40'],
        roof: ['#3f5a48', '#4a3a30', '#5f6466', '#7a3a2c'],
        door: ['#4a3324', '#7a3a2c', '#2f4a3a'],
        shutter: ['#4a3324'],
        plinth: '#7d7a74',
        chimney: '#8f8a84',
      },
    },
  },
  mediterranean: {
    name: 'Mediterranean',
    // Stand-ins until the library has Aleppo pine, holm oak and carob: a conifer for the pines,
    // a round broadleaf for the evergreen oaks. Gorse grows in the Valencian garrigue itself.
    trees: [
      { tree: 'douglas-fir', leaf: 'needleleaved', cover: 'tree', elevation: [0, 1600], weight: 1 },
      { tree: 'sycamore-maple', leaf: 'broadleaved', cover: 'tree', elevation: [0, 1600], weight: 1, evergreen: true },
      { tree: 'gorse', leaf: 'broadleaved', cover: 'shrub', elevation: [0, 1600], weight: 1 },
    ],
    ground: ['grass', 'heather'],
    // Stands in for almond and olive trees.
    orchard: 'hawthorn',
    style: {
      name: 'Village house',
      pitch: [18, 26],
      eave: 0.35,
      verge: 0.2,
      hip: 0,
      timber: 'none',
      shutters: true,
      chimneys: 1,
      palette: {
        render: ['#f6f2e8', '#f2ead8', '#efe2c4', '#f6f2e8'],
        wood: ['#6b4a32'],
        roof: ['#b5653e', '#a5583a', '#c0744a'],
        door: ['#5e4030', '#3f5a3a', '#2f4a6a'],
        shutter: ['#3f6a42', '#5e4030', '#2f4a6a'],
        plinth: '#c8bca4',
        chimney: '#f2ead8',
      },
    },
  },
} satisfies Record<string, Kit>;

export type KitName = keyof typeof KITS;
