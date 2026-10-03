import { DATA_URLS } from '../map-data';
import { availableLayers } from './data-layer';
import { snowLayer } from './snow-layer.svelte';
import { weatherLayer } from './weather-layer.svelte';

/** The data layers of the layer menu, in menu order. */
export const dataLayers = availableLayers([
    { archive: 'snow', create: snowLayer },
    { archive: 'climate', create: weatherLayer },
], DATA_URLS);
