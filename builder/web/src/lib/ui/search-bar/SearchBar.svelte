<!-- A search bar of fixed slots. Each slot shows its values as words and opens one panel under it; one panel is open at
     a time. The cross empties the search, and "Undo" brings it back for a short time. -->
<script lang="ts" module>
    /** `words` are the values of the slot in their order; `unset` shows when it has none. A slot with `off` is disabled
     * and shows `off`. */
    export interface Slot { id: string; icon: string; label: string; words: string[]; unset: string; off?: string }
</script>

<script lang="ts">
    import type { Snippet } from 'svelte';
    import Icon from '../Icon.svelte';

    let { label, slots, panel, onreset, empty, onclear, onundo, open = $bindable() }: {
        label: string;
        slots: Slot[];
        /** The content of the panel of a slot. */
        panel: Snippet<[id: string]>;
        /** Sets the values of the slot to their defaults. */
        onreset: (id: string) => void;
        /** The search holds only defaults: the cross hides. */
        empty: boolean;
        onclear: () => void;
        onundo: () => void;
        /** The id of the slot whose panel is open. */
        open?: string;
    } = $props();

    const UNDO_MS = 8000, SHOWN = 2;
    let bar = $state<HTMLElement>(), panelWidth = $state(0), barWidth = $state(0), innerWidth = $state(0);
    /** The width of a narrow window, in which the panel is as wide as the bar. */
    const NARROW_PX = 760;
    /** "Undo" shows after the cross until its time ends or the search holds a value again. */
    let undoing = $state(false), timer: ReturnType<typeof setTimeout> | undefined;
    const current = $derived(slots.find(slot => slot.id === open && !slot.off));
    const button = (id: string) => bar?.querySelector<HTMLElement>(`[data-slot="${CSS.escape(id)}"]`);
    /** The panel starts under its slot and stays inside the bar. */
    const left = $derived.by(() => {
        if (!current || !bar) return 0;
        const at = button(current.id)?.offsetLeft ?? 0;
        return Math.max(0, Math.min(at, barWidth - panelWidth));
    });

    const text = (words: string[]) => words.length > SHOWN ? `${words.slice(0, SHOWN).join(' · ')} +${words.length - SHOWN}` : words.join(' · ');

    function close(focus: boolean) {
        const id = open;
        open = undefined;
        if (focus && id) button(id)?.focus();
    }
    function clear() {
        open = undefined;
        onclear();
        undoing = true;
        clearTimeout(timer);
        timer = setTimeout(() => undoing = false, UNDO_MS);
    }
    function undo() {
        undoing = false;
        clearTimeout(timer);
        onundo();
    }
    // Any change that gives the search a value ends "Undo".
    $effect(() => {
        if (!empty) undoing = false;
    });
    $effect(() => () => clearTimeout(timer));
    // The page can turn the slot of the open panel off.
    $effect(() => {
        if (open && !current) open = undefined;
    });

    // In the capture phase, so that Escape closes the panel before the page acts on it: page listeners skip a
    // `defaultPrevented` key. Escape in an open window closes only the window.
    function escape(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open || event.defaultPrevented || document.querySelector('dialog[open]')) return;
        event.preventDefault();
        close(!!bar?.contains(document.activeElement));
    }
</script>

<!-- A press outside the bar closes the panel and still acts where it lands, such as a pick on the map. -->
<svelte:window bind:innerWidth onkeydowncapture={escape} onpointerdown={event => { if (open && !bar?.contains(event.target as Node)) close(false); }} />

