<!--
  One gallery tile: the tile *is* the button — click anywhere (or Enter/Space) opens the object's
  preview, and the only inner controls are small icon buttons and the ⋯ menu.

  A `role="button"` div rather than a `<button>`, deliberately: the meta row holds real buttons
  and, mid-rename, an input, and interactive content inside a button element is invalid HTML that
  screen readers flatten. The activation guard ignores any click or key that came from an
  interactive descendant, so inner controls work without each one remembering to stopPropagation
  (most do anyway, to keep the ⋯ menu's close behaviour local).
-->
<script lang="ts">
    let {
        label,
        disabled = false,
        onopen,
        thumb,
        children,
    }: {
        /** The accessible name — "Preview 'Kaiserstuhl loop'". */
        label: string;
        /** True while another preview download holds the cable. */
        disabled?: boolean;
        onopen: () => void;
        /** The thumbnail area — a TrackThumb, typically with its tag overlaid. */
        thumb?: import("svelte").Snippet;
        /** The meta row under the thumbnail: name, facts, icon buttons. */
        children?: import("svelte").Snippet;
    } = $props();

    /** True where the event's real target is an inner control the tile must not speak over. */
    function fromInnerControl(event: Event): boolean {
        return (
            event.target instanceof Element &&
            event.target.closest("button, a, input, select, textarea, summary, details") !== null
        );
    }

    function activate(event: MouseEvent) {
        if (disabled || fromInnerControl(event)) return;
        onopen();
    }

    function onKeydown(event: KeyboardEvent) {
        if (event.key !== "Enter" && event.key !== " ") return;
        if (disabled || fromInnerControl(event)) return;
        event.preventDefault();
        onopen();
    }
</script>

<div
    class="tile"
    role="button"
    tabindex="0"
    aria-label={label}
    aria-disabled={disabled}
    onclick={activate}
    onkeydown={onKeydown}
>
    {@render thumb?.()}
    <div class="meta">
        {@render children?.()}
    </div>
</div>

<style>
    .tile {
        position: relative;
        background: linear-gradient(180deg, var(--parchment-2), var(--parchment));
        border: 1px solid var(--parchment-3);
        border-radius: 14px;
        /* No overflow clip: the ⋯ menu's popover must escape the tile. The thumbnail rounds its
           own top corners instead (TrackThumb's `corners`). */
        cursor: pointer;
        box-shadow: 0 1px 2px rgba(36, 51, 28, 0.06);
        transition:
            transform 0.1s,
            box-shadow 0.1s,
            border-color 0.1s;
        text-align: left;
    }

    .tile:hover {
        transform: translateY(-2px);
        box-shadow: 0 8px 20px rgba(36, 51, 28, 0.14);
        border-color: var(--wood);
    }

    /* The hover transform makes the tile a stacking context, which would cap the ⋯ menu's
       z-index under later tiles' thumbnails — so the whole tile rises while it is hovered or
       holds focus (an open menu keeps focus inside). */
    .tile:hover,
    .tile:focus-within {
        z-index: 5;
    }

    .tile:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }

    .tile[aria-disabled="true"] {
        cursor: default;
    }

    .meta {
        padding: 9px 12px 11px;
        display: flex;
        align-items: flex-start;
        gap: 8px;
    }
</style>
