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
        tag,
    }: {
        /** Tracks to draw, in order. `color: null` draws ink — the single-track tiles' look. */
        segments: ReadonlyArray<{ track: Thumb; color?: string | null }>;
        /** The viewBox; the element itself is fluid and letterboxes via preserveAspectRatio. */
        width?: number;
        height?: number;
        /** The box's `border-radius` — tiles round the top, the trip band rounds the left. */
        corners?: string;
        /** Overlaid top-right — the expiry/status tag. */
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
</script>

<div class="thumb" style:height="{height}px" style:border-radius={corners}>
    {#if drawn.length > 0}
        <svg viewBox="0 0 {width} {height}" preserveAspectRatio="xMidYMid meet" aria-hidden="true">
            {#each drawn as segment, i (i)}
                <path
                    d={segment.fit.d}
                    fill="none"
                    stroke={segment.color ?? "var(--ink)"}
                    stroke-width={segment.color ? 2.6 : 2.2}
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            {/each}
            {#if first}
                <circle cx={first.fit.start[0]} cy={first.fit.start[1]} r="4" fill={first.color ?? "var(--forest)"} />
            {/if}
            {#if last}
                <circle
                    cx={last.fit.end[0]}
                    cy={last.fit.end[1]}
                    r="4"
                    fill="none"
                    stroke={last.color ?? "var(--forest)"}
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