<div class="search-bar" role="search" aria-label={label} bind:this={bar} bind:clientWidth={barWidth}>
    <div class="strip">
        <div class="slots">
            {#each slots as slot (slot.id)}
                <button type="button" class="slot" class:unset={!slot.words.length || !!slot.off} data-slot={slot.id} disabled={!!slot.off}
                    aria-expanded={open === slot.id} aria-controls="search-panel" onclick={() => open = open === slot.id ? undefined : slot.id}>
                    <span class="label"><Icon name={slot.icon} size={14} />{slot.label}</span>
                    <strong>{slot.off ?? (text(slot.words) || slot.unset)}</strong>
                </button>
            {/each}
        </div>
        {#if undoing && empty}
            <div class="tail"><button type="button" class="undo" onclick={undo}>Undo</button></div>
        {:else if !empty}
            <div class="tail"><button type="button" class="clear" aria-label="Clear the search" title="Clear the search" onclick={clear}><Icon name="close" size={16} /></button></div>
        {/if}
    </div>
    {#if current}
        <div class="panel" id="search-panel" role="group" aria-label={current.label} bind:offsetWidth={panelWidth} style:left={innerWidth > NARROW_PX ? `${left}px` : undefined}>
            {@render panel(current.id)}
            <div class="foot">
                <button type="button" class="quiet" disabled={!current.words.length} onclick={() => onreset(current.id)}>Reset</button>
                <button type="button" class="done" onclick={() => close(true)}>Done</button>
            </div>
        </div>
    {/if}
</div>

<style>
    .search-bar {
        position: relative;
        z-index: 5;
    }
    .strip {
        display: flex;
        align-items: stretch;
        min-height: 58px;
        overflow: hidden;
        border: 1px solid var(--line-strong);
        border-radius: 10px;
        background: var(--panel);
    }
    .slots {
        display: flex;
        flex: 1 1 0;
        min-width: 0;
    }
    .slot {
        display: flex;
        flex: 1 1 0;
        flex-direction: column;
        justify-content: center;
        gap: 1px;
        min-width: 0;
        padding: 8px 14px;
        border: 0;
        border-left: 1px solid var(--line);
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
    }
    .slot:first-child {
        border-left: 0;
    }
    .slot:hover:not(:disabled) {
        background: var(--parchment);
    }
    .slot[aria-expanded="true"] {
        background: var(--parchment-2);
    }
    .slot:focus-visible {
        outline-offset: -3px;
    }
    .slot:disabled {
        cursor: default;
    }
    .label {
        display: flex;
        align-items: center;
        gap: 5px;
        color: var(--ink-soft);
        font-size: 12px;
        white-space: nowrap;
    }
    strong {
        overflow: hidden;
        font-weight: 600;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .unset strong {
        color: var(--ink-faint);
        font-weight: 400;
    }
    .tail {
        display: flex;
        align-items: center;
        padding: 0 8px;
        border-left: 1px solid var(--line);
    }
    .clear {
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        padding: 0;
        border: 0;
        border-radius: 50%;
        background: none;
        color: var(--ink-soft);
    }
    .clear:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .undo, .done {
        min-height: 30px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 13px var(--sans);
    }
    .undo:hover, .done:hover {
        background: var(--parchment-2);
    }
    .panel {
        position: absolute;
        top: calc(100% + 6px);
        min-width: 340px;
        max-width: 100%;
        max-height: min(560px, 74vh);
        overflow: auto;
        border: 1px solid var(--line-strong);
        border-radius: 10px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        animation: drop 160ms cubic-bezier(0.2, 0.8, 0.2, 1);
    }
    @keyframes drop {
        from { opacity: 0; transform: translateY(-4px); }
    }
    @media (prefers-reduced-motion: reduce) {
        .panel { animation-name: fade; }
        @keyframes fade {
            from { opacity: 0; }
        }
    }
    .foot {
        position: sticky;
        bottom: 0;
        display: flex;
        justify-content: space-between;
        gap: 8px;
        padding: 8px 10px;
        border-top: 1px solid var(--line);
        background: var(--panel);
    }
    .quiet {
        min-height: 30px;
        padding: 0 10px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: 600 13px var(--sans);
    }
    .quiet:hover:not(:disabled) {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .quiet:disabled {
        cursor: default;
        opacity: 0.4;
    }
    @media (max-width: 760px) {
        .slot {
            padding: 7px 10px;
        }
        strong {
            display: -webkit-box;
            white-space: normal;
            -webkit-box-orient: vertical;
            -webkit-line-clamp: 2;
            line-clamp: 2;
        }
        .panel {
            right: 0;
            left: 0;
            min-width: 0;
        }
    }
</style>
