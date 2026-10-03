<script lang="ts">
    import { onMount, tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';

    /** A month grid that picks one day. The opener closes it on a click outside. */
    let { date, onPick, onClose }: { date: string; onPick: (date: string) => void; onClose: () => void } = $props();

    const DAY_MS = 86_400_000;
    const iso = (time: number) => new Date(time).toISOString().slice(0, 10);
    const parts = (value: string) => value.split('-').map(Number);
    const today = new Date(Date.now() - new Date().getTimezoneOffset() * 60_000).toISOString().slice(0, 10);

    let root: HTMLElement;
    // svelte-ignore state_referenced_locally
    let focused = $state(date);
    const first = $derived.by(() => { const [y, m] = parts(focused); return Date.UTC(y, m - 1, 1); });
    const weeks = $derived.by(() => {
        const [y, m] = parts(focused);
        const lead = (new Date(first).getUTCDay() + 6) % 7;
        const days = new Date(Date.UTC(y, m, 0)).getUTCDate();
        const cells = [...Array<null>(lead).fill(null), ...Array.from({ length: days }, (_, i) => iso(first + i * DAY_MS))];
        return Array.from({ length: Math.ceil(cells.length / 7) }, (_, w) => cells.slice(7 * w, 7 * w + 7));
    });
    const title = $derived(new Date(first).toLocaleDateString('en-GB', { month: 'long', year: 'numeric', timeZone: 'UTC' }));
    const dayName = (day: string) => new Date(day).toLocaleDateString('en-GB', { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' });

    /** The same day in another month, or its last day when the month is shorter. */
    function addMonths(value: string, months: number) {
        const [y, m, d] = parts(value);
        return iso(Date.UTC(y, m - 1 + months, Math.min(d, new Date(Date.UTC(y, m + months, 0)).getUTCDate())));
    }

    async function move(day: string, focus = true) {
        focused = day;
        if (!focus) return;
        await tick();
        root.querySelector<HTMLElement>(`[data-day="${day}"]`)?.focus();
    }

    function key(event: KeyboardEvent) {
        if (event.key === 'Escape') {
            event.stopPropagation();
            onClose();
            return;
        }
        const [y, m, d] = parts(focused);
        const weekday = (new Date(Date.UTC(y, m - 1, d)).getUTCDay() + 6) % 7;
        const days = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7, Home: -weekday, End: 6 - weekday }[event.key];
        const months = { PageUp: -1, PageDown: 1 }[event.key];
        if (days === undefined && months === undefined) return;
        if (!(event.target as HTMLElement).dataset.day) return;
        event.preventDefault();
        void move(months ? addMonths(focused, event.shiftKey ? 12 * months : months) : iso(Date.UTC(y, m - 1, d + days!)));
    }

    onMount(() => root.querySelector<HTMLElement>(`[data-day="${focused}"]`)?.focus());
</script>

<div class="calendar" role="dialog" tabindex="-1" aria-label="Choose the layer date" bind:this={root} onkeydown={key}>
    <div class="head">
        <button type="button" aria-label="Previous month" onclick={() => move(addMonths(focused, -1), false)}><Icon path="m15 5-7 7 7 7" size={16} /></button>
        <h3 aria-live="polite">{title}</h3>
        <button type="button" aria-label="Next month" onclick={() => move(addMonths(focused, 1), false)}><Icon name="chevron" size={16} /></button>
    </div>
    <table>
        <thead><tr>{#each ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'] as name (name)}<th scope="col"><abbr title={name}>{name.slice(0, 2)}</abbr></th>{/each}</tr></thead>
        <tbody>
            {#each weeks as week, w (w)}
                <tr>
                    {#each week as day, i (day ?? `blank-${i}`)}
                        <td>{#if day}<button type="button" data-day={day} tabindex={day === focused ? 0 : -1} aria-label={dayName(day)} aria-pressed={day === date}
                            class:today={day === today} onclick={() => onPick(day)}>{Number(day.slice(8))}</button>{/if}</td>
                    {/each}
                </tr>
            {/each}
        </tbody>
    </table>
</div>

<style>
    .calendar { width: 260px; padding: 10px 12px 12px; border-radius: 8px; background: var(--panel); color: var(--ink); font: 13px var(--sans); box-shadow: var(--planner-shadow); animation: rise 140ms cubic-bezier(.16, 1, .3, 1); }
    @keyframes rise { from { opacity: 0; transform: translateY(4px); } }
    @media (prefers-reduced-motion: reduce) { .calendar { animation: none; } }
    button { padding: 0; border: 0; background: none; color: var(--ink); font: inherit; cursor: pointer; }
    .head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 6px; }
    h3 { margin: 0; font: 600 14px var(--sans); }
    .head button { display: grid; place-items: center; width: 30px; height: 30px; border-radius: 6px; }
    .head button:hover { background: var(--parchment-2); }
    table { width: 100%; border-collapse: collapse; table-layout: fixed; }
    th { padding: 4px 0; font-size: 11px; font-weight: 600; color: var(--ink-soft); text-align: center; }
    abbr { text-decoration: none; }
    td { padding: 1px; text-align: center; }
    td button { width: 100%; height: 30px; border-radius: 6px; font-size: 13px; font-variant-numeric: tabular-nums; }
    td button:hover { background: var(--parchment-2); }
    td button.today { box-shadow: inset 0 0 0 1px var(--line-strong); font-weight: 700; }
    td button[aria-pressed="true"] { background: var(--ink); color: var(--panel); font-weight: 700; }
    td button:focus-visible { outline: 2px solid var(--forest); outline-offset: 1px; }
</style>
