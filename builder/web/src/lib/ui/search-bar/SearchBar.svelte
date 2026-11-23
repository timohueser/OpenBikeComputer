<!-- A search bar: a text field, then fixed slots. Each slot shows its values as words and opens one panel under it; the
     list of the text field is a panel too, and one panel is open at a time. Enter in the field sends the words of each
     value to its slot. The cross empties the search, and "Undo" brings it back for a short time. The bar lays out by
     its own width: a narrow bar has the field in one row and the slots in the next. -->
<script lang="ts" module>
    /** `words` are the values of the slot in their order; `unset` shows when it has none. `set` says that a value differs
     * from its default, also one with no word, so "Reset" has work. A slot with `off` is disabled and shows `off`. */
    export interface Slot { id: string; icon: string; label: string; words: string[]; unset: string; set: boolean; off?: string }
</script>

<script lang="ts" generics="V extends Value, P extends Place">
    import { untrack, type Snippet } from 'svelte';
    import Icon from '../Icon.svelte';
    import Field, { type FieldProps, type Place } from './Field.svelte';
    import { drop, rise, still } from './motion';
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
    let bar = $state<HTMLElement>(), panelWidth = $state(0), barWidth = $state(0);
    /** The width of a narrow bar, whose panels are as wide as the bar; the CSS of the two rows has it too. */
    const NARROW_PX = 720;
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
    /** The cross. A page calls it for a control of its own that clears the search, so that control has "Undo" too. */
    export function clear() {
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

    /** The words of a value that fly from the field to their slot, one after the other. `left` and `top` are in the bar. */
    let ghosts = $state<{ id: number; slot: string; label: string; left: number; top: number; from: DOMRect; to: DOMRect; delay: number }[]>([]), flights = 0;
    const FLY_MS = 360;
    /** The words of a value fly from `from` to its slot, which then flashes. With reduced motion, only the slot flashes. */
    function land(value: Value, from: DOMRect | undefined, delay: number) {
        const to = button(value.slot)?.querySelector('strong')?.getBoundingClientRect(), box = bar?.getBoundingClientRect();
        if (!from || !to || !box || still()) return flash(value.slot);
        ghosts.push({ id: flights++, slot: value.slot, label: value.label, left: from.left - box.left, top: from.top - box.top, from, to, delay });
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
<svelte:window onkeydowncapture={escape} onpointerdown={event => { if (open && !bar?.contains(event.target as Node)) close(false); }} />

<div class="search-bar" role="search" aria-label={label} bind:this={bar} bind:clientWidth={barWidth}>
    <div class="strip">
        <Field {...field} bind:text={typed} bind:listing {slots} {land} />
        <div class="slots">
            {#each slots as slot (slot.id)}
                <button type="button" class="slot" class:unset={!slot.words.length || !!slot.off} data-slot={slot.id} disabled={!!slot.off}
                    aria-expanded={open === slot.id} aria-controls="search-panel" onclick={() => open = open === slot.id ? undefined : slot.id}>
                    <span class="label"><Icon name={slot.icon} size={14} />{slot.label}</span>
                    <!-- A slot that gets or loses a value shows it; a value that only changes, as under a slider, stays still. -->
                    {#key slot.words.length}<strong in:rise>{slot.off ?? (text(slot.words) || slot.unset)}</strong>{/key}
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
        <div class="search-panel" id="search-panel" role="group" aria-label={current.label} bind:offsetWidth={panelWidth} style:left={barWidth > NARROW_PX ? `${left}px` : undefined}
            in:drop out:drop={{ leave: true }}>
            {@render panel(current.id)}
            <div class="foot">
                <button type="button" class="quiet" disabled={!current.set} onclick={() => onreset(current.id)}>Reset</button>
                <button type="button" class="done" onclick={() => close(true)}>Done</button>
            </div>
        </div>
    {/if}
    {#each ghosts as ghost (ghost.id)}
        <span class="ghost" style:left="{ghost.left}px" style:top="{ghost.top}px" use:fly={ghost}>{ghost.label}</span>
    {/each}
</div>

<style>
    /* A query container also contains the layout in some browsers, so the ghosts are placed in the bar, not in the
       window. */
    .search-bar {
        position: relative;
        z-index: 5;
        container-type: inline-size;
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
        position: relative;
        min-width: 0;
        padding: 8px 14px;
        border: 0;
        border-left: 1px solid var(--line);
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
        transition: background 120ms;
    }
    /* The cross and "Undo" have their place after the last slot, as wide as the cell of "Undo", so no slot moves when
       they show. Until then, the colour of the last slot fills that place and a click there is a click on the slot. */
    .slot:last-child {
        margin-right: 76px;
    }
    .slot:last-child::after {
        content: "";
        position: absolute;
        top: 0;
        bottom: 0;
        left: 100%;
        width: 76px;
        background: inherit;
        transition: inherit;
    }
    .slot:hover:not(:disabled) {
        background: var(--parchment);
    }
    .slot:active:not(:disabled), .slot[aria-expanded="true"]:not(:disabled) {
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
        position: absolute;
        top: 1px;
        right: 1px;
        bottom: 1px;
        display: flex;
        align-items: center;
        padding: 0 8px;
        border-left: 1px solid var(--line);
        border-radius: 0 9px 9px 0;
        background: var(--panel);
        animation: appear 140ms ease-out;
    }
    @keyframes appear {
        from { opacity: 0; }
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
        transition: background 120ms, color 120ms, transform 140ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .clear:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .clear:active, .undo:active, .done:active {
        transform: scale(0.96);
    }
    .undo, .done {
        min-height: 30px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 13px var(--sans);
        transition: background 120ms, border-color 120ms, transform 140ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .undo:hover, .done:hover {
        border-color: var(--ink-faint);
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
        transition: background 120ms, color 120ms;
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
        position: absolute;
        z-index: 1;
        padding: 3px 9px;
        border-radius: 6px;
        background: var(--parchment-3);
        color: var(--ink);
        font: 600 13px/1.4 var(--sans);
        white-space: nowrap;
        pointer-events: none;
    }
    @media (prefers-reduced-motion: reduce) {
        .clear, .undo, .done {
            transition-property: background, border-color, color;
        }
    }
    /* The field takes the first row and the slots the second; the cross stays at the end of the field. */
    @container (max-width: 720px) {
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
        .slot:last-child {
            margin-right: 0;
        }
        .slot:last-child::after {
            content: none;
        }
        .tail {
            bottom: auto;
            height: 46px;
            border-left: 0;
            background: none;
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
