<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import MapControlPanel from './MapControlPanel.svelte';
    import { categoryIds, placeCategories } from '../../lib/planner/poi-kinds';
    import { placeChoice, toggleHighlight, toggleKind } from '../../lib/planner/place-choice.svelte';
</script>

<MapControlPanel label="Kinds of places">
    {#snippet button()}
        {#if placeChoice.highlight}
            <span class="mark lit"><Icon path={placeCategories[placeChoice.highlight].icon} size={12} /></span>
        {:else}
            <Icon name="sliders" />
        {/if}
    {/snippet}
    <div class="head"><strong>Places</strong><span aria-hidden="true">Highlight</span></div>
    <ul>
        {#each categoryIds as kind (kind)}
            {@const { icon, plural } = placeCategories[kind]}
            {@const lit = placeChoice.highlight === kind}
            <li>
                <label>
                    <input type="checkbox" checked={placeChoice.kinds.includes(kind)} onchange={() => toggleKind(kind)} />
                    <Icon path={icon} size={15} />{plural}
                </label>
                <button type="button" class="highlight" aria-pressed={lit} aria-label={`Highlight ${plural}`} onclick={() => toggleHighlight(kind)}>
                    <span class="mark" class:lit><Icon path={icon} size={12} /></span>
                </button>
            </li>
        {/each}
    </ul>
    <p>Icons show when you zoom in. A highlighted kind shows from further out.</p>
</MapControlPanel>

<style>
    button { padding: 0; border: 0; background: none; cursor: pointer; }

    /* The look of a mark on the map: an empty ring, or the rust disc of the highlighted kind. */
    .mark {
        display: grid; place-items: center; width: 20px; height: 20px; box-sizing: border-box; border-radius: 50%;
        border: 1.5px solid var(--line-strong); color: var(--ink-soft);
        transition: transform 120ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .mark :global(svg) { opacity: .55; stroke-width: 2; }
    .mark.lit { border: 0; background: var(--place-disc); color: #fff; }
    .mark.lit :global(svg) { opacity: 1; }

    .head { display: flex; align-items: baseline; justify-content: space-between; padding-bottom: 4px; }
    .head strong { font-weight: 600; }
    .head span { font-size: 11px; color: var(--ink-soft); }
    ul { margin: 0; padding: 0; list-style: none; }
    li { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
    label { display: flex; flex: 1; align-items: center; gap: 8px; min-height: 32px; color: var(--ink); cursor: pointer; }
    label :global(svg) { flex-shrink: 0; color: var(--ink-soft); }
    input { width: 18px; height: 18px; margin: 0; accent-color: var(--link); cursor: pointer; }
    .highlight { display: grid; place-items: center; width: 40px; height: 32px; margin-right: -6px; border-radius: 6px; }
    .highlight:active .mark { transform: scale(0.95); }
    p { margin: 8px 0 0; font-size: 12px; line-height: 1.45; color: var(--ink-soft); }

    @media (hover: hover) and (pointer: fine) {
        .highlight:hover .mark { transform: scale(1.1); border-color: var(--ink-soft); }
        .highlight:hover .mark :global(svg) { opacity: .9; }
        .highlight:hover .mark.lit :global(svg) { opacity: 1; }
        .highlight:hover:active .mark { transform: scale(0.95); }
    }
    @media (prefers-reduced-motion: reduce) {
        .mark { transform: none !important; }
    }
</style>
