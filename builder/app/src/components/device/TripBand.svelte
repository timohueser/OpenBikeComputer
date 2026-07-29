<!--
  One trip, as a full-width band above the route tiles: a combined multi-color track preview on
  the left (each stage in its own color, one shared projection so the stages join up), and on the
  right the trip's name, totals, ⋯ menu, and the stage rows — colored dot matching the drawn
  segment, per-stage facts, reorder and remove.

  The left preview is a real button: it opens the whole trip's preview (the page concatenates the
  same stage tracks that were drawn here). Each stage row's name is a button too, opening that
  stage's own preview. Presentation only — every cable operation is a callback from the page.
-->
<script lang="ts">
    import PopMenu from "./PopMenu.svelte";
    import TrackThumb from "./TrackThumb.svelte";
    import type { TripView } from "../../lib/device/dashboard.svelte";
    import { STAGE_COLORS, type Thumb } from "../../lib/device/thumbs.svelte";
    import { menuPick } from "../../lib/ui/menu";
    import type { RouteListEntry } from "../../lib/usb/objects";

    let {
        trip,
        stages,
        trackFor,
        busy = false,
        onopen,
        onopenstage,
        onrename,
        ondelete,
        onmovestage,
        onremovestage,
    }: {
        trip: TripView;
        /** The trip's stage list resolved against the route list; `route` null = dangling id. */
        stages: ReadonlyArray<{ id: number; route: RouteListEntry | null }>;
        /** The thumbnail track of a stage route, or null while it is still on its way. */
        trackFor: (routeId: number) => Thumb | null;
        /** True while a preview download holds the cable. */
        busy?: boolean;
        /** Preview the whole trip — the combined track. */
        onopen: () => void;
        /** Preview one stage. */
        onopenstage: (route: RouteListEntry) => void;
        onrename: (name: string) => void;
        ondelete: () => void;
        onmovestage: (index: number, delta: number) => void;
        onremovestage: (index: number) => void;
    } = $props();

    /** The stage's dot and segment color — cycled through the field-guide palette, by position. */
    const colorOf = (index: number): string => STAGE_COLORS[index % STAGE_COLORS.length];

    const segments = $derived(
        stages.flatMap((stage, index) => {
            const track = stage.route ? trackFor(stage.route.objectId) : null;
            return track ? [{ track, color: colorOf(index) }] : [];
        }),
    );

    /** The combined preview needs stages that still resolve to routes — not drawn thumbnails:
     *  the page fetches the tracks itself (from cache, mostly) when the preview opens. */
    const openable = $derived(stages.some((stage) => stage.route !== null));

    const name = $derived(trip.name || `Trip ${trip.objectId}`);

    /** The one name being edited inline. */
    let editing = $state<string | null>(null);

    function commitEdit() {
        const value = editing;
        editing = null;
        if (value === null || !value.trim()) return;
        if (value.trim() !== trip.name) onrename(value);
    }

    function onEditKey(event: KeyboardEvent) {
        if (event.key === "Enter") commitEdit();
        if (event.key === "Escape") editing = null;
    }

    function tripFacts(): string {
        const count = trip.detail?.stages.length ?? trip.stageCount;
        return [
            `${count} stage${count === 1 ? "" : "s"}`,
            `${(trip.totalDistanceM / 1000).toFixed(1)} km`,
            `${trip.totalAscentM.toLocaleString()} m up`,
        ].join(" · ");
    }

    function stageFacts(route: RouteListEntry): string {
        return `${(route.distanceM / 1000).toFixed(1)} km · ${route.ascentM.toLocaleString()} m`;
    }
</script>

