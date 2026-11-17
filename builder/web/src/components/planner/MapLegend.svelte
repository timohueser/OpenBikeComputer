<script lang="ts">
    // Prototype of issue 3555: the legend of the layers that are on, on the map. The sources and the caveats are behind its info control.
    import Icon from '../../lib/ui/Icon.svelte';
    import LayerLegend from './LayerLegend.svelte';
    import { bandShare, networkLevels, type OverlayOptions } from '../../lib/planner/route-overlays';
    import type { DataLayer } from '../../lib/planner/layers/data-layer';

    let { mapOverlays, dataLayer, theme = 'light', walking = false }: { mapOverlays: OverlayOptions; dataLayer?: DataLayer; theme?: 'light' | 'dark'; walking?: boolean } = $props();

    const networkNames = { cycling: 'Cycling', hiking: 'Hiking', mtb: 'MTB', none: '' };
    const network = $derived(networkNames[mapOverlays.network]);
    const legend = $derived(dataLayer?.legend(theme));
    // The snow classes as one stepped bar: the share of past years with snow, marked where one class ends.
    const snow = $derived(dataLayer?.id === 'snow' && legend && 'swatches' in legend ? { steps: legend.swatches.filter(s => !s.hatch), none: legend.swatches.find(s => s.hatch) } : null);
    let info = $state(false);
    const id = $props.id();
</script>

{#if network || dataLayer}
    <section class="map-legend" aria-label="Map legend">
        {#if info}
            <div class="info" id={`${id}-info`}>
                {#if network}<p><b>{network}</b> Right-click or long-press a route for details.</p>{/if}
                {#if dataLayer}
                    <p><b>{dataLayer.label}</b> {dataLayer.description} {dataLayer.caveat}</p>
                    {#if dataLayer.error || dataLayer.source}<p class="source">{dataLayer.error || dataLayer.source}</p>{/if}
                {/if}
                {#if mapOverlays.access}<p><b>Closures</b> {walking ? '' : 'A walking symbol means dismount and push. '}Click a symbol for its rules. Closure reports are not live.</p>{/if}
            </div>
        {/if}
        <div class="rows">
            {#if network}
                <div class="row">
                    <b>{network}</b>
                    <ul aria-label={`${network} route colours`} style:--band-share={`${bandShare[theme] * 100}%`}>
                        {#each networkLevels as level (level.rank)}<li title={level.label}><span class="band" style:color={theme === 'dark' ? level.dark : level.color}></span>{level.rank === 3 ? 'National / intl.' : level.rank === 0 ? 'Unspecified' : level.label}</li>{/each}
                    </ul>
                </div>
            {/if}
            {#if dataLayer && legend}
                <div class="row">
                    <b>{dataLayer.label}</b>
                    <div class="content">{#if snow}
                        <div class="steps" role="img" aria-label={`Snow colours: ${snow.steps.map(s => s.label).join('; ')}`}>
                            <span class="bar">{#each snow.steps as step (step.label)}<i style:background={step.color} title={step.label}></i>{/each}</span>
                            <span class="ticks" aria-hidden="true"><span>¼</span><span>½</span><span>¾</span><span>every year</span></span>
                        </div>
                        {#if snow.none}<span class="none"><span class="hatch" style:--swatch={snow.none.color}></span>{snow.none.label}</span>{/if}
                    {:else}
                        <LayerLegend {legend} label={`${dataLayer.label} colours`} />
                    {/if}</div>
                </div>
            {/if}
        </div>
        <button type="button" class="about" class:chosen={info} aria-label="About these layers" aria-expanded={info} aria-controls={`${id}-info`} onclick={() => info = !info}>
            <Icon path="M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 8v5.5m0-9v.2" size={16} />
        </button>
    </section>
{/if}

<style>
    .map-legend { position: relative; width: fit-content; max-width: min(430px, 100%); padding: 7px 34px 7px 12px; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); font-size: 12px; line-height: 18px; color: var(--ink-soft); font-variant-numeric: tabular-nums; }
    .rows { display: grid; grid-template-columns: auto 1fr; gap: 4px 12px; align-items: start; }
    .row { display: contents; }
    .content { display: flex; flex-wrap: wrap; align-items: flex-start; gap: 2px 14px; min-width: 0; }
    b { font-weight: 600; color: var(--ink); }
    ul { display: flex; flex-wrap: wrap; gap: 2px 12px; margin: 0; padding: 0; list-style: none; }
    li, .none { display: flex; align-items: center; gap: 6px; white-space: nowrap; }
    /* A band with its line: the map draws a route network as a line when far out and as a band when close. */
    .band { flex: 0 0 14px; height: 8px; border-radius: 2px; background: linear-gradient(currentColor 0 0) center / 100% 2px no-repeat, color-mix(in srgb, currentColor var(--band-share), var(--parchment)); }
    .steps { display: grid; gap: 1px; width: 152px; padding-top: 5px; }
    .bar { display: flex; width: 120px; height: 8px; border-radius: 2px; overflow: hidden; box-shadow: 0 0 0 1px var(--line-strong); }
    .bar i { flex: 1; }
    /* A mark stands under the edge between two classes; the last class has its name. */
    .ticks { position: relative; display: block; height: 15px; font-size: 11px; line-height: 15px; white-space: nowrap; }
    .ticks span { position: absolute; left: 24px; translate: -50%; }
    .ticks span:nth-child(2) { left: 48px; }
    .ticks span:nth-child(3) { left: 72px; }
    .ticks span:nth-child(4) { left: 98px; translate: none; }
    .hatch { flex: 0 0 14px; height: 8px; border-radius: 2px; background: repeating-linear-gradient(135deg, transparent 0 2px, var(--swatch) 2px 3.5px); box-shadow: inset 0 0 0 1px var(--line-strong); }
    .about { position: absolute; right: 4px; bottom: 4px; display: grid; place-items: center; width: 26px; height: 26px; padding: 0; border: 0; border-radius: 6px; background: none; color: var(--ink-soft); }
    .about:hover, .about.chosen { color: var(--link); }
    .about.chosen { background: var(--parchment-2); }
    .info { max-width: 380px; max-height: 190px; margin: 3px -22px 8px 0; padding-bottom: 8px; overflow: auto; border-bottom: 1px solid var(--line); }
    .info p { margin: 0 0 6px; line-height: 17px; }
    .info p:last-child { margin: 0; }
    .source { font-size: 11px; line-height: 15px; overflow-wrap: anywhere; }
</style>
