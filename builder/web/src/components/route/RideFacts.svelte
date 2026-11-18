<script lang="ts">
    import Card from '../../lib/ui/Card.svelte';
    import type { RideSummary } from '../../lib/library/records';
    import { duration } from '../../lib/planner/editor';
    import type { Channel, Channels, PartNumbers, RideClimb } from '../../lib/ride/numbers';
    import { formatDistance, formatHeight, formatSpeed } from '../../lib/settings/format';
    import { townOf } from './towns';
    import ZoneTimes from './ZoneTimes.svelte';

    /** The left card of the Ride page: the numbers of the summary, then heart rate, power and cadence, each sensor with
     * the time in its zones under it, and the climbs, which come from the samples. A sensor with no value shows nothing.
     * With a selected part, its numbers and its zones stand beside the numbers of the whole ride, and the climbs go. A
     * click on a climb selects it as the part. */
    let { summary, speed, channels, climbs, part, unreadable, onclimb }: {
        summary: RideSummary;
        /** The speed over the moving steps, in m/s (`movingSpeed`). */
        speed?: number;
        /** Undefined while the samples load or when the file cannot be read. */
        channels?: Channels;
        climbs?: RideClimb[];
        /** The numbers of the selected part. */
        part?: PartNumbers;
        unreadable: boolean;
        onclimb: (climb: RideClimb) => void;
    } = $props();

    const hours = (seconds: number) => duration(seconds / 3600);
    const speedText = (value?: number) => value === undefined ? '–' : formatSpeed(value);
    const mean = (channel: Channel | undefined, unit: string) => channel ? `${Math.round(channel.mean)}${unit}` : '–';
    const sensors = [
        { key: 'hr', label: 'Heart rate', unit: '', zoneUnit: 'bpm', missing: 'maximum heart rate' },
        { key: 'power', label: 'Power', unit: ' W', zoneUnit: 'W', missing: 'threshold power' },
        { key: 'cadence', label: 'Cadence', unit: '', zoneUnit: '', missing: '' },
    ] as const;
</script>

