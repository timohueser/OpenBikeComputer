<script lang="ts">
    import QueryAlong from './QueryAlong.svelte';
    import QueryPointPicker from './QueryPointPicker.svelte';
    import { allKinds, fieldLabel, kindLabel } from '../../lib/planner/search/presentation';
    import type { QueryRequest, Where, Quantity, QueryPoint, QueryDay } from '../../lib/planner/search/types';
    let { field, value, removed = false, removable = true, days, onChange, onToggle }: {
        field: string; value: unknown; removed?: boolean; removable?: boolean; days: number[];
        onChange: (value: unknown) => void; onToggle: () => void;
    } = $props();
    let expanded = $state(false);
    const where = $derived((value ?? {}) as Where);
    const quantity = $derived(value as Quantity);
    const opening = $derived(value as QueryRequest['open']);
    const requiredPoint = $derived(['from','to','point','at','near'].includes(field));
    function change(next: unknown) { onChange(next); expanded = false; }
</script>
<div class="chip-group">
    <button type="button" class="chip" class:removed aria-expanded={expanded} onclick={() => removed ? onToggle() : expanded = !expanded}>{fieldLabel(field, value)}</button>
    {#if expanded}
        <div class="picker">
            {#if field === 'what' && Array.isArray(value)}
                {#each value as selected, i}
                    <label>Place type {i+1}<select value={selected} onchange={e => change(value.map((v,n) => n===i?e.currentTarget.value:v))}>
                        {#each allKinds as kind}<option value={kind}>{kindLabel(kind)}</option>{/each}
                    </select></label>
                    {#if value.length>1}<button type="button" onclick={() => change(value.filter((_,n)=>n!==i))}>Remove {kindLabel(selected)}</button>{/if}
                {/each}
                {#if value.length<3}<button type="button" onclick={() => change([...value,allKinds.find(k=>!value.includes(k))])}>Add place type</button>{/if}
            {:else if field === 'where'}
                <label>Search area<select value={where.along ? 'interval' : where.day ? 'day' : where.scope ?? (where.near ? 'near' : where.anchor ? 'anchor' : 'view')} onchange={e => change(e.currentTarget.value === 'day' ? { day: days[0] ?? 1 } : e.currentTarget.value === 'interval' ? {along:{ref:'km',from:{value:0,unit:'km'},to:{value:10,unit:'km'}}} : ['near','anchor'].includes(e.currentTarget.value) ? where : { scope: e.currentTarget.value })}>
                    <option value="interval">Distance or time interval</option>{#if where.anchor}<option value="anchor">Selected map point</option>{/if}<option value="view">In this map view</option><option value="route">Along the route</option><option value="here">Near my location</option>
                    {#if days.length}<option value="day">On a day</option>{/if}
                    {#if where.near}<option value="near">Near {where.near.map(p => 'name' in p ? p.name : 'point').join(' and ')}</option>{/if}
                </select></label>
                {#if where.day}
                    <label>Day<select value={String(where.day)} onchange={e => change({ ...where, day: /^\d+$/.test(e.currentTarget.value) ? Number(e.currentTarget.value) : e.currentTarget.value as QueryDay })}>
                        {#each days as day}<option value={day}>Day {day}</option>{/each}<option value="every">Every day</option><option value="today">Today</option><option value="tomorrow">Tomorrow</option>
                    </select></label>
                    <label>Part<select value={where.part ?? 'whole'} onchange={e => { const next = { ...where }; if (e.currentTarget.value === 'whole') delete next.part; else next.part = e.currentTarget.value as Where['part']; change(next); }}>
                        <option value="whole">Whole day</option><option value="start">Start</option><option value="middle">Middle</option><option value="end">End</option>
                    </select></label>
                {/if}
                {#if where.along}<QueryAlong value={where.along} onChange={along => change({...where,along})} />{/if}
                {#each ['before','after'] as side}
                    {#if where[side as 'before' | 'after']}
                        <span>{side}</span><QueryPointPicker value={where[side as 'before' | 'after']!} {days} onChange={p => change({...where,[side]:p})} />
                    {/if}
                {/each}
                {#each where.near ?? [] as p,i}<QueryPointPicker value={p} {days} onChange={p => change({...where,near:where.near!.map((old,n)=>n===i?p:old)})} />{/each}
                <label>Near a named place<input aria-label="Search near a place" placeholder="Place name" onchange={e => { if (e.currentTarget.value.trim()) change({ ...where, near: [{ name: e.currentTarget.value.trim() }] }); }} /></label>
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
            {#if removable}<button type="button" onclick={() => { onToggle(); expanded = false; }}>Remove filter</button>{/if}
        </div>
    {/if}
</div>
<style>
    .chip-group { display: contents; }
    button, .picker :global(input), .picker :global(select) { font: inherit; color: var(--ink); background: var(--panel); border: 1px solid var(--line); border-radius: 6px; }
    button { cursor: pointer; min-height: 32px; padding: 5px 8px; }
    .chip { font-size: 11px; max-width: 100%; overflow-wrap: anywhere; text-align: start; }
    .removed { text-decoration: line-through; color: var(--ink-soft); }
    .picker { display: flex; flex-wrap: wrap; gap: 8px; padding: 8px 0; width: 100%; font-size: 13px; }
    .picker :global(label) { display: flex; flex-direction: column; gap: 4px; max-width: 100%; }
    .picker :global(input), .picker :global(select) { padding: 6px; min-height: 36px; max-width: 100%; min-width: 0; }
    button:hover { border-color: var(--ink-soft); }
    button:focus-visible, .picker :global(input:focus-visible), .picker :global(select:focus-visible) { outline: 2px solid var(--ink); outline-offset: 2px; }
</style>
