<script lang="ts">
    import type { Along, Quantity } from '../../lib/planner/search/types';
    let { value, onChange }: { value: Along; onChange: (value: Along) => void } = $props();
    function number(key: 'at' | 'from' | 'to', n: number) {
        if (Number.isFinite(n) && n >= 0) onChange({ ...value, [key]: { value: n, unit: value[key]?.unit ?? 'km' } });
    }
</script>
<label>Measure from<select value={value.ref} onchange={e => onChange({ ...value, ref: e.currentTarget.value as Along['ref'] })}>
    <option value="km">Route start (absolute)</option><option value="start">Section start</option><option value="end">Section end</option><option value="here">My position on route</option>
</select></label>
<label>Position or interval<select value={value.at ? 'at' : 'interval'} onchange={e => onChange(e.currentTarget.value === 'at' ? { ref:value.ref, at:value.from ?? {value:0,unit:'km'} } : {ref:value.ref,from:{value:0,unit:'km'},to:value.at ?? {value:10,unit:'km'}})}><option value="at">At a position</option><option value="interval">An interval</option></select></label>
{#each (value.at ? ['at'] : ['from','to']) as key}
    {@const q = value[key as 'at' | 'from' | 'to']}
    <label class="half">{key === 'at' ? 'Position' : key === 'from' ? 'From' : 'To'}<input type="number" min="0" step="0.1" value={q?.value ?? ''} placeholder={key === 'to' ? 'Section end' : '0'} onchange={e => number(key as 'at' | 'from' | 'to',e.currentTarget.valueAsNumber)} /></label>
    <label class="half">Unit<select value={q?.unit ?? 'km'} onchange={e => onChange({ ...value, [key]:{value:q?.value ?? 0,unit:e.currentTarget.value as Quantity['unit']} })}><option value="km">km</option><option value="h">riding hours</option></select></label>
{/each}
