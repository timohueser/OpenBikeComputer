<script lang="ts">
    // One multiplier cell in a profile grid: a numeric input floored at the
    // schema minimum (>= 1.0) plus a per-cell "forbidden" toggle. A cell is
    // either explicit (its own value) or inheriting the profile default — the
    // latter renders muted. Valid values commit live while typing; an invalid
    // entry (sub-minimum, empty, NaN) never reaches the model — it's reverted
    // on blur/Enter with the hint from checkMultiplier (the single copy of
    // the packer-mirroring admissibility message).
    import type { Multiplier } from "../../lib/config/model";
    import { checkMultiplier } from "../../lib/config/profiles";

    let {
        value,
        explicit,
        min,
        label,
        onset,
        onclear,
        onhint,
    }: {
        value: Multiplier;
        explicit: boolean;
        min: number;
        label: string;
        onset: (v: Multiplier) => void;
        onclear: () => void;
        onhint: (hint: string | null) => void;
    } = $props();

    const forbidden = $derived(value === "forbidden");

    /** Live path (per keystroke): commit a valid value as it's typed; leave
     * anything else alone so an in-progress edit (a cleared field, "0." mid
     * entry) isn't stomped — the settle path below deals with it. */
    function liveCommit(e: Event & { currentTarget: HTMLInputElement }) {
        const v = parseFloat(e.currentTarget.value);
        if (checkMultiplier(v, min).ok) {
            onhint(null);
            onset(v);
        }
    }

    /** Settle path (blur/Enter): an invalid entry raises the admissibility
     * hint and the field reverts to the model value, so a sub-minimum
     * multiplier can never stick. */
    function settle(e: Event & { currentTarget: HTMLInputElement }) {
        const v = parseFloat(e.currentTarget.value);
        const { ok, hint } = checkMultiplier(v, min);
        if (!ok) {
            onhint(`“${label}”: ${hint}`);
            e.currentTarget.value = String(value);
        }
    }
</script>

<div class="cell" class:inherit={!explicit} class:forbidden title={label}>
    {#if forbidden}
        <button
            type="button"
            class="fbtn on"
            title="Forbidden — not routable under this profile. Click to allow."
            aria-label="{label}: forbidden — click to allow"
            onclick={() => {
                onhint(null);
                onclear();
            }}>forbid</button
        >
    {:else}
        <input
            type="number"
            class="num"
            {min}
            step="0.05"
            value={typeof value === "number" ? value : min}
            title={explicit ? "Explicit multiplier" : "Inheriting the profile default"}
            oninput={liveCommit}
            onchange={settle}
        />
        <button
            type="button"
            class="fbtn"
            title="Forbid this class (not routable)"
            aria-label="Forbid {label}"
            onclick={() => {
                onhint(null);
                onset("forbidden");
            }}>∅</button
        >
        {#if explicit}
            <button
                type="button"
                class="revert"
                title="Reset to the profile default"
                aria-label="Reset {label} to the profile default"
                onclick={() => {
                    onhint(null);
                    onclear();
                }}>↺</button
            >
        {/if}
    {/if}
</div>

<style>
    .cell {
        display: flex;
        align-items: center;
        gap: 2px;
    }

    .num {
        width: 46px;
        padding: 2px 4px;
        font-size: 12px;
        border-radius: 5px;
        text-align: right;
    }

    .cell.inherit .num {
        color: var(--ink-faint);
        font-style: italic;
    }

    .fbtn {
        background: none;
        border: 1px solid var(--line);
        border-radius: 5px;
        color: var(--ink-faint);
        font-size: 11px;
        line-height: 1;
        padding: 2px 4px;
        cursor: pointer;
    }

    .fbtn.on {
        background: rgba(226, 110, 90, 0.16);
        border-color: var(--coral);
        color: var(--coral);
        font-size: 10.5px;
        letter-spacing: 0.2px;
    }

    .fbtn:hover {
        color: var(--coral);
        border-color: var(--coral);
    }

    .revert {
        background: none;
        border: none;
        color: var(--ink-faint);
        font-size: 12px;
        padding: 0 1px;
        cursor: pointer;
    }

    .revert:hover {
        color: var(--forest);
    }
</style>
