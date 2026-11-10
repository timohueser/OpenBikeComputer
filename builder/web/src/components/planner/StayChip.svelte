<script lang="ts">
    import { tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import { allStay, type Stay } from '../../lib/planner/editor';
    import { stayIcon, stayOptions } from '../../lib/planner/search/presentation';

    let { value, onChange, onReset, resetLabel }: {
        value: Stay[];
        onChange: (kinds: Stay[]) => void;
        onReset?: () => void;
        resetLabel?: string;
    } = $props();

    let expanded = $state(false);
    let root = $state<HTMLDivElement>();
    let trigger: HTMLButtonElement;
    let panel = $state<HTMLDivElement>();
    const chosen = $derived(stayOptions.filter(option => value.includes(option.kind)));
    const text = $derived(chosen.length === allStay.length ? 'Any place' : chosen.length === 1 ? chosen[0].label : `${chosen[0]?.label} +${chosen.length - 1}`);

    async function toggle() {
        if (expanded) { close(); return; }
        expanded = true;
        await tick();
        panel?.querySelector<HTMLElement>('input')?.focus({ preventScroll: true });
    }

    function close() {
        expanded = false;
        trigger?.focus({ preventScroll: true });
    }
</script>

<svelte:window onpointerdown={(event) => { if (expanded && !root?.contains(event.target as Node)) expanded = false; }} />

<!-- The panel is a flex item of the wrapping row around the chip, so it takes a full line below it and never overflows a narrow side panel. -->
<div class="stay-chip" bind:this={root}>
    <button bind:this={trigger} type="button" class="planner-chip" aria-label={`Places to sleep: ${text}`} aria-expanded={expanded} onclick={toggle}>
        <Icon name={stayIcon(chosen[0]?.kind)} size={14} />{text}
    </button>
    {#if expanded}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <div class="panel" bind:this={panel} role="group" aria-label="Places to sleep" onkeydown={(event) => { if (event.key === 'Escape') { event.stopPropagation(); close(); } }}>
            {#each stayOptions as { kind, label } (kind)}
                {@const on = value.includes(kind)}
                <label><input type="checkbox" checked={on} disabled={on && value.length === 1} onchange={() => onChange(allStay.filter(k => k === kind ? !on : value.includes(k)))} /><Icon name={stayIcon(kind)} size={14} />{label}</label>
            {/each}
            {#if onReset}<button type="button" class="planner-action reset" onclick={() => { onReset(); close(); }}>{resetLabel}</button>{/if}
        </div>
    {/if}
</div>

<style>
    .stay-chip { display: contents; }
    .planner-chip { --chip-ink: var(--query-place); white-space: nowrap; }
    .panel {
        order: 2;
        flex-basis: 100%;
        display: flex;
        flex-direction: column;
        padding: 6px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--panel);
        font: 600 13px var(--sans);
        color: var(--ink);
    }
    label {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 32px;
        padding: 0 6px;
        border-radius: 6px;
        cursor: pointer;
    }
    label:hover { background: var(--parchment-2); }
    label :global(svg) { color: var(--ink-soft); }
    input { width: 14px; height: 14px; margin: 0; accent-color: var(--ink); }
    input:disabled { cursor: default; }
    input:focus-visible, .reset:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .reset { align-self: flex-start; margin-top: 6px; }
</style>
