<script lang="ts">
    import type { Snippet } from 'svelte';
    import Card from '../../lib/ui/Card.svelte';
    import type { RideSummary } from '../../lib/library/records';
    import { duration } from '../../lib/planner/editor';
    import { STEEPEST_KM } from '../../lib/planner/route-facts';
    import type { Channel, Channels, RideClimb } from '../../lib/ride/numbers';
    import { ROUTES } from '../../lib/routes';
    import { formatDistance, formatHeight, formatPercent, formatSpeed } from '../../lib/settings/format';
    import { townOf } from './towns';

    /** The left card of the Ride page: the numbers of the summary, then heart rate, power and cadence with their zones and
     * the climbs, which come from the samples. A sensor with no value shows nothing. */
    let { summary, channels, climbs, unreadable, children }: {
        summary: RideSummary;
        /** Undefined while the samples load or when the file cannot be read. */
        channels?: Channels;
        climbs?: RideClimb[];
        unreadable: boolean;
        /** The buttons at the end of the card. */
        children: Snippet;
    } = $props();

    const hours = (seconds: number) => duration(seconds / 3600);
</script>

{#snippet number(label: string, value: string)}
    <div><dt>{label}</dt><dd>{value}</dd></div>
{/snippet}

{#snippet sensor(label: string, channel: Channel | undefined, unit: string, max: boolean)}
    {#if channel}
        <li class="sensor"><span>{label}</span><strong>{Math.round(channel.mean)}{unit}</strong><small>{max ? `max ${Math.round(channel.max)}` : ''}</small></li>
    {/if}
{/snippet}

{#snippet zones(label: string, channel: Channel | undefined, unit: string, missing: string, tone: 'warm' | 'cool')}
    {#if channel}
        <h3>{label}, time in each zone</h3>
        {#if channel.zones}
            {@const parts = channel.zones.map(({ zone, seconds }) =>
                `${zone.name} · ${Number.isFinite(zone.to) ? `${zone.from}–${zone.to}` : `${zone.from}+`} ${unit} · ${hours(seconds)}`)}
            <div class={`bar ${tone}`} role="img" aria-label={parts.join(', ')}>
                {#each channel.zones as { seconds }, i (i)}
                    <span class={`z${i + 1}`} style:flex-grow={seconds} title={parts[i]}></span>
                {/each}
            </div>
        {:else}
            <p class="note">Set your {missing} in <a href={ROUTES.settings}>Settings</a> to see zones.</p>
        {/if}
    {/if}
{/snippet}

<div class="facts">
    <Card>
        <dl class="numbers">
            {@render number('Distance', formatDistance(summary.lengthM))}
            {@render number('Climb', formatHeight(summary.climbM))}
            {@render number('Time moving', hours(summary.movingS))}
            {@render number('Speed', summary.movingS ? formatSpeed(summary.lengthM / summary.movingS) : '–')}
        </dl>

        {#if unreadable}
            <p class="note" role="alert">The file of this ride cannot be read.</p>
        {:else if channels && climbs}
            <ul class="sensors">
                {@render sensor('Heart rate', channels.hr, '', true)}
                {@render sensor('Power', channels.power, ' W', true)}
                {@render sensor('Cadence', channels.cadence, '', false)}
            </ul>
            <div class="zones">
                {@render zones('Heart rate', channels.hr, 'bpm', 'maximum heart rate', 'warm')}
                {@render zones('Power', channels.power, 'W', 'threshold power', 'cool')}
            </div>
            {#if climbs.length}
                <h3 class="climbs">Climbs</h3>
                <ul class="rows">
                    {#each climbs as climb (climb.from)}
                        {@const numbers = [formatDistance((climb.to - climb.from) * 1000), hours(climb.seconds), climb.powerW === undefined ? '' : `${Math.round(climb.powerW)} W`].filter(Boolean).join(' · ')}
                        <li>
                            {#await townOf(climb.top)}
                                <strong>Climb</strong>
                            {:then name}
                                <strong>{name ? `To ${name}` : 'Climb'}</strong>
                            {/await}
                            <small>{numbers}</small>
                            <small>{formatPercent(climb.gradient)}, steepest {formatDistance(STEEPEST_KM * 1000)} {formatPercent(climb.steepest)}</small>
                        </li>
                    {/each}
                </ul>
            {/if}
        {/if}

        <div class="actions">{@render children()}</div>
    </Card>
</div>

<style>
    .facts {
        min-height: 0;
    }
    .facts > :global(section) {
        display: flex;
        flex-direction: column;
        height: 100%;
        overflow: auto;
    }
    .numbers {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 14px 12px;
        margin: 0;
    }
    dt, h3 {
        color: var(--ink-soft);
        font: 600 11px/1.3 var(--sans);
        letter-spacing: .06em;
        text-transform: uppercase;
    }
    dd {
        margin: 2px 0 0;
        font: 600 21px/1.2 var(--sans);
    }
    h3 {
        margin: 14px 0 6px;
    }
    .climbs {
        margin-top: 20px;
        padding-bottom: 6px;
        border-bottom: 1px solid var(--line);
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .sensors {
        margin-top: 12px;
        border-top: 1px solid var(--line);
    }
    .sensor {
        display: grid;
        grid-template-columns: 1fr auto 64px;
        gap: 10px;
        align-items: baseline;
        padding: 8px 0;
        border-bottom: 1px solid var(--line);
    }
    .sensor strong {
        text-align: right;
    }
    .rows > li {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 9px 0;
        border-bottom: 1px solid var(--line);
    }
    small {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .note {
        margin: 12px 0 0;
        color: var(--ink-soft);
    }
    .note a {
        color: inherit;
    }
    .zones .note {
        margin: 0;
    }
    .bar {
        display: flex;
        height: 10px;
        overflow: hidden;
        border-radius: 3px;
        background: var(--parchment-2);
    }
    .bar span {
        flex-basis: 0;
        min-width: 0;
    }
    /* Five steps from a pale tint to the darkest shade: warm for heart rate, cool for power. */
    .z1 { background: color-mix(in srgb, var(--zone) 18%, var(--panel)); }
    .z2 { background: color-mix(in srgb, var(--zone) 45%, var(--panel)); }
    .z3 { background: color-mix(in srgb, var(--zone) 75%, var(--panel)); }
    .z4 { background: var(--zone); }
    .z5 { background: color-mix(in srgb, var(--zone) 60%, var(--ink)); }
    .warm {
        --zone: var(--ride);
    }
    .cool {
        --zone: var(--water);
    }
    /* The card scrolls inside; its buttons stay in sight and cover the padding of the card under them. */
    .actions {
        position: sticky;
        bottom: -16px;
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        margin: auto 0 -16px;
        padding: 16px 0;
        background: var(--panel);
    }
</style>
