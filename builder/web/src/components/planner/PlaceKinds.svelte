<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import { categoryIds, placeCategories } from '../../lib/planner/poi-kinds';
    import { placeChoice, toggleHighlight, toggleKind } from '../../lib/planner/place-choice.svelte';

    let open = $state(false);
    let root: HTMLDivElement;
    let opener: HTMLButtonElement;

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }

    // A focus that moves to another control closes the panel, so only one panel of the controls is open.
    function leave(event: FocusEvent) {
        if (open && event.relatedTarget && !root.contains(event.relatedTarget as Node)) open = false;
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open) return;
        event.stopPropagation();
        open = false;
        opener.focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="place-kinds" bind:this={root} onkeydown={key} onfocusout={leave}>
    <button type="button" class="toggle" class:chosen={open} bind:this={opener} aria-label="Kinds of places" aria-expanded={open} aria-haspopup="dialog" onclick={() => open = !open}>
        {#if placeChoice.highlight}
            <span class="mark lit"><Icon path={placeCategories[placeChoice.highlight].icon} size={12} /></span>
        {:else}
            <Icon name="sliders" />
        {/if}
    </button>
    {#if open}
        <div class="panel" role="dialog" aria-label="Kinds of places" tabindex="-1">
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
        </div>
    {/if}
</div>

<style>
    /* The frame and the button of the map controls, as in the map settings. */
    .place-kinds { border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); }
    button { padding: 0; border: 0; background: none; color: var(--ink); cursor: pointer; }
    .toggle { display: grid; place-items: center; width: 36px; height: 36px; border-radius: 8px; }
    .toggle:hover, .toggle.chosen { color: var(--link); }

    /* The look of a mark on the map: an empty ring, or the rust disc of the highlighted kind. */
    .mark {
        display: grid; place-items: center; width: 20px; height: 20px; box-sizing: border-box; border-radius: 50%;
        border: 1.5px solid var(--line-strong); color: var(--ink-soft);
        transition: transform 120ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .mark :global(svg) { opacity: .55; stroke-width: 2; }
    .mark.lit { border: 0; background: var(--rust, #a4501e); color: #fff; }
    .mark.lit :global(svg) { opacity: 1; }
    :global([data-theme="dark"]) .mark.lit { background: #c8642a; }

    /* Positioned against the controls column, as the map settings are. */
    .panel {
        position: absolute; top: 0; right: 44px;
        width: min(288px, calc(100vw - 80px)); box-sizing: border-box;
        max-height: calc(var(--map-height, 100vh) - 32px); overflow: auto;
        padding: 10px 12px 12px; border-radius: 8px;
        background: var(--panel); box-shadow: var(--planner-shadow); font-size: 13px;
    }
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
