<script lang="ts" module>
    /** What the part under the pointer does, and its key. */
    export type Hint = { text: string; key?: string };
</script>

<script lang="ts">
    import { onMount, type Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';

    /** The frame of a map popup: a palette on the panel colour of the theme, with a close button, Escape, and the focus, which it takes on mount
     * unless `takeFocus` is false and gives back to the opener on close. It never takes the focus from a control that
     * the rider chose before its first frame. Its style gives the parts of each popup one look: a `header` with a
     * `.glyph`, the title and a `.meta` line, `.facts`, `.subs` of second actions, and a `.dock` at the bottom with the
     * `.hint` line and the `.tiles`. The part marked `data-aim` is straight over the point of the popup. A part with
     * `data-hint` gives its text to `children` while the pointer or the focus is on it, and one with `data-key` takes
     * that key; Enter takes the `.go` tile. */
    let { label, onClose, takeFocus = true, children }: {
        label: string;
        onClose: () => void;
        takeFocus?: boolean;
        children: Snippet<[Hint | null]>;
    } = $props();

    let root: HTMLDivElement;
    let opener: HTMLElement | null = null;
    let hint = $state<Hint | null>(null);

    onMount(() => {
        opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        // The map moves this content into its popup after mounting.
        // MapLibre can focus the close button as it adds the popup.
        const frame = requestAnimationFrame(() => {
            if (!takeFocus) return;
            const at = document.activeElement;
            if (at === opener || at === document.body || root.contains(at)) root.focus({ preventScroll: true });
        });
        return () => cancelAnimationFrame(frame);
    });

    function close() {
        onClose();
        if (opener?.isConnected) opener.focus({ preventScroll: true });
    }

    function key(event: KeyboardEvent) {
        const target = event.target as HTMLElement;
        if (target.closest('input, select') || event.ctrlKey || event.metaKey || event.altKey) return;
        if (event.key === 'Escape') {
            event.stopPropagation();
            close();
            return;
        }
        // Enter on a button is the click of that button.
        if (event.key === 'Enter' && target !== root) return;
        const name = event.key === 'Backspace' ? 'delete' : event.key.toLowerCase();
        const part = name === 'enter' ? root.querySelector<HTMLElement>('.go:not(:disabled)')
            : [...root.querySelectorAll<HTMLElement>('[data-key]:not(:disabled)')].find(part => part.dataset.key!.toLowerCase() === name);
        if (!part) return;
        event.preventDefault();
        part.click();
    }

    function aim(event: Event) {
        const part = (event.target as HTMLElement).closest<HTMLElement>('[data-hint]');
        hint = part && root.contains(part) ? { text: part.dataset.hint!, key: part.dataset.key } : null;
    }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="callout" bind:this={root} tabindex="-1" role="dialog" aria-label={label} onkeydown={key}
    onpointerover={aim} onfocusin={aim} onpointerleave={() => hint = null} onfocusout={() => hint = null}>
    <button type="button" class="close" onclick={close} aria-label="Close"><Icon name="close" size={14} /></button>
    {@render children(hint)}
</div>

<style>
    /* The map sets the free room; the content scrolls as a last resort. */
    .callout {
        position: relative;
        display: flex;
        flex-direction: column;
        gap: 8px;
        width: 320px;
        max-width: var(--callout-width);
        max-height: var(--callout-room);
        overflow-y: auto;
        padding: 10px 10px 0;
        border-radius: 14px;
        color: var(--ink);
        font: 400 13px/1.4 var(--sans);
        outline: none;
        /* The ground of a tile, of the track of a choice, of the chosen tile in it, and of a small button in a track. */
        --tile: var(--parchment-2);
        --tile-over: var(--parchment-3);
        --track: var(--parchment-2);
        --lit: var(--panel);
        --on-lit: var(--ink);
        --lit-ring: var(--line-strong);
        --knob: var(--panel);
    }
    /* On the dark panel the greens of the page are too near it: the tiles are lighter than the panel, the track is
       darker, and the chosen tile is the light one. */
    :global(:root[data-theme="dark"]) .callout {
        --tile: rgb(255 255 255 / .09);
        --tile-over: rgb(255 255 255 / .16);
        --track: rgb(0 0 0 / .28);
        --lit: var(--ink);
        --on-lit: var(--panel);
        --lit-ring: transparent;
        --knob: var(--tile);
    }
    .callout > :global(*) {
        flex: none;
    }
    .callout :global(button),
    .callout :global(a.sub) {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        text-decoration: none;
        cursor: pointer;
    }
    .callout :global(button:disabled) {
        cursor: default;
    }
    .close {
        position: absolute;
        top: 10px;
        right: 10px;
        z-index: 2;
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        border-radius: 7px;
        color: var(--ink-soft);
    }
    .close:hover {
        background: var(--tile);
        color: var(--ink);
    }

    /* The symbol of the thing, its title clear of the close button, and one line under it. */
    .callout :global(header) {
        display: flex;
        align-items: center;
        gap: 10px;
        min-width: 0;
        padding: 2px 30px 2px 4px;
    }
    .callout :global(header > div) {
        flex: 1;
        min-width: 0;
    }
    .callout :global(.glyph) {
        display: grid;
        place-items: center;
        flex: none;
        width: 30px;
        height: 30px;
        border-radius: 50%;
        background: var(--tile);
        font: 700 13px var(--sans);
    }
    /* The symbol in the colour of its thing on the map: a place, a warning, the route. */
    .callout :global(.glyph.place) {
        background: var(--place-disc);
        color: #fff;
    }
    .callout :global(.glyph.warn) {
        background: var(--coral);
        color: var(--panel);
    }
    .callout :global(.glyph.on-route) {
        background: var(--route);
        color: #fff;
    }
    .callout :global(h2) {
        margin: 0;
        font: 600 15px/1.25 var(--sans);
        overflow-wrap: anywhere;
    }
    .callout :global(p) {
        margin: 0;
    }
    .callout :global(.meta) {
        color: var(--ink-soft);
        font-size: 12px;
        font-variant-numeric: tabular-nums;
    }
    .callout > :global(p),
    .callout > :global(section) {
        padding: 0 4px;
    }
    .callout :global(a) {
        color: var(--ink);
        text-underline-offset: 3px;
    }
    .callout :global(a:hover) {
        color: var(--link);
    }

    /* A tile is an action: a symbol over one word. The amber tile is the usual one. */
    .callout :global(.tiles) {
        display: grid;
        grid-template-columns: repeat(var(--n, 4), minmax(0, 1fr));
        gap: 6px;
    }
    .callout :global(.tile) {
        display: grid;
        justify-items: center;
        align-content: center;
        gap: 5px;
        min-height: 66px;
        padding: 10px 2px 9px;
        border-radius: 10px;
        background: var(--tile);
        font: 600 12px/1.2 var(--sans);
        text-align: center;
    }
    .callout :global(.tile:hover:not(:disabled)) {
        background: var(--tile-over);
    }
    .callout :global(.tile.go) {
        background: var(--amber);
        color: var(--on-amber);
    }
    .callout :global(.tile.go:hover:not(:disabled)) {
        background: color-mix(in srgb, var(--amber) 88%, #fff);
    }
    .callout :global(.tile.danger) {
        background: color-mix(in srgb, var(--coral) 16%, transparent);
        color: var(--coral);
    }
    .callout :global(.tile.danger:hover) {
        background: color-mix(in srgb, var(--coral) 28%, transparent);
    }
    .callout :global(.tile:disabled) {
        opacity: .4;
    }
    /* A choice of one: the tiles share a track, and the chosen one is lit. */
    .callout :global(.track) {
        display: grid;
        grid-template-columns: repeat(var(--n, 4), minmax(0, 1fr));
        gap: 2px;
        padding: 3px;
        border-radius: 12px;
        background: var(--track);
    }
    .callout :global(.track .tile) {
        min-height: 60px;
        border-radius: 9px;
        background: none;
        color: var(--ink-soft);
    }
    .callout :global(.track .tile:hover) {
        background: none;
        color: var(--ink);
    }
    .callout :global(.track .tile[aria-checked="true"]) {
        background: var(--lit);
        color: var(--on-lit);
        box-shadow: inset 0 0 0 1px var(--lit-ring);
    }
    /* The track and, at the far end, the tile that removes. */
    .callout :global(.split) {
        display: grid;
        grid-template-columns: minmax(0, 1fr) 66px;
        align-items: center;
        gap: 6px;
    }

    /* The line over the tiles explains the part under the pointer; its key is at the end of the line. */
    .callout :global(.hint) {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 20px;
        padding: 0 4px;
        color: var(--ink-soft);
        font-size: 12px;
    }
    .callout :global(kbd) {
        display: inline-grid;
        place-items: center;
        min-width: 18px;
        height: 18px;
        margin-left: auto;
        padding: 0 4px;
        border-radius: 5px;
        box-shadow: inset 0 0 0 1px var(--line), inset 0 -2px 0 var(--line);
        color: var(--ink-soft);
        font: 600 10px var(--mono);
    }
    /* The usual actions stay in view at the bottom while a long popup scrolls. */
    .callout :global(.dock) {
        position: sticky;
        bottom: 0;
        display: grid;
        gap: 8px;
        /* Its ground starts over the hint, so the text that scrolls under it ends clear of the line. */
        margin: -8px -10px 0;
        padding: 8px 10px 10px;
        background: var(--panel);
    }

    /* A second action is a row as high as a button. */
    .callout :global(.subs) {
        display: grid;
        grid-template-columns: repeat(var(--n, 2), minmax(0, 1fr));
        gap: 6px;
    }
    .callout :global(.sub) {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 34px;
        padding: 0 10px;
        border-radius: 9px;
        box-shadow: inset 0 0 0 1px var(--line);
        font: 600 12px var(--sans);
        text-align: left;
    }
    .callout :global(.sub:hover) {
        background: var(--tile);
    }
    .callout :global(.sub svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .callout :global(.sub span + svg) {
        margin-left: auto;
    }

    /* An entry of a list: a symbol, a name, and a note at the right. */
    .callout :global(.rows) {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        gap: 2px;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .callout :global(.row) {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        min-height: 42px;
        padding: 0 10px;
        border-radius: 10px;
        background: var(--tile);
        font: 600 13px var(--sans);
        text-align: left;
    }
    .callout :global(.row:hover) {
        background: var(--tile-over);
    }
    .callout :global(.row svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .callout :global(.row span),
    .callout :global(.row small) {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .callout :global(.row small) {
        flex-shrink: 100;
        margin-left: auto;
        color: var(--ink-soft);
        font: 400 12px var(--sans);
    }

    /* Facts with a symbol each, one to a line. */
    .callout :global(.facts) {
        display: grid;
        gap: 6px;
        margin: 0;
        padding: 0 4px;
        list-style: none;
        font-variant-numeric: tabular-nums;
    }
    .callout :global(.fact) {
        display: flex;
        align-items: center;
        gap: 8px;
        min-width: 0;
    }
    .callout :global(.fact > svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .callout :global(.fact > a),
    .callout :global(.fact > span) {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    /* The last part holds the room under it: the dock fills that room, and a negative margin would move it as it sticks.
       After the rules of the parts, so that it sets their margin. */
    .callout > :global(:last-child) {
        margin-bottom: 10px;
    }
    .callout > :global(.dock:last-child) {
        margin-bottom: 0;
    }
    .callout :global(:focus-visible) {
        outline: 2px solid var(--amber);
        outline-offset: 2px;
    }
    .callout :global(.tile:focus-visible),
    .callout :global(.row:focus-visible),
    .callout :global(.sub:focus-visible) {
        outline-offset: -2px;
    }
    .callout :global(.tile.go:focus-visible) {
        outline-color: var(--ink);
    }
</style>
