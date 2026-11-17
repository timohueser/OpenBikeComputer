<!-- A row of a list of routes: a picture, the name, a line of numbers, and a badge. The row shows its picture only
     while it is near the view, so a long list draws few pictures. -->
<script module lang="ts">
    const rows = new WeakMap<Element, (near: boolean) => void>();
    let observer: IntersectionObserver | undefined;
</script>

<script lang="ts">
    import type { Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';

    let { name, icon, current, onclick, lead = '', parts, tail, badge, room, picture }: {
        name: string;
        /** The name of an icon of `Icon.svelte`, before the name. */
        icon?: string;
        current: boolean;
        onclick: () => void;
        /** The second line, written when `parts` is given: `lead` (such as a date), each part in its own span, so the
         * line wraps between two parts and never inside one, then `tail` as plain text. */
        lead?: string;
        parts?: (string | false)[];
        tail?: string;
        badge?: { text: string; kind: string };
        /** The text of a button that lies on the row. The row keeps its room with the same text, hidden, where a badge
         * would be. */
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

<button type="button" class="row" aria-current={current} {onclick} use:watch>
    <span class="thumb">{#if near}{@render picture()}{/if}</span>
    <span class="text">
        <strong>{#if icon}<span class="icon"><Icon name={icon} size={16} /></span>{/if}{name}</strong>
        {#if parts}
            <span class="meta">{lead}{#each parts.filter(Boolean) as part, i (i)}{i || lead ? ' · ' : ''}<span>{part}</span>{/each}{#if tail}{' · '}{tail}{/if}</span>
        {/if}
    </span>
    {#if badge}<span class="badge {badge.kind}">{badge.text}</span>{:else if room}<span class="room" aria-hidden="true">{room}</span>{/if}
</button>

<style>
    .row {
        display: grid;
        grid-template-columns: 150px minmax(0, 1fr) auto;
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
    .row:hover {
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
    /* The numbers are one line where they fit and wrap where they do not: none is cut. */
    .meta, .badge, .room {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .badge, .meta span, .room {
        white-space: nowrap;
    }
    /* On the line of the name. */
    .badge {
        padding-top: 8px;
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
    /* The padding of the button that lies on the row. */
    .room {
        padding: 2px 4px;
        visibility: hidden;
    }
    @media (max-width: 760px) {
        .row {
            grid-template-columns: 120px minmax(0, 1fr) auto;
            gap: 10px;
        }
        .thumb {
            height: 80px;
        }
    }
</style>
