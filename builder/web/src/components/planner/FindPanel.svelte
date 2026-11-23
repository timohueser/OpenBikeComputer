<!-- The panel of "Find": one kind of place, with the kinds under it once it is chosen, or one fact of the route, with
     "At least" for a fact that has a size. A press on the chosen row removes it. -->
<script lang="ts">
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { category, contract } from '../../lib/planner/search/presentation';
    import { factIcon, factWord, kindsUnder, kindWord, topKind, type Find } from '../../lib/planner/search/slots';
    import QueryChoice from './QueryChoice.svelte';

    let { find, onchange }: { find?: Find; onchange: (find: Find | undefined) => void } = $props();

    const kinds: Record<string, { parent: string | null }> = contract.kinds;
    const PLACES = Object.keys(kinds).filter(kind => !kinds[kind].parent);
    const FACTS = [...contract.stretches, 'gap:water'];
    /** The scale of "At least" of a fact with a size, in the unit that the search compares. */
    const SIZES: Record<string, { unit: string; max: number; step: number }> = {
        climb: { unit: 'm', max: 1000, step: 50 }, descent: { unit: 'm', max: 1000, step: 50 }, steep: { unit: '%', max: 20, step: 1 },
        unpaved: { unit: 'km', max: 30, step: 1 }, 'gap:water': { unit: 'km', max: 60, step: 5 },
    };

    const kind = $derived(find && 'kind' in find ? find.kind : undefined);
    const fact = $derived(find && 'fact' in find ? find : undefined);
    /** The size under the thumb while it moves; the search runs when it stops. */
    let moving = $state<number>();
    const least = $derived(moving ?? (fact?.min && fact.min.unit === SIZES[fact.fact]?.unit ? fact.min.value : 0));
</script>

<!-- The chosen row shows when the panel opens, also a fact under the long list of places. -->
<div class="groups" {@attach node => node.querySelector('[aria-pressed="true"]')?.scrollIntoView({ block: 'nearest' })}>
    <section aria-labelledby="find-place">
        <h3 id="find-place">A place</h3>
        {#each PLACES as one (one)}
            {@const on = !!kind && topKind(kind) === one}
            {@const under = kindsUnder(one)}
            <QueryChoice label={kindWord(one)} path={placeCategories[category(one)].icon} {on} onclick={() => onchange(on ? undefined : { kind: one })}>
                {#if under.length}
                    <div class="pills" role="group" aria-label="Which {kindWord(one).toLowerCase()}">
                        {#each [one, ...under] as sub (sub)}
                            <button type="button" aria-pressed={kind === sub} onclick={() => onchange({ kind: sub })}>{sub === one ? 'Any' : kindWord(sub)}</button>
                        {/each}
                    </div>
                {/if}
            </QueryChoice>
        {/each}
    </section>
    <section aria-labelledby="find-route">
        <h3 id="find-route">The route itself</h3>
        {#each FACTS as one (one)}
            {@const on = fact?.fact === one}
            {@const size = SIZES[one]}
            <QueryChoice label={factWord(one)} icon={factIcon(one)} {on} onclick={() => onchange(on ? undefined : { fact: one })}>
                {#if size}
                    <label class="least">
                        <span>At least</span><em>{least ? `${least} ${size.unit}` : 'Any size'}</em>
                        <input type="range" min="0" max={size.max} step={size.step} value={least} aria-valuetext={least ? `at least ${least} ${size.unit}` : 'any size'}
                            oninput={event => moving = event.currentTarget.valueAsNumber}
                            onchange={event => { const value = event.currentTarget.valueAsNumber; moving = undefined; onchange({ fact: one, ...value ? { min: { value, unit: size.unit } } : {} }); }} />
                    </label>
                {/if}
            </QueryChoice>
        {/each}
    </section>
</div>

<style>
    .groups {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
        gap: 10px 18px;
        width: 560px;
        max-width: 100%;
        padding: 10px 12px 4px;
    }
    section {
        display: flex;
        flex-direction: column;
    }
    h3 {
        margin: 0 0 4px;
        color: var(--ink-soft);
        font: 400 12px var(--sans);
    }
    .least {
        display: grid;
        grid-template-columns: 1fr auto;
        gap: 4px;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    em {
        color: var(--ink);
        font-style: normal;
        font-weight: 600;
    }
    input {
        grid-column: 1 / -1;
        width: 100%;
        margin: 0;
        accent-color: var(--ink-soft);
    }
</style>