{#snippet number(label: string, value: string)}
    <div><dt>{label}</dt><dd>{value}</dd></div>
{/snippet}

{#snippet pair(label: string, selected: string, whole: string)}
    <tr><th scope="row">{label}</th><td>{selected}</td><td>{whole}</td></tr>
{/snippet}

<div class="facts">
    <Card>
        {#if part && channels}
            <table>
                <thead><tr><td></td><th scope="col">Selected</th><th scope="col">Whole ride</th></tr></thead>
                <tbody>
                    {@render pair('Distance', formatDistance(part.lengthM), formatDistance(summary.lengthM))}
                    {@render pair('Climb', formatHeight(part.climbM), formatHeight(summary.climbM))}
                    {@render pair('Time moving', hours(part.movingS), hours(summary.movingS))}
                    {@render pair('Speed', speedText(part.speed), speedText(speed))}
                    {#each sensors as { key, label, unit, zoneUnit, missing } (key)}
                        {#if channels[key]}
                            {@render pair(label, mean(part.channels[key], unit), mean(channels[key], unit))}
                            {#if missing && part.channels[key]}<tr class="zones"><td colspan="3"><ZoneTimes channel={part.channels[key]} unit={zoneUnit} {missing} /></td></tr>{/if}
                        {/if}
                    {/each}
                </tbody>
            </table>
        {:else}
            <dl class="numbers">
                {@render number('Distance', formatDistance(summary.lengthM))}
                {@render number('Climb', formatHeight(summary.climbM))}
                {@render number('Time moving', hours(summary.movingS))}
                {@render number('Speed', speedText(speed))}
            </dl>

            {#if unreadable}
                <p class="note" role="alert">The file of this ride cannot be read.</p>
            {:else if channels && climbs}
                <ul class="sensors" class:none={!sensors.some(({ key }) => channels[key])}>
                    {#each sensors as { key, label, unit, zoneUnit, missing } (key)}
                        {@const channel = channels[key]}
                        {#if channel}
                            <li class="sensor">
                                <span>{label}</span><strong>{mean(channel, unit)}</strong><small>{missing ? `max ${Math.round(channel.max)}` : ''}</small>
                                {#if missing}<div class="zones"><ZoneTimes {channel} unit={zoneUnit} {missing} /></div>{/if}
                            </li>
                        {/if}
                    {/each}
                </ul>
                {#if climbs.length}
                    <div class="climbs" style:--count={climbs.length}>
                        <h3>Climbs{#if climbs.length > 1}<small>{climbs.length}</small>{/if}</h3>
                        <ul>
                            {#each climbs as climb (climb.first)}
                                {@const numbers = [formatDistance((climb.to - climb.from) * 1000), hours(climb.seconds), climb.powerW === undefined ? '' : `${Math.round(climb.powerW)} W`].filter(Boolean).join(' · ')}
                                <li>
                                    <button type="button" onclick={() => onclimb(climb)}>
                                        {#await townOf(climb.top)}
                                            <strong>Climb</strong>
                                        {:then name}
                                            <strong>{name ? `To ${name}` : 'Climb'}</strong>
                                        {/await}
                                        <small>{numbers}</small>
                                    </button>
                                </li>
                            {/each}
                        </ul>
                    </div>
                {/if}
            {/if}
        {/if}
    </Card>
</div>

<style>
    .facts {
        min-height: 0;
        /* The height of a climb, and of the heading over the climbs. */
        --row: 34px;
        --head: 38px;
    }
    .facts > :global(section) {
        display: flex;
        flex-direction: column;
        height: 100%;
        overflow: auto;
    }
    .facts > :global(section > *) {
        flex: none;
    }
    .numbers {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 10px 12px;
        margin: 0;
    }
    dt, h3, th {
        color: var(--ink-soft);
        font: 13px/1.3 var(--sans);
    }
    dd {
        margin: 1px 0 0;
        font: 600 21px/1.2 var(--sans);
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .sensors {
        margin-top: 10px;
        border-top: 1px solid var(--line);
    }
    .none {
        display: none;
    }
    .sensor {
        display: grid;
        grid-template-columns: 1fr auto 60px;
        gap: 0 10px;
        align-items: baseline;
        padding: 6px 0 7px;
        border-bottom: 1px solid var(--line);
    }
    .sensor strong, td {
        font-variant-numeric: tabular-nums;
        text-align: right;
    }
    small {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .note {
        margin: 12px 0 0;
        color: var(--ink-soft);
    }
    /* The zones of a sensor stand under its row. */
    .sensor .zones {
        grid-column: 1 / -1;
    }
    .sensor .zones :global(.bar) {
        margin-top: 7px;
    }
    /* The part in ink, the whole ride beside it in the quiet colour. */
    table {
        width: 100%;
        border-collapse: collapse;
    }
    thead th {
        padding-bottom: 6px;
        font-size: 12.5px;
        text-align: right;
    }
    tbody th {
        font-weight: 400;
        text-align: left;
    }
    tbody th, tbody td {
        padding: 7px 0;
        border-top: 1px solid var(--line);
    }
    td {
        width: 84px;
        font-weight: 600;
        white-space: nowrap;
    }
    td:last-child {
        color: var(--ink-soft);
        font-weight: 400;
    }
    .zones td {
        padding: 0 0 9px;
        border: 0;
        white-space: normal;
    }
    /* The box is as high as its climbs, or as the card leaves it. Then the list is as high as whole climbs fit under
     * the heading and scrolls by whole climbs, so no climb is cut at the lower edge. The box holds the heading and one
     * climb at least; in a lower card, the card scrolls. */
    .facts .climbs {
        display: flex;
        flex: 0 1 calc(var(--head) + var(--count) * var(--row));
        flex-direction: column;
        min-height: calc(var(--head) + var(--row));
        container-type: size;
    }
    h3 {
        display: flex;
        box-sizing: border-box;
        justify-content: space-between;
        height: var(--head);
        margin: 0;
        padding-top: 14px;
        border-bottom: 1px solid var(--line);
        font-weight: 600;
    }
    h3 small {
        font-weight: 400;
    }
    .climbs ul {
        height: calc(100cqh - var(--head));
        height: round(down, 100cqh - var(--head), var(--row));
        overflow-y: auto;
        scroll-snap-type: y mandatory;
    }
    .climbs li {
        box-sizing: border-box;
        height: var(--row);
        border-bottom: 1px solid var(--line);
        scroll-snap-align: start;
    }
    .climbs button {
        display: flex;
        gap: 10px;
        align-items: center;
        justify-content: space-between;
        width: 100%;
        height: 100%;
        padding: 0;
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        text-align: left;
    }
    .climbs button:hover strong {
        text-decoration: underline;
    }
    .climbs button:focus-visible {
        outline-offset: -2px;
    }
    .climbs strong {
        overflow: hidden;
        font-weight: 400;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .climbs small {
        flex: none;
        font-variant-numeric: tabular-nums;
    }
    /* The card is as high as its content: every climb shows. */
    @media (max-width: 760px) {
        .facts .climbs {
            flex: none;
            container-type: normal;
        }
        .climbs ul {
            height: auto;
            overflow: visible;
        }
    }
</style>
