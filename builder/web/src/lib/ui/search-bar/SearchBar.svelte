<!-- A search bar: a text field, then fixed slots. Each slot shows its values as words and opens one panel under it; the
     list of the text field is a panel too, and one panel is open at a time. Enter in the field sends the words of each
     value to its slot. The cross empties the search, and "Undo" brings it back for a short time. -->
<script lang="ts" module>
    /** `words` are the values of the slot in their order; `unset` shows when it has none. `set` says that a value differs
     * from its default, also one with no word, so "Reset" has work. A slot with `off` is disabled and shows `off`. */
    export interface Slot { id: string; icon: string; label: string; words: string[]; unset: string; set: boolean; off?: string }
</script>

<script lang="ts" generics="V extends Value, P extends Place">
    import { untrack, type Snippet } from 'svelte';
    import Icon from '../Icon.svelte';
    import Field, { type FieldProps, type Place } from './Field.svelte';
    import type { Value } from './read';

    let { label, slots, field, panel, onreset, empty, onclear, onundo, open = $bindable() }: {
        label: string;
        slots: Slot[];
        field: FieldProps<V, P>;
        /** The content of the panel of a slot. */
        panel: Snippet<[id: string]>;
        /** Sets the values of the slot to their defaults. */
        onreset: (id: string) => void;
        /** No slot that is on holds a value: with an empty field, the cross hides. */
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
    /** The text of the field, the text before the cross, and whether the list of the field is open. */
    let typed = $state(''), clearedText = '', listing = $state(false);
    const blank = $derived(empty && !typed);
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
        [clearedText, typed] = [typed, ''];
        onclear();
        undoing = true;
        clearTimeout(timer);
        timer = setTimeout(() => undoing = false, UNDO_MS);
    }
    function undo() {
        undoing = false;
        clearTimeout(timer);
        typed = clearedText;
        onundo();
    }
    // Any change that gives the search a value ends "Undo".
    $effect(() => {
        if (!blank) undoing = false;
    });
    // The list of the field and the panel of a slot close each other. A panel that closes ends a search.
    $effect(() => {
        if (listing) untrack(() => open = undefined);
    });
    let was: string | undefined;
    $effect(() => {
        const now = open;
        if (now) untrack(() => listing = false);
        if (was && now !== was) untrack(() => field.onsearch());
        was = now;
    });

    /** The words of a value that fly from the field to their slot, one after the other. */
    let ghosts = $state<{ id: number; slot: string; label: string; from: DOMRect; to: DOMRect; delay: number }[]>([]), flights = 0;
    const FLY_MS = 360;
    /** The words of a value fly from `from` to its slot, which then flashes. With reduced motion, only the slot flashes. */
    function land(value: V, from: DOMRect | undefined, delay: number) {
        const to = button(value.slot)?.querySelector('strong')?.getBoundingClientRect();
        if (!from || !to || matchMedia('(prefers-reduced-motion: reduce)').matches) return flash(value.slot);
        ghosts.push({ id: flights++, slot: value.slot, label: value.label, from, to, delay });
    }
    function fly(node: HTMLElement, ghost: typeof ghosts[number]) {
        const { from, to, delay } = ghost;
        node.animate([{ transform: 'none', opacity: 1 }, { transform: `translate(${to.left - from.left}px, ${to.top - from.top}px) scale(0.94)`, opacity: 0.35 }],
            { duration: FLY_MS, delay, easing: 'cubic-bezier(0.3, 0.7, 0.2, 1)', fill: 'backwards' }).onfinish = () => {
            ghosts = ghosts.filter(one => one.id !== ghost.id);
            flash(ghost.slot);
        };
    }
    function flash(slot: string) {
        const node = button(slot);
        if (!node) return;
        const style = getComputedStyle(node), amber = style.getPropertyValue('--amber'), panel = style.getPropertyValue('--panel');
        node.animate([{ backgroundColor: `color-mix(in srgb, ${amber} 40%, ${panel})` }, {}], { duration: FLY_MS, easing: 'ease-out' });
    }
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
        <Field {...field} bind:text={typed} bind:listing slotLabel={id => slots.find(slot => slot.id === id)?.label ?? ''} {land} />
        <div class="slots">
            {#each slots as slot (slot.id)}
                <button type="button" class="slot" class:unset={!slot.words.length || !!slot.off} data-slot={slot.id} disabled={!!slot.off}
                    aria-expanded={open === slot.id} aria-controls="search-panel" onclick={() => open = open === slot.id ? undefined : slot.id}>
                    <span class="label"><Icon name={slot.icon} size={14} />{slot.label}</span>
                    <strong>{slot.off ?? (text(slot.words) || slot.unset)}</strong>
                </button>
            {/each}
        </div>
        {#if undoing && blank}
            <div class="tail"><button type="button" class="undo" onclick={undo}>Undo</button></div>
        {:else if !blank}
            <div class="tail"><button type="button" class="clear" aria-label="Clear the search" title="Clear the search" onclick={clear}><Icon name="close" size={16} /></button></div>
        {/if}
    </div>
    {#if current}
        <div class="search-panel" id="search-panel" role="group" aria-label={current.label} bind:offsetWidth={panelWidth} style:left={innerWidth > NARROW_PX ? `${left}px` : undefined}>
            {@render panel(current.id)}
            <div class="foot">
                <button type="button" class="quiet" disabled={!current.set} onclick={() => onreset(current.id)}>Reset</button>
                <button type="button" class="done" onclick={() => close(true)}>Done</button>
            </div>
        </div>
    {/if}
    {#each ghosts as ghost (ghost.id)}
        <span class="ghost" style:left="{ghost.from.left}px" style:top="{ghost.from.top}px" use:fly={ghost}>{ghost.label}</span>
    {/each}
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
    /* The panel of a slot, and the list of the field. */
    .search-bar :global(.search-panel) {
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
        .search-bar :global(.search-panel) { animation-name: fade; }
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
    .ghost {
        position: fixed;
        z-index: 1;
        padding: 3px 9px;
        border-radius: 6px;
        background: var(--parchment-3);
        color: var(--ink);
        font: 600 13px/1.4 var(--sans);
        white-space: nowrap;
        pointer-events: none;
    }
    /* The field takes the first row and the slots the second; the cross stays at the end of the field. */
    @media (max-width: 760px) {
        .strip {
            flex-wrap: wrap;
        }
        .slots {
            flex: 1 1 100%;
        }
        .slot {
            padding: 7px 10px;
        }
        .slot:first-child {
            border-left: 0;
        }
        .tail {
            position: absolute;
            top: 0;
            right: 0;
            height: 46px;
            border-left: 0;
        }
        strong {
            display: -webkit-box;
            white-space: normal;
            -webkit-box-orient: vertical;
            -webkit-line-clamp: 2;
            line-clamp: 2;
        }
        .search-bar :global(.search-panel) {
            right: 0;
            left: 0;
            min-width: 0;
        }
    }
</style>
