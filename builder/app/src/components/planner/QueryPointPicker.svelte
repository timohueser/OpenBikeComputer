<script lang="ts">
    import QueryAlong from './QueryAlong.svelte';
    import { allKinds, kindLabel } from '../../lib/planner/search/presentation';
    import type { QueryDay, QueryPoint } from '../../lib/planner/search/types';
    let { value, days, onChange }: { value: QueryPoint; days: number[]; onChange: (value: QueryPoint) => void } = $props();
    let naming = $state(false);
    const mode = $derived('name' in value ? 'name' : 'kind' in value ? 'kind' : 'day' in value ? 'day' : 'along' in value ? 'along' : 'here' in value ? 'here' : value.plan);
    function choose(mode: string) {
        naming = false;
        if (mode === 'name') naming = true;
        else if (mode === 'kind') onChange({kind:'campsite'});
        else if (mode === 'day') onChange({day:days[0] ?? 1,part:'end'});
        else if (mode === 'along') onChange({along:{ref:'km',at:{value:10,unit:'km'}}});
        else if (mode === 'here') onChange({here:true});
        else onChange({plan:mode as 'start' | 'end'});
    }
</script>
<label>Point reference<select value={naming ? 'name' : mode} onchange={e => choose(e.currentTarget.value)}>
    <option value="name">Named place</option><option value="kind">Nearest place of a type</option><option value="day">Day boundary</option><option value="along">Route position</option><option value="here">My location</option><option value="start">Route start</option><option value="end">Route end</option>
</select></label>
{#if naming || 'name' in value}<label>Place name<input value={'name' in value ? value.name : ''} onchange={e => { if(e.currentTarget.value.trim()) onChange({name:e.currentTarget.value.trim()}); }} /></label>
{:else if 'kind' in value}<label>Place type<select value={value.kind} onchange={e => onChange({kind:e.currentTarget.value})}>{#each allKinds as kind}<option value={kind}>{kindLabel(kind)}</option>{/each}</select></label>
{:else if 'day' in value}
    <label>Day<select value={String(value.day)} onchange={e => onChange({...value,day: /^\d+$/.test(e.currentTarget.value) ? Number(e.currentTarget.value) : e.currentTarget.value as QueryDay})}>
        {#each days as day}<option value={day}>Day {day}</option>{/each}<option value="today">Today</option><option value="tomorrow">Tomorrow</option>
    </select></label>
    <label>Part<select value={value.part ?? 'end'} onchange={e => onChange({...value,part:e.currentTarget.value as 'start' | 'middle' | 'end'})}><option value="start">Start</option><option value="middle">Middle</option><option value="end">End</option></select></label>
{:else if 'along' in value}<QueryAlong value={value.along} onChange={along => onChange({along})} />{/if}
