<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import QueryKinds from './QueryKinds.svelte';
    import QueryAreaPicker from './QueryAreaPicker.svelte';
    import { tick } from 'svelte';
    import QueryPointPicker from './QueryPointPicker.svelte';
    import { allKinds, fieldLabel, kindLabel } from '../../lib/planner/search/presentation';
    import type { QueryRequest, Where, Quantity, QueryPoint } from '../../lib/planner/search/types';
    let { field, value: currentValue, active = $bindable(null), removed = false, removable = true, days, onChange, onToggle }: {
        field: string; value: unknown; active?: string | null; removed?: boolean; removable?: boolean; days: number[];
        onChange: (value: unknown) => void; onToggle: () => void;
    } = $props();
    let value = $state<unknown>();
    let trigger: HTMLButtonElement;
    let panel = $state<HTMLDivElement>();
    const expanded = $derived(active === field);
    const group = $derived(field === 'what' || field === 'cuisine' || field === 'name' ? 'place' : ['where','from','to','point','at','near','via'].includes(field) ? 'area' : ['open','day','days'].includes(field) ? 'time' : 'route');
    const icon = $derived(group === 'place' ? 'search' : group === 'area' ? 'pin' : group === 'time' ? 'calendar' : 'sliders');
    const title = $derived(({what:'Place types',where:'Search area',open:'Opening hours',name:'Place name',radius:'Search radius',every:'Spacing',per_day:'Daily distance',min:'Minimum',from:'Start',to:'Destination',at:'End point',point:'Route point',via:'Via points'} as Record<string,string>)[field] ?? kindLabel(field));
    const valid = $derived(value !== undefined && (!Array.isArray(value) || value.length > 0) && ! /"name":"\s*"/.test(JSON.stringify(value)));
    const quantity = $derived(value as Quantity);
    const opening = $derived(value as QueryRequest['open']);
    const requiredPoint = $derived(['from','to','point','at','near'].includes(field));
    function change(next: unknown) { value = next; }
    function apply() {
        if (panel?.querySelector<HTMLInputElement>(':invalid')?.reportValidity() === false) return;
        onChange(value); close();
    }
    function close() { active = null; trigger?.focus({preventScroll:true}); }
    async function open() {
        if (removed) { onToggle(); return; }
        if (expanded) { close(); return; }
        value = structuredClone($state.snapshot(currentValue)); active = field;
        await tick(); panel?.querySelector<HTMLElement>('input, select, .fields button')?.focus({preventScroll:true});
    }
