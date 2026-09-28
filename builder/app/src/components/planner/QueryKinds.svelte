<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { allKinds, category, kindLabel } from '../../lib/planner/search/presentation';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    let { value, onChange }: { value: string[]; onChange: (value: string[]) => void } = $props();
    let filter = $state('');
    const choices = $derived(allKinds.filter(kind => kindLabel(kind).includes(filter.trim().toLowerCase())));
    function toggle(kind: string) {
        onChange(value.includes(kind) ? value.filter(v => v !== kind) : [...value, kind]);
    }
</script>
<div class="kinds">
    <label class="find"><Icon name="search" size={15} /><input aria-label="Find a place type" placeholder="Find a type…" bind:value={filter} /></label>
    <div class="choices" role="group" aria-label="Place types">
        {#each choices as kind}
            <button type="button" class:chosen={value.includes(kind)} aria-pressed={value.includes(kind)} disabled={!value.includes(kind) && value.length >= 3} onclick={() => toggle(kind)}>
                <Icon path={placeCategories[category(kind)].icon} size={16} /><span>{kindLabel(kind)}</span>
                {#if value.includes(kind)}<Icon name="check" size={14} />{/if}
            </button>
        {:else}<p>No matching type.</p>{/each}
    </div>
    <p>{value.length} of 3 types selected</p>
</div>
<style>
    .kinds { grid-column: 1 / -1; min-width: 0; }
    .find { position: relative; }
    .find > :global(svg) { position: absolute; left: 10px; top: 12px; color: var(--ink-soft); }
    .find input { padding-left: 32px !important; }
    .choices { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 4px; max-height: 224px; overflow-y: auto; margin-top: 12px; padding: 2px; scrollbar-width: thin; scrollbar-color: var(--line-strong) transparent; }
    .choices button { display: flex; align-items: center; gap: 7px; min-height: 40px; padding: 8px; border: 0; border-radius: 6px; background: transparent; color: var(--ink); font: inherit; text-align: left; cursor: pointer; }
    .choices button span { flex: 1; overflow-wrap: anywhere; }
    .choices button :global(svg) { flex: none; }
    .choices button:hover { background: var(--parchment-2); }
    .choices button.chosen { background: var(--filter-tint); color: var(--filter-ink); }
    .choices button:disabled { opacity: .45; cursor: default; }
    .choices button:focus-visible { outline: 2px solid var(--ink); outline-offset: -2px; }
    p { margin: 10px 0 0; color: var(--ink-soft); font-size: 12px; }
</style>
