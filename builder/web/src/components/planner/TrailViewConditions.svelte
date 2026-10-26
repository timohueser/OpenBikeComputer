<script lang="ts">
    import { onDestroy, tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import DateStepper from './DateStepper.svelte';
    import Segmented from './Segmented.svelte';
    import { clock, daylight } from '../../lib/planner/layers/sun';
    import { TIME_ZONE, today, type View, type Weather } from '../../lib/trailview/view';

    type Conditions = Pick<View, 'date' | 'time' | 'weather'>;
    /** A chip with the day, the time and the weather of the view; a click opens a card that changes them. */
    let { view, onChange }: { view: View; onChange: (change: Partial<Conditions>) => void } = $props();

    const WEATHER: { value: Weather; label: string; icon: string }[] = [
        { value: 'sunny', label: 'Sunny', icon: 'sun' },
        { value: 'cloudy', label: 'Cloudy', icon: 'weather' },
        { value: 'rainy', label: 'Rainy', icon: 'rain' },
    ];

    let open = $state(false);
    let root: HTMLElement;
    let chip: HTMLButtonElement;
    let card = $state<HTMLElement>();
    // The view changes at each frame of a walk; these values change only when they differ.
    const date = $derived(view.date), time = $derived(view.time);
    // A walk moves the sun times by seconds: they follow the eye to the hundredth of a degree.
    const lon = $derived(Math.round(view.lon * 100) / 100), lat = $derived(Math.round(view.lat * 100) / 100);
    const day = $derived(new Date(date).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', timeZone: 'UTC' }));
    const span = $derived(daylight([lon, lat], date, TIME_ZONE));
    const sun = $derived(!span ? 'No sunrise' : span[0] === 0 && span[1] === 1440 ? 'Sun all day' : `Sunrise ${clock(span[0])} · Sunset ${clock(span[1])}`);
    // The track is night before sunrise and after sunset, day between, with 30 minutes of twilight at each edge.
    const track = $derived.by(() => {
        if (!span) return 'var(--water)';
        const at = (minute: number) => `${(minute / 1440 * 100).toFixed(2)}%`;
        const [rise, set] = span;
        return `linear-gradient(90deg, var(--water) ${at(rise - 15)}, var(--amber) ${at(rise + 15)}, var(--amber) ${at(set - 15)}, var(--water) ${at(set + 15)})`;
    });

    /** Passes on the last value at most once per `ms`; flush() passes a waiting value on at once. */
    function throttle<T>(ms: number, send: (value: T) => void) {
        let timer: ReturnType<typeof setTimeout> | undefined, last: T;
        const flush = () => {
            if (timer === undefined) return;
            clearTimeout(timer);
            timer = undefined;
            send(last);
        };
        onDestroy(() => clearTimeout(timer));
        return { set: (value: T) => { last = value; timer ??= setTimeout(flush, ms); }, flush };
    }

    // The slider and the day stepper move drafts. The view takes the time at most every 100 ms, as the layer date
    // bar does, and the day at most every 300 ms: a new date rebuilds every ground page.
    const minutes = (time: string) => Number(time.slice(0, 2)) * 60 + Number(time.slice(3));
    let draft = $state(720);
    $effect(() => { draft = minutes(time); });
    let draftDay = $derived(date);
    const longDay = $derived(new Date(draftDay).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' }));
    const sendTime = throttle(100, (time: string) => onChange({ time }));
    const sendDay = throttle(300, (date: string) => onChange({ date }));
    function pick(day: string) {
        draftDay = day;
        sendDay.set(day);
    }

    async function show() {
        open = true;
        await tick();
        card?.querySelector<HTMLElement>('button')?.focus();
    }
    function hide() {
        open = false;
        chip.focus();
    }
</script>

<svelte:window onpointerdown={(event) => { if (open && !root.contains(event.target as Node)) open = false; }} />

<!-- Keys in the card stop here: the engine reads the arrow keys on `window`. An Escape whose default runs closes the window. -->
<div class="conditions" bind:this={root} role="presentation" onkeydown={(event) => {
    if (open && event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        hide();
    } else if (card?.contains(event.target as Node)) event.stopPropagation();
}}>
    <button type="button" class="chip" bind:this={chip} aria-expanded={open} aria-controls="trailview-conditions"
        aria-label={`${day}, ${time}, ${view.weather}. Change the day, the time and the weather`} onclick={() => open ? hide() : show()}>
        <Icon name={WEATHER.find(weather => weather.value === view.weather)?.icon} />{day} · {time}<Icon name={open ? 'up' : 'down'} size={14} />
    </button>
    {#if open}
        <div class="card" id="trailview-conditions" role="group" aria-label="Day, time and weather" bind:this={card}>
            <section aria-label="Day">
                <h3>Day</h3>
                <DateStepper date={draftDay} text={longDay} label={`Day: ${longDay}. Choose a day`} below onDate={pick} />
                <button type="button" class="today" disabled={draftDay === today()} onclick={() => pick(today())}>Today</button>
            </section>
            <section>
                <label class="head" for="trailview-time"><span>Time</span><strong>{clock(draft)}</strong></label>
                <input id="trailview-time" type="range" min="0" max="1435" step="5" bind:value={draft} aria-valuetext={clock(draft)} style:--track={track}
                    oninput={() => sendTime.set(clock(draft))} onchange={sendTime.flush} />
                <div class="scale"><span>00:00</span><span>{sun}</span><span>24:00</span></div>
            </section>
            <section aria-label="Weather">
                <h3>Weather</h3>
                <Segmented label="Weather" options={WEATHER} value={view.weather} onChange={(weather) => onChange({ weather })} />
            </section>
        </div>
    {/if}
</div>

<style>
    .chip {
        position: absolute;
        top: max(12px, env(safe-area-inset-top));
        left: max(12px, env(safe-area-inset-left));
        display: flex;
        align-items: center;
        gap: 8px;
        height: 38px;
        padding: 0 12px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
        font: 700 14px var(--sans);
        font-variant-numeric: tabular-nums;
        box-shadow: var(--planner-shadow);
        cursor: pointer;
    }
    .chip :global(svg:last-child) {
        color: var(--ink-soft);
    }
    .chip:hover,
    .chip[aria-expanded="true"] {
        background: var(--parchment-2);
    }
    .chip:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }
    .card {
        position: absolute;
        top: calc(max(12px, env(safe-area-inset-top)) + 46px);
        left: max(12px, env(safe-area-inset-left));
        display: flex;
        flex-direction: column;
        gap: 16px;
        width: 340px;
        padding: 16px;
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
        font: 14px var(--sans);
        box-shadow: var(--planner-shadow);
    }
    section {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    h3,
    .head span,
    .scale {
        margin: 0;
        color: var(--ink-soft);
        font: 12px var(--sans);
    }
    .today {
        align-self: flex-start;
        padding: 4px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 14px;
        background: var(--panel);
        color: var(--ink);
        font: 12px var(--sans);
        cursor: pointer;
    }
    .today:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .today:disabled {
        color: var(--ink-faint);
        cursor: default;
    }
    .head {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
    }
    .head strong {
        font-variant-numeric: tabular-nums;
    }
    input {
        width: 100%;
        height: 24px;
        margin: 0;
        background: none;
        cursor: pointer;
        appearance: none;
        -webkit-appearance: none;
    }
    input::-webkit-slider-runnable-track {
        height: 6px;
        border-radius: 3px;
        background: var(--track);
    }
    input::-moz-range-track {
        height: 6px;
        border-radius: 3px;
        background: var(--track);
    }
    input::-webkit-slider-thumb {
        -webkit-appearance: none;
        width: 18px;
        height: 18px;
        margin-top: -6px;
        border: 3px solid var(--panel);
        border-radius: 50%;
        background: var(--rust);
        box-shadow: 0 1px 3px rgba(28, 27, 20, .4);
    }
    input::-moz-range-thumb {
        width: 12px;
        height: 12px;
        border: 3px solid var(--panel);
        border-radius: 50%;
        background: var(--rust);
        box-shadow: 0 1px 3px rgba(28, 27, 20, .4);
    }
    :is(.today, input):focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }
    .scale {
        display: flex;
        justify-content: space-between;
        gap: 8px;
        font-size: 11px;
        font-variant-numeric: tabular-nums;
    }
    @media (max-width: 760px) {
        .card {
            right: max(12px, env(safe-area-inset-right));
            width: auto;
        }
    }
</style>
