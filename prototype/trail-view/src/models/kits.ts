/** The regional kits: the trees, ground plants and buildings a place draws, as data. */

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
}

export interface Kit {
  name: string;
  trees: Species[];
  ground: PlantName[];
  style: BuildingStyle;
  /** What cottages and houses change from the style, where they differ from farms. */
  houses?: Partial<BuildingStyle>;
}

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
  'black-forest': {
    name: 'Black Forest',
    trees: [
      { tree: 'norway-spruce', leaf: 'needleleaved', cover: 'tree', elevation: [300, 1300], weight: 4 },
      { tree: 'silver-fir', leaf: 'needleleaved', cover: 'tree', elevation: [300, 1200], weight: 3 },
      { tree: 'european-beech', leaf: 'broadleaved', cover: 'tree', elevation: [200, 1100], weight: 5 },
    ],
    ground: ['grass', 'fern'],
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
  },
  'irish-coast': {
    name: 'Irish coast',
    trees: [
      { tree: 'hawthorn', leaf: 'broadleaved', cover: 'tree', elevation: [0, 250], weight: 1 },
      { tree: 'gorse', leaf: 'broadleaved', cover: 'shrub', elevation: [0, 350], weight: 1 },
    ],
    ground: ['grass', 'heather', 'heather-bloom'],
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
} satisfies Record<string, Kit>;

export type KitName = keyof typeof KITS;
