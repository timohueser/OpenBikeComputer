import { config } from '../map-data';
import { availableLayers } from './data-layer';
import { snowLayer } from './snow-layer.svelte';
import { weatherLayer } from './weather-layer.svelte';
import { windLayer } from './wind-layer.svelte';

/** The data layers of the layer menu, in menu order. */
export const dataLayers = availableLayers([
    { archives: ['snow', 'snow-levels', 'snow-seasons'], create: urls => snowLayer(urls.snow, urls['snow-levels'], urls['snow-seasons']) },
    { archives: ['climate'], create: urls => weatherLayer(urls.climate) },
    { archives: ['climate'], create: urls => windLayer(urls.climate) },
], config.layers);
