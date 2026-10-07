<!--
  A track thumbnail: one or more `[lat, lon]` tracks fitted into a panel-colored box, with a start
  dot (filled) and an end dot (outlined) and an optional tag overlaid top-right.

  Pure drawing — the points come from `deviceThumbs` (or anywhere), the fitting from `fitTracks`,
  and multi-segment input shares one projection so a trip's stages join up. An empty segment list
  renders the empty box: tiles appear immediately and fill in when their track lands.
-->
<script lang="ts">
    import { fitTracks } from "../../lib/device/library";
    import type { Thumb } from "../../lib/device/thumbs.svelte";

    let {
        segments,
        width = 300,
        height = 116,
        corners = "13px 13px 0 0",
        fill = false,
        tag,
    }: {
        /** Tracks to draw, in order — each in its caller's color (routes coral, rides forest,
         *  trip stages the stage palette). `color: null` falls back to ink. */
        segments: ReadonlyArray<{ track: Thumb; color?: string | null }>;
        /** The viewBox; the element itself is fluid and letterboxes via preserveAspectRatio. */
        width?: number;
        height?: number;
        /** The box's `border-radius` — tiles round the top, the trip band rounds the left. */
        corners?: string;
        /** Fill the parent instead of owning a fixed height: the box stretches to the whole
         *  cell (its `--panel` background edge-to-edge, `height` demoted to a minimum) and the
         *  track letterboxes inside it. The trip band's left cell, whose height the stage rows
         *  set; tiles keep the default fixed-height look. */
        fill?: boolean;
        /** Overlaid top-right — the caller's status tag ("trip", "recording", "in library"). */
        tag?: import("svelte").Snippet;
    } = $props();

    const fitted = $derived(
        fitTracks(
            segments.map((s) => s.track),
            width,
            height,
            10,
        ),
    );
    const drawn = $derived(
        segments
            .map((segment, i) => ({ fit: fitted[i], color: segment.color ?? null }))
            .filter((s): s is { fit: NonNullable<(typeof fitted)[number]>; color: string | null } => s.fit !== null),
    );
    const first = $derived(drawn[0] ?? null);
    const last = $derived(drawn[drawn.length - 1] ?? null);
    /** A trip band draws each stage a touch heavier; a single track stays fine-lined. */
    const multi = $derived(drawn.length > 1);
    /** Single-track dots are ink so they read against a coral or forest line on the panel
     *  background; a trip's dots keep their stage colors — the band's rows key off them. */
    const dotColor = (segment: { color: string | null } | null): string =>
        (multi ? segment?.color : null) ?? "var(--ink)";
</script>

<div
    class="thumb"
    class:fill
    style:height={fill ? undefined : `${height}px`}
    style:min-height={fill ? `${height}px` : undefined}
    style:border-radius={corners}
>
    {#if drawn.length > 0}
        <svg viewBox="0 0 {width} {height}" preserveAspectRatio="xMidYMid meet" aria-hidden="true">
            {#each drawn as segment, i (i)}
                <path
                    d={segment.fit.d}
                    fill="none"
                    stroke={segment.color ?? "var(--ink)"}
                    stroke-width={multi ? 2.6 : 2.2}
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            {/each}
            {#if first}
                <circle cx={first.fit.start[0]} cy={first.fit.start[1]} r="4" fill={dotColor(first)} />
            {/if}
            {#if last}
                <circle
                    cx={last.fit.end[0]}
                    cy={last.fit.end[1]}
                    r="4"
                    fill="none"
                    stroke={dotColor(last)}
                    stroke-width="2"
                />
            {/if}
        </svg>
    {/if}
    {#if tag}
        <span class="corner">{@render tag()}</span>
    {/if}
</div>

<style>
    .thumb {
        position: relative;
        background: var(--panel);
        border-bottom: 1px solid var(--line);
        overflow: hidden;
    }

    /* Filling the cell: the height is the parent's, and the tiles' bottom hairline would sit
       above the band's own border — the band draws its own seam (border-right). */
    .thumb.fill {
        height: 100%;
        border-bottom: 0;
    }

    .thumb svg {
        display: block;
        width: 100%;
        height: 100%;
    }

    .corner {
        position: absolute;
        top: 8px;
        right: 8px;
        display: inline-flex;
    }
</style>
