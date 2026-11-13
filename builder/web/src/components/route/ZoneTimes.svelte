<script lang="ts">
    import { duration } from '../../lib/planner/editor';
    import type { Channel } from '../../lib/ride/numbers';
    import { ROUTES } from '../../lib/routes';

    /** The time in each zone of a sensor of a ride: one bar of five parts, or with `list` one row for each zone, the
     * hardest first, with its bar and its time. A sensor with no limit shows the link to the Settings. */
    let { channel, unit, missing, list = false }: {
        channel: Channel;
        /** The unit of the zones: "bpm" or "W". */
        unit: string;
        /** The limit that the zones need, such as "maximum heart rate". */
        missing: string;
        list?: boolean;
    } = $props();

    const hours = (seconds: number) => duration(seconds / 3600);
    const zones = $derived((channel.zones ?? []).map(({ zone, seconds }, i) => ({
        name: zone.name, seconds, colour: `var(--zone-${i + 1})`,
        text: `${zone.name} · ${Number.isFinite(zone.to) ? `${zone.from}–${zone.to}` : `${zone.from}+`} ${unit} · ${hours(seconds)}`,
    })));
    const longest = $derived(Math.max(1, ...zones.map(zone => zone.seconds)));
</script>

{#if !channel.zones}
    <p class="note">Set your {missing} in <a href={ROUTES.settings}>Settings</a> to see zones.</p>
{:else if list}
    <ul>
        {#each [...zones].reverse() as zone (zone.name)}
            <li title={zone.text}><span>{zone.name}</span><i style:width={`${zone.seconds / longest * 100}%`} style:background={zone.colour}></i><small>{hours(zone.seconds)}</small></li>
        {/each}
    </ul>
{:else}
    <div class="bar" role="img" aria-label={zones.map(zone => zone.text).join(', ')}>
        {#each zones as zone (zone.name)}<span style:flex-grow={zone.seconds} style:background={zone.colour} title={zone.text}></span>{/each}
    </div>
{/if}

<style>
    .note {
        margin: 2px 0 0;
        color: var(--ink-soft);
        font-size: 12.5px;
        text-align: left;
    }
    .note a {
        color: inherit;
    }
    .bar {
        display: flex;
        height: 8px;
        overflow: hidden;
        border-radius: 3px;
        background: var(--parchment-2);
    }
    .bar span {
        flex-basis: 0;
        min-width: 0;
    }
    ul {
        display: grid;
        gap: 12px;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: grid;
        grid-template-columns: 22px minmax(0, 1fr) 54px;
        gap: 8px;
        align-items: center;
        font-size: 13px;
    }
    /* A zone with no time keeps a mark of its colour. */
    i {
        min-width: 2px;
        height: 10px;
        border-radius: 3px;
    }
    small {
        color: var(--ink-soft);
        font-size: 12.5px;
        font-variant-numeric: tabular-nums;
        text-align: right;
    }
</style>
