import { config } from '../map-data';
import { availableLayers } from './data-layer';
import { sunLayer } from './sun-layer.svelte';
import { snowLayer } from './snow-layer.svelte';
import { weatherLayer } from './weather-layer.svelte';
import { windLayer } from './wind-layer.svelte';

/** The data layers of the layer menu, in menu order. */
export const dataLayers = availableLayers([
    { archive: 'sun', create: sunLayer },
    { archive: 'snow', create: snowLayer },
    { archive: 'climate', create: weatherLayer },
    { archive: 'climate', create: windLayer },
], config.layers);