<div class="tripband">
    <button
        type="button"
        class="preview"
        aria-label={`Preview the trip “${name}”`}
        disabled={busy || !openable}
        onclick={onopen}
    >
        <TrackThumb {segments} width={240} height={150} corners="13px 0 0 13px" fill>
            {#snippet tag()}
                <span class="tag">trip</span>
            {/snippet}
        </TrackThumb>
    </button>
    <div class="stages">
        <div class="bandhead">
            {#if editing !== null}
                <!-- svelte-ignore a11y_autofocus -->
                <input
                    class="rename"
                    type="text"
                    autofocus
                    bind:value={editing}
                    onblur={commitEdit}
                    onkeydown={onEditKey}
                />
            {:else}
                <p class="name">{name}</p>
            {/if}
            <p class="small faint grow">{tripFacts()}</p>
            <PopMenu label="Trip actions">
                <button type="button" onclick={(e) => menuPick(e, () => (editing = trip.name))}>Rename…</button>
                <button type="button" class="danger" onclick={(e) => menuPick(e, ondelete)}>Delete trip…</button>
            </PopMenu>
        </div>
        {#if trip.detail === null}
            <p class="small faint pad">The trip's stage list could not be read.</p>
        {:else}
            {#each stages as stage, index (`${stage.id}-${index}`)}
                <div class="stagerow">
                    <span class="dot" style:background={colorOf(index)}></span>
                    {#if stage.route}
                        {@const route = stage.route}
                        <button
                            type="button"
                            class="stagebtn"
                            disabled={busy}
                            onclick={() => onopenstage(route)}
                        >
                            <span class="nm">{route.name || `Route ${stage.id}`}</span>
                            <span class="small faint">{stageFacts(route)}</span>
                        </button>
                    {:else}
                        <span class="stagebtn dangling">
                            <span class="nm faint">Route {stage.id}</span>
                            <span class="small faint">no longer on the device</span>
                        </span>
                    {/if}
                    <span class="ops">
                        <button
                            type="button"
                            class="nudge"
                            disabled={index === 0}
                            aria-label="Move up"
                            onclick={() => onmovestage(index, -1)}>↑</button>
                        <button
                            type="button"
                            class="nudge"
                            disabled={index === stages.length - 1}
                            aria-label="Move down"
                            onclick={() => onmovestage(index, 1)}>↓</button>
                        <button
                            type="button"
                            class="nudge"
                            title="Remove from trip (the route stays on the device)"
                            aria-label="Remove from trip"
                            onclick={() => onremovestage(index)}>×</button>
                    </span>
                </div>
            {/each}
        {/if}
    </div>
</div>

<style>
    .tripband {
        display: grid;
        grid-template-columns: 240px 1fr;
        border: 1px solid var(--parchment-3);
        border-radius: 14px;
        background: linear-gradient(180deg, var(--parchment-2), var(--parchment));
        box-shadow: 0 1px 2px rgba(36, 51, 28, 0.06);
    }

    @media (max-width: 640px) {
        .tripband {
            grid-template-columns: 1fr;
        }
    }

    .preview {
        display: block;
        padding: 0;
        border: 0;
        border-right: 1px solid var(--line);
        background: none;
        cursor: pointer;
        text-align: left;
    }

    .preview:disabled {
        cursor: default;
    }

    .preview:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }

    .stages {
        padding: 10px 14px 12px;
        min-width: 0;
    }

    .bandhead {
        display: flex;
        align-items: baseline;
        gap: 10px;
        margin-bottom: 4px;
    }

    .bandhead p {
        margin: 0;
    }

    .grow {
        flex: 1;
        min-width: 0;
    }

    .name {
        font-family: var(--serif);
        font-size: 16.5px;
        font-weight: 600;
    }

    .rename {
        font-family: var(--serif);
        font-size: 16.5px;
        color: var(--ink);
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 7px;
        padding: 2px 8px;
        min-width: 200px;
    }

    .tag {
        font-size: 10px;
        letter-spacing: 0.05em;
        text-transform: uppercase;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        padding: 1px 8px;
        color: var(--ink-soft);
        white-space: nowrap;
        background: color-mix(in srgb, var(--panel) 80%, transparent);
    }

    .stagerow {
        display: flex;
        align-items: center;
        gap: 9px;
        padding: 5px 0;
    }

    .stagerow + .stagerow {
        border-top: 1px dashed var(--line);
    }

    .dot {
        width: 9px;
        height: 9px;
        border-radius: 50%;
        flex: none;
    }

    .stagebtn {
        display: flex;
        align-items: baseline;
        gap: 9px;
        flex: 1;
        min-width: 0;
        padding: 2px 0;
        border: 0;
        background: none;
        text-align: left;
        color: var(--ink);
        cursor: pointer;
    }

    .stagebtn:disabled,
    .stagebtn.dangling {
        cursor: default;
    }

    .stagebtn:hover:not(:disabled):not(.dangling) .nm {
        color: var(--forest-deep);
    }

    .stagebtn:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
        border-radius: 4px;
    }

    .nm {
        font-family: var(--serif);
        font-size: 14px;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .nm.faint {
        color: var(--ink-faint);
    }

    .pad {
        margin: 0;
        padding: 8px 0;
    }

    .ops {
        display: flex;
        gap: 1px;
        flex: none;
    }

    .nudge {
        border: 0;
        background: none;
        padding: 0 4px;
        font-size: 12px;
        line-height: 1.4;
        color: var(--ink-faint);
        cursor: pointer;
    }

    .nudge:hover:not(:disabled) {
        color: var(--ink);
    }

    .nudge:disabled {
        opacity: 0.3;
        cursor: default;
    }
</style>
