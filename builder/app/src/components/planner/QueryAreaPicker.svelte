<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import QueryAlong from './QueryAlong.svelte';
    import QueryPointPicker from './QueryPointPicker.svelte';
    import type { Where, QueryDay } from '../../lib/planner/search/types';
    let { value, days, selection, onChange }: { value: Where; days: number[]; selection?: Where; onChange: (value: Where) => void } = $props();
    const mode = $derived(value.near ? 'near' : value.along ? 'interval' : value.day ? 'day' : value.anchor ? 'anchor' : value.scope ?? 'view');
    const options = $derived([
        {id:'view', label:'Map view', icon:'fit'}, {id:'route', label:'Route', icon:'route'},
        {id:'near', label:'Near a place', icon:'pin'}, {id:'here', label:'My location', icon:'locate'},
        ...(days.length ? [{id:'day',label:'Trip day',icon:'calendar'}] : []), {id:'interval',label:'Route section',icon:'sliders'},
        ...(value.anchor || selection ? [{id:'anchor',label:'Selected point',icon:'pin'}] : []),
    ]);
    function choose(nextMode: string) {
        if (nextMode === mode) return;
        const choice = nextMode;
        if (choice === 'anchor') { onChange(selection ?? { anchor: value.anchor }); return; }
        onChange(choice === 'near' ? {near:[{name:''}]} : choice === 'day' ? {day:days[0]} : choice === 'interval' ? {along:{ref:'km',from:{value:0,unit:'km'},to:{value:10,unit:'km'}}} : {scope:choice as Where['scope']});
    }
</script>
<div class="scopes" role="group" aria-label="Search area">
    {#each options as option}<button type="button" class:chosen={mode === option.id} aria-pressed={mode === option.id} onclick={() => choose(option.id)}><Icon name={option.icon} size={16} />{option.label}</button>{/each}
</div>
{#if value.day}
    <label class="half">Day<select value={String(value.day)} onchange={e => onChange({...value,day: /^\d+$/.test(e.currentTarget.value) ? Number(e.currentTarget.value) : e.currentTarget.value as QueryDay})}>
        {#each days as day}<option value={day}>Day {day}</option>{/each}<option value="every">Every day</option><option value="today">Today</option><option value="tomorrow">Tomorrow</option>
    </select></label>
{/if}
{#if value.day || mode === 'route'}
    <label class="half">{value.day ? 'Part of day' : 'Part of route'}<select value={value.part ?? 'whole'} onchange={e => { const next = {...value}; if(e.currentTarget.value === 'whole') delete next.part; else next.part = e.currentTarget.value as Where['part']; onChange(next); }}><option value="whole">{value.day ? 'Whole day' : 'Whole route'}</option><option value="start">Start</option><option value="middle">Middle</option><option value="end">End</option></select></label>
{/if}
{#if value.along}<QueryAlong value={value.along} onChange={along => onChange({...value,along})} />{/if}
{#each value.near ?? [] as point, i}
    <div class="point"><QueryPointPicker value={point} {days} onChange={p => onChange({...value,near:value.near!.map((old,n)=>n===i?p:old)})} /></div>
{/each}
{#each ['before','after'] as side}
    {#if value[side as 'before' | 'after']}
        <div class="point"><strong>{side === 'before' ? 'Before' : 'After'}</strong><QueryPointPicker value={value[side as 'before' | 'after']!} {days} onChange={p => onChange({...value,[side]:p})} /></div>
    {/if}
{/each}
<style>
    .scopes { grid-column: 1 / -1; display: grid; grid-template-columns: repeat(2, minmax(0,1fr)); gap: 6px; }
    .scopes button { display: flex; align-items: center; gap: 8px; min-height: 44px; padding: 8px; background: var(--panel); border: 1px solid var(--line); border-radius: 8px; color: var(--ink); font: inherit; text-align: left; cursor: pointer; }
    .scopes button :global(svg) { flex: none; }
    .scopes button.chosen { background: var(--filter-tint); border-color: var(--filter-ink); color: var(--filter-ink); }
    .scopes button:hover { border-color: var(--filter-ink); }
    .scopes button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .point { display: grid; grid-template-columns: repeat(2,minmax(0,1fr)); gap: 12px; grid-column: 1 / -1; min-width: 0; }
    strong { grid-column: 1 / -1; font-size: 12px; }
</style>
