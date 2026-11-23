<!-- The panel of "When": any time, open now, or open on a day of the week. An opening on a day of the trip, which only a
     sentence gives, shows as a row of its own. -->
<script lang="ts">
    import { contract } from '../../lib/planner/search/presentation';
    import { openWords, weekdayWord, type Open } from '../../lib/planner/search/slots';
    import QueryChoice from './QueryChoice.svelte';

    let { open, onchange }: { open?: Open; onchange: (open: Open | undefined) => void } = $props();
</script>

<div class="when" role="group" aria-label="Opening time">
    <QueryChoice label="Any time" icon="clock" on={!open} onclick={() => onchange(undefined)} />
    <QueryChoice label="Open now" icon="clock" on={!!open?.now} onclick={() => onchange(open?.now ? undefined : { now: true })} />
    <QueryChoice label="Open on a day of the week" icon="calendar" on={!!open?.weekday} onclick={() => onchange(open?.weekday ? undefined : { weekday: 'sun' })}>
        <div class="pills" role="group" aria-label="Day of the week">
            {#each contract.weekdays as day (day)}
                <button type="button" aria-pressed={open?.weekday === day} aria-label={weekdayWord(day)} onclick={() => onchange({ weekday: day })}>{weekdayWord(day).slice(0, 3)}</button>
            {/each}
        </div>
    </QueryChoice>
    {#if open?.day !== undefined}<QueryChoice label={openWords(open)[0]} icon="calendar" on onclick={() => onchange(undefined)} />{/if}
</div>

<style>
    .when {
        display: flex;
        flex-direction: column;
        width: 340px;
        max-width: 100%;
        padding: 8px 12px;
    }
</style>
