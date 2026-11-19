<!-- A row of a list of routes: a picture, the name, a line of numbers, and a badge. The row shows its picture only
     while it is near the view, so a long list draws few pictures. -->
<script module lang="ts">
    const rows = new WeakMap<Element, (near: boolean) => void>();
    let observer: IntersectionObserver | undefined;
</script>

<script lang="ts">
    import type { Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';

    let { name, icon, current, line, onclick, lead = '', parts, tail, badge, room, picture }: {
        name: string;
        /** The name of an icon of `Icon.svelte`, before the name. */
        icon?: string;
        current: boolean;
        /** The id of the lead line of the row on the map: the row and its lines light together. */
        line?: string;
        onclick: () => void;
        /** The second line, written when `parts` is given: `lead` (such as a date), each part in its own span, so the
         * line wraps between two parts and never inside one, then `tail` as plain text. */
        lead?: string;
        parts?: (string | false)[];
        tail?: string;
        badge?: { text: string; kind: string };
        /** The text of a button that lies on the top right of the row. The row keeps its room with the same text,
         * hidden, so the name does not run under it. */
        room?: string;
        /** A cover, whose root has the class `cover`. */
        picture: Snippet;
    } = $props();

    let near = $state(false);
    function watch(node: Element) {
        observer ??= new IntersectionObserver(entries => {
            for (const { target, isIntersecting } of entries) rows.get(target)?.(isIntersecting);
        }, { rootMargin: '200px' });
        rows.set(node, value => near = value);
        observer.observe(node);
        return { destroy: () => observer!.unobserve(node) };
    }
</script>

<button type="button" class="row" aria-current={current} data-line={line} {onclick} use:watch>
    <span class="thumb">{#if near}{@render picture()}{/if}</span>
    <span class="text">
        <strong>{#if icon}<span class="icon"><Icon name={icon} size={16} /></span>{/if}{name}</strong>
        {#if parts}
            <span class="meta">{lead}{#each parts.filter(Boolean) as part, i (i)}{i || lead ? ' · ' : ''}<span>{part}</span>{/each}{#if tail}{' · '}{tail}{/if}</span>
        {/if}
        {#if badge}<span class="badge {badge.kind}">{badge.text}</span>{/if}
    </span>{#if room}<span class="room" aria-hidden="true">{room}</span>{/if}
</button>

<style>
    .row {
        display: grid;
        grid-template-columns: 150px minmax(0, 1fr);
        align-items: start;
        gap: 14px;
        width: 100%;
        padding: 7px;
        border: 0;
        border-radius: 8px;
        background: none;
        color: var(--ink);
        text-align: left;
    }
    /* Inside the row: the scroll box of a list cuts a ring around it. */
    .row:focus-visible {
        outline-offset: -2px;
    }
    /* `data-hover`: the pointer is on a line of the row on the map, or a key focused the row. The frame sets it. */
    .row:hover, .row:global([data-hover]) {
        background: var(--parchment);
    }
    .row[aria-current="true"] {
        background: var(--parchment-2);
    }
    .thumb {
        height: 100px;
        overflow: hidden;
        border-radius: 6px;
        background: var(--parchment-2);
    }
    .thumb :global(.cover) {
        background: none;
    }
    .text {
        display: flex;
        flex-direction: column;
        gap: 3px;
        min-width: 0;
        padding-top: 6px;
    }
    .icon {
        display: inline-flex;
        margin-right: 5px;
        vertical-align: -2px;
    }
    /* A long name has two lines. */
    strong {
        display: -webkit-box;
        overflow: hidden;
        font-weight: 600;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
        line-clamp: 2;
    }
    /* The numbers are one line where they fit and wrap where they do not: none is cut. */
    .meta, .badge, .room {
        color: var(--ink-soft);
        font-size: 12.5px;
        font-variant-numeric: tabular-nums;
    }
    .badge, .meta span, .room {
        white-space: nowrap;
    }
    .badge {
        align-self: start;
    }
    /* The one state that waits for the rider to connect the device has a dot. */
    .badge.waiting::before {
        display: inline-block;
        width: 6px;
        height: 6px;
        margin-right: 5px;
        border-radius: 50%;
        background: var(--amber);
        vertical-align: 1px;
        content: '';
    }
    /* The padding and the place of the button that lies on the row. */
    .room {
        grid-column: 3;
        padding: 2px 4px;
        visibility: hidden;
    }
    @media (max-width: 760px) {
        .row {
            grid-template-columns: 120px minmax(0, 1fr);
            gap: 10px;
        }
        .thumb {
            height: 80px;
        }
    }
</style>