</script>
<div class="chip-group" data-group={group}>
    <button bind:this={trigger} type="button" class="chip" class:removed class:expanded aria-expanded={expanded} aria-controls={`query-${field}`} onclick={open}><Icon name={icon} size={13} /><span>{fieldLabel(field, currentValue)}</span><Icon name={removed ? 'plus' : 'down'} size={12} /></button>
    {#if expanded}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <div class="picker" id={`query-${field}`} bind:this={panel} role="group" aria-label={title} onkeydown={e => { if(e.key === 'Escape') { e.stopPropagation(); close(); } }}>
            <div class="head"><strong>{title}</strong><button type="button" class="dismiss" aria-label="Close filter editor" onclick={close}><Icon name="close" size={16} /></button></div>
            <div class="fields">
            {#if field === 'what' && Array.isArray(value)}
                <QueryKinds value={value} onChange={change} />
            {:else if field === 'where'}
                <QueryAreaPicker value={value as Where} {days} onChange={change} />
            {:else if field === 'cuisine'}<label>Food<select value={String(value)} onchange={e=>change(e.currentTarget.value)}><option value="pizza">Pizza</option><option value="kebab">Kebab</option></select></label>
            {:else if field === 'open'}
                <label>Opening filter<select value={opening?.now ? 'now' : opening?.weekday ?? `day:${opening?.day}`} onchange={e => change(e.currentTarget.value === 'now' ? { now: true } : e.currentTarget.value.startsWith('day:') ? { day: /^\d+$/.test(e.currentTarget.value.slice(4)) ? Number(e.currentTarget.value.slice(4)) : e.currentTarget.value.slice(4) } : { weekday: e.currentTarget.value })}>
                    <option value="now">Open now</option>{#each ['mon','tue','wed','thu','fri','sat','sun'] as day}<option value={day}>Open {day}</option>{/each}
                    {#each days as day}<option value={`day:${day}`}>On Day {day}</option>{/each}<option value="day:today">Today’s trip day</option><option value="day:tomorrow">Tomorrow’s trip day</option>
                </select></label>
            {:else if ['radius','every','per_day','min'].includes(field)}
                <label>{kindLabel(field)}<input type="number" min="0.1" max="10000" step="0.1" value={quantity.value} onchange={e => { const n = e.currentTarget.valueAsNumber; if (Number.isFinite(n) && n > 0) change({ ...quantity, value: n }); }} /></label>
                {#if field !== 'radius'}<label>Unit<select value={quantity.unit} onchange={e => change({ ...quantity, unit: e.currentTarget.value })}>
                    <option value="km">km</option>{#if field !== 'min'}<option value="h">riding hours</option>{:else}<option value="m">m climb</option><option value="%">% gradient</option>{/if}
                </select></label>{/if}
            {:else if field === 'bike' || field === 'goal' || field === 'kind'}
                <label>{kindLabel(field)}<select value={String(value)} onchange={e => change(e.currentTarget.value)}>
                    {#each field === 'bike' ? ['road','gravel','mtb','touring'] : field === 'goal' ? ['balanced','shortest','least_climbing','least_unpaved','most_climbing'] : ['visit','stop','pass'] as choice}<option value={choice}>{kindLabel(choice)}</option>{/each}
                </select></label>
            {:else if field === 'days'}
                <label>Number of days<input type="number" min="1" max="14" value={Number(value)} onchange={e => { if (e.currentTarget.validity.valid) change(e.currentTarget.valueAsNumber); }} /></label>
            {:else if field === 'day'}
                <label>Day<select value={String(value)} onchange={e => change(/^\d+$/.test(e.currentTarget.value) ? Number(e.currentTarget.value) : e.currentTarget.value)}>
                    {#each days as day}<option value={day}>Day {day}</option>{/each}<option value="every">Every day</option><option value="today">Today</option><option value="tomorrow">Tomorrow</option>
                </select></label>
            {:else if requiredPoint}<QueryPointPicker value={value as QueryPoint} {days} onChange={change} />
            {:else if field === 'name'}<label>Place name<input value={String(value)} onchange={e => { if(e.currentTarget.value.trim()) change(e.currentTarget.value.trim()); }} /></label>
            {:else if field === 'via'}
                {#each (value as QueryPoint[]) as p,i}<QueryPointPicker value={p} {days} onChange={p => change((value as QueryPoint[]).map((old,n)=>n===i?p:old))} />{/each}
            {:else if field === 'what'}
                <label>Route data<select value={String(value)} onchange={e => change(e.currentTarget.value)}>
                    {#each ['climb','descent','steep','unpaved','unknown_surface','pushing','closure','main_road', ...allKinds.map(k => `gap:${k}`)] as choice}<option value={choice}>{kindLabel(choice)}</option>{/each}
                </select></label>
            {/if}
            </div>
            <div class="footer">
                {#if removable}<button type="button" class="remove" onclick={() => { onToggle(); close(); }}>Remove filter</button>{:else}<button type="button" class="remove" onclick={close}>Cancel</button>{/if}
                <button type="button" class="apply" disabled={!valid} onclick={apply}>Apply<Icon name="check" size={14} /></button>
            </div>
        </div>
    {/if}
</div>
<style>
    .chip-group { display: contents; --filter-ink: var(--ink-soft); --filter-tint: var(--parchment-2); }
    .chip-group[data-group="place"] { --filter-ink: var(--query-place); --filter-tint: color-mix(in srgb, var(--query-place) 11%, var(--panel)); }
    .chip-group[data-group="area"] { --filter-ink: var(--query-area); --filter-tint: color-mix(in srgb, var(--query-area) 11%, var(--panel)); }
    .chip-group[data-group="time"] { --filter-ink: var(--query-time); --filter-tint: color-mix(in srgb, var(--query-time) 11%, var(--panel)); }
    button { font: inherit; color: inherit; cursor: pointer; }
    .chip { display: inline-flex; align-items: center; gap: 6px; min-height: 32px; max-width: 100%; padding: 5px 10px; border: 1px solid transparent; border-radius: 999px; font-size: 12px; line-height: 1.4; color: var(--filter-ink); background: var(--filter-tint); text-align: left; }
    .chip span { overflow-wrap: anywhere; }
    .chip :global(svg) { flex: none; }
    .chip:hover, .chip.expanded { border-color: var(--filter-ink); }
    .chip.removed { color: var(--ink-soft); background: var(--parchment-2); }
    .removed span { text-decoration: line-through; }
    .picker { display: flex; flex-direction: column; max-height: min(520px, calc(100dvh - 320px)); order: 2; width: 100%; min-width: 0; margin-top: 6px; border: 1px solid var(--line-strong); border-radius: 12px; background: var(--panel); font-size: 13px; overflow: hidden; }
    .head { flex: none; display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 10px 12px; background: var(--filter-tint); color: var(--filter-ink); }
    .head strong { font-size: 13px; font-weight: 600; }
    .dismiss { display: grid; place-items: center; width: 28px; height: 28px; border: 0; border-radius: 50%; background: transparent; }
    .dismiss:hover { background: color-mix(in srgb, var(--filter-ink) 12%, transparent); }
    .fields { min-height: 0; overflow-y: auto; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; padding: 12px; }
    .fields :global(label) { display: flex; flex-direction: column; gap: 6px; grid-column: 1 / -1; min-width: 0; font-size: 12px; font-weight: 500; color: var(--ink-soft); }
    .fields :global(label.half) { grid-column: auto; }
    .fields :global(input), .fields :global(select) { width: 100%; min-width: 0; box-sizing: border-box; min-height: 40px; padding: 8px 10px; border: 1px solid var(--line-strong); border-radius: 6px; color: var(--ink); background: var(--panel); font: 400 13px var(--sans); caret-color: var(--ink); }
    .fields :global(input::placeholder) { color: var(--ink-soft); opacity: 1; }
    .footer { flex: none; display: flex; justify-content: space-between; gap: 8px; padding: 10px 12px; border-top: 1px solid var(--line); }
    .footer button { min-height: 36px; padding: 7px 12px; border: 0; border-radius: 6px; }
    .remove { background: transparent; color: var(--ink-soft); }
    .remove:hover { background: var(--parchment-2); color: var(--ink); }
    .apply { display: flex; align-items: center; gap: 8px; background: var(--filter-ink); color: var(--panel); font-weight: 600; }
    .apply:hover { filter: brightness(.92); }
    .apply:disabled { opacity: .4; cursor: default; }
    button:focus-visible, .fields :global(input:focus-visible), .fields :global(select:focus-visible) { outline: 2px solid var(--ink); outline-offset: -2px; }
</style>
