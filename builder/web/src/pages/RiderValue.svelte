<script lang="ts">
    import type { RiderValue } from '../lib/settings/settings.svelte';
    import type { Zone } from '../lib/settings/zones';

    let { id, label, unit, value, range, zones, ramp, onSet }: {
        id: string;
        label: string;
        unit: string;
        value: RiderValue | null;
        range: { min: number; max: number };
        zones: (limit: number) => Zone[];
        ramp: 'hr' | 'power';
        onSet: (value: number | null) => void;
    } = $props();

    // The value while the rider types; the store gets it on `change`.
    let draft = $state<number | null>(null);
    let field: HTMLInputElement;
    const shown = $derived(draft ?? value?.value ?? null);
    const day = (at: number) => new Date(at * 1000).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
    const list = $derived(shown === null ? [] : zones(shown).map((zone, i, all) => ({
        ...zone,
        range: i === 0 ? `≤ ${zone.to}` : i === all.length - 1 ? `≥ ${zone.from}` : `${zone.from}–${zone.to}`,
    })));

    function typed() {
        const n = Number(field.value);
        draft = field.value !== '' && !field.validity.badInput && n >= range.min && n <= range.max ? Math.round(n) : null;
    }

    // An entry that the browser cannot read as a number also has an empty value: it keeps the stored value.
    function changed() {
        draft = null;
        if (!field.validity.badInput) {
            const n = Number(field.value);
            onSet(field.value === '' ? null : Math.min(Math.max(Math.round(n), range.min), range.max));
        }
        field.value = String(value?.value ?? '');
    }
</script>

<!-- A click anywhere on the value focuses its field; the keyboard reaches the field with Tab. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="block" onclick={event => { if (event.target !== field) field.focus(); }}>
    <div class="head">
        <div>
            <label for={id}><strong>{label}</strong></label>
            {#if value?.device}<p id={`${id}-from`}>From the OpenBikeComputer, with the ride of {day(value.at)}</p>{/if}
        </div>
        <span class="value">
            <input bind:this={field} {id} type="number" inputmode="numeric" min={range.min} max={range.max} placeholder="–"
                aria-describedby={[`${id}-unit`, value?.device ? `${id}-from` : ''].join(' ').trim()}
                value={shown ?? ''} oninput={typed} onchange={changed} />
            <span class="unit" id={`${id}-unit`}>{unit}</span>
        </span>
    </div>
    {#if shown === null}
        <div class="zones empty"><span></span></div>
        <p>Set a value to see your zones.</p>
    {:else}
        <ul class="zones" aria-label={`${label} zones`}>
            {#each list as zone, i (zone.name)}
                <li style:--zone={`var(--${ramp}-${i + 1})`}><span class="bar"></span><small><b>{zone.name}</b> {zone.range}</small></li>
            {/each}
        </ul>
    {/if}
</div>

<style>
    .block {
        padding: 12px 0;
        border-top: 1px solid var(--line);
        cursor: text;
        /* The zone ramps of the ride board: heart rate warm, power cool. */
        --hr-1: #e9e4d4; --hr-2: #e8b06d; --hr-3: #d0752a; --hr-4: #a24f1f; --hr-5: #5e2a10;
        --power-1: #e1e8e2; --power-2: #b9d4d8; --power-3: #6e9ea3; --power-4: #2f5559; --power-5: #1b3133;
    }
    :global([data-theme="dark"]) .block {
        --hr-1: #4a4232; --hr-2: #8a5a2c; --hr-3: #c46d2a; --hr-4: #e8954a; --hr-5: #f6c48a;
        --power-1: #33403e; --power-2: #3e6a6e; --power-3: #5b9a9f; --power-4: #8fc4c8; --power-5: #c4e3e5;
    }
    .head {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
    }
    label {
        cursor: text;
    }
    p {
        margin: 2px 0 0;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .value {
        display: flex;
        flex: none;
        align-items: baseline;
        gap: 6px;
    }
    .unit {
        color: var(--ink-soft);
        font-size: 13px;
    }
    /* A thin line under the number says that it is a field; focus gives it the focus colour of the product. */
    input {
        width: 3.4ch;
        padding: 0 1px;
        border: 0;
        border-bottom: 1px solid var(--line-strong);
        border-radius: 0;
        background: none;
        color: var(--ink);
        font: 700 26px/1.15 var(--sans);
        font-variant-numeric: tabular-nums;
        text-align: right;
        appearance: textfield;
    }
    input::-webkit-inner-spin-button,
    input::-webkit-outer-spin-button {
        appearance: none;
        margin: 0;
    }
    input::placeholder {
        color: var(--ink-faint);
    }
    input:hover {
        border-bottom-color: var(--ink-faint);
    }
    input:focus,
    input:focus-visible {
        outline: none;
        border-bottom: 2px solid var(--forest);
        margin-bottom: -1px;
    }
    .zones {
        display: flex;
        gap: 2px;
        margin: 10px 0 0;
        padding: 0;
        list-style: none;
    }
    .zones li {
        flex: 1 1 0;
        min-width: 0;
    }
    .bar, .empty span {
        display: block;
        height: 10px;
        background: var(--zone);
    }
    .zones li:first-child .bar { border-radius: 3px 0 0 3px; }
    .zones li:last-child .bar { border-radius: 0 3px 3px 0; }
    .empty span {
        flex: 1;
        border-radius: 3px;
        background: var(--parchment-2);
    }
    .empty + p {
        margin-top: 6px;
    }
    small {
        display: block;
        margin-top: 4px;
        overflow: hidden;
        color: var(--ink-soft);
        font-size: 11px;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
        text-overflow: ellipsis;
    }
    small b {
        display: block;
        color: var(--ink);
        font-weight: 700;
    }
</style>
