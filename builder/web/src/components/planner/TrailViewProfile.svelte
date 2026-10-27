<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { heightRange, profileHeightAt, profilePaths, type ProfileSample } from '../../lib/planner/profile-data';

    let { profile, colours, total, onHover }: {
        /** The height at each coordinate of the route. */
        profile: readonly ProfileSample[];
        /** The colour of each edge of the route. */
        colours: readonly string[];
        /** Route length in km. */
        total: number;
        onHover: (progress: number | null) => void;
    } = $props();

    const { low, high } = $derived(heightRange(profile));
    const { area } = $derived(profilePaths(profile, low, high));
    // The line in the colours of its edges: one path per colour.
    const lines = $derived.by(() => {
        const paths = new Map<string, string>();
        const y = (height: number) => 105 - (height - low) / (high - low) * 100;
        for (let i = 0; i + 1 < profile.length; i++) {
            const a = profile[i], b = profile[i + 1], colour = colours[i];
            if (a.height === null || b.height === null || !colour) continue;
            paths.set(colour, `${paths.get(colour) ?? ''}M${a.progress * 1000} ${y(a.height)}L${b.progress * 1000} ${y(b.height)}`);
        }
        return [...paths];
    });
    // The rider folds the profile down to a tab at the same place, and opens it from there.
    let open = $state(true);
    let hover = $state<number | null>(null);
    const height = $derived(hover === null ? null : profileHeightAt(hover, profile));

    function point(event: PointerEvent) {
        const box = (event.currentTarget as Element).getBoundingClientRect();
        hover = Math.max(0, Math.min(1, (event.clientX - box.left) / box.width));
        onHover(hover);
    }
    function leave() {
        hover = null;
        onHover(null);
    }
</script>

{#if open}
    <div class="profile">
        <div class="head">
            <p class="label">
                {#if hover === null}Hover to follow the route{:else}<b>{(hover * total).toFixed(1)} km</b>{height === null ? '' : ` · ${Math.round(height)} m`}{/if}
            </p>
            <button type="button" class="fold" aria-label="Hide the elevation profile" title="Hide the elevation profile" onclick={() => { leave(); open = false; }}><Icon name="down" size={16} /></button>
        </div>
        <svg viewBox="0 0 1000 110" preserveAspectRatio="none" role="img" aria-label="Elevation profile of the route"
            onpointermove={point} onpointerdown={point} onpointerleave={leave} onpointercancel={leave}>
            <path class="area" d={area} />
            {#each lines as [colour, d] (colour)}<path class="line" d={d} stroke={colour} />{/each}
            {#if hover !== null}<line class="cursor" x1={hover * 1000} x2={hover * 1000} y1="0" y2="110" />{/if}
        </svg>
    </div>
{:else}
    <button type="button" class="profile tab" aria-label="Show the elevation profile" onclick={() => open = true}><Icon name="mountain" size={16} />Profile<Icon name="up" size={14} /></button>
{/if}

<style>
    .profile {
        position: absolute;
        left: calc(max(12px, env(safe-area-inset-left)) + 232px);
        right: calc(max(12px, env(safe-area-inset-right)) + 44px);
        bottom: max(12px, env(safe-area-inset-bottom));
        max-width: 760px;
        margin: 0 auto;
        padding: 6px 10px 8px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: color-mix(in srgb, var(--panel) 88%, transparent);
        box-shadow: var(--planner-shadow);
    }
    .head {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        margin-bottom: 2px;
    }
    .label {
        margin: 0;
        color: var(--ink-soft);
        font: 12px var(--sans);
    }
    .label b {
        color: var(--ink);
    }
    button {
        display: flex;
        align-items: center;
        border: 0;
        background: none;
        color: var(--ink-soft);
        cursor: pointer;
    }
    button:hover {
        color: var(--ink);
    }
    button:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }
    .fold {
        margin: -4px -6px -4px 0;
        padding: 4px;
        border-radius: 6px;
    }
    .tab {
        width: fit-content;
        gap: 6px;
        height: 36px;
        padding: 0 12px;
        border: 1px solid var(--line-strong);
        color: var(--ink);
        font: 700 14px var(--sans);
    }
    svg {
        display: block;
        width: 100%;
        height: 72px;
        touch-action: none;
        cursor: crosshair;
    }
    .area {
        fill: color-mix(in srgb, var(--ink) 10%, transparent);
    }
    .line {
        fill: none;
        stroke-width: 2.5;
        stroke-linecap: round;
        vector-effect: non-scaling-stroke;
    }
    .cursor {
        stroke: var(--ink);
        stroke-width: 1.5;
        vector-effect: non-scaling-stroke;
    }
    @media (max-width: 640px) {
        .profile {
            left: max(12px, env(safe-area-inset-left));
            right: max(12px, env(safe-area-inset-right));
            bottom: calc(max(12px, env(safe-area-inset-bottom)) + 152px);
        }
    }
</style>
