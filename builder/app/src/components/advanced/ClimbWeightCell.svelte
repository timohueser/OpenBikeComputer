<script lang="ts">
    // The per-profile climb-weight cell (OBCM v12 §8.6): flat metres charged per
    // metre of ascent, a whole number in the schema's 0..255. It follows the
    // multiplier cell's two-path editing exactly — valid values commit live while
    // typing, an invalid entry (out of range, fractional, empty) never reaches the
    // model and is reverted on blur/Enter with the hint from checkClimbWeight.
    //
    // What it does NOT share is "inherit": a class multiplier falls back to the
    // profile default, but an unstated climb weight simply *is* 0, climb-blind.
    // That state renders muted, and the revert button returns to it — so a config
    // written before v12 reads as the 0 the packer gives it, never as a blank.
    import { checkClimbWeight } from "../../lib/config/profiles";

    let {
        value,
        stated,
        min,
        max,
        label,
        onset,
        onclear,
        onhint,
    }: {
        value: number;
        stated: boolean;
        min: number;
        max: number;
        label: string;
        onset: (v: number) => void;
        onclear: () => void;
        onhint: (hint: string | null) => void;
    } = $props();

    /** Live path (per keystroke): commit a valid value as it's typed; leave
     * anything else alone so an in-progress edit isn't stomped. */
    function liveCommit(e: Event & { currentTarget: HTMLInputElement }) {
        const v = parseFloat(e.currentTarget.value);
        if (checkClimbWeight(v, min, max).ok) {
            onhint(null);
            onset(v);
        }
    }

    /** Settle path (blur/Enter): an invalid entry raises the hint and the field
     * reverts to the model value, so an out-of-range weight can never stick. */
    function settle(e: Event & { currentTarget: HTMLInputElement }) {
        const v = parseFloat(e.currentTarget.value);
        const { ok, hint } = checkClimbWeight(v, min, max);
        if (!ok) {
            onhint(`“${label}”: ${hint}`);
            e.currentTarget.value = String(value);
        }
    }
</script>

<div class="cell" class:unstated={!stated} title={label}>
    <input
        type="number"
        class="num"
        {min}
        {max}
        step="1"
        {value}
        aria-label={label}
        title={stated
            ? "Flat metres charged per metre of ascent"
            : `Climb-blind (${min}) — this profile states no climb weight`}
        oninput={liveCommit}
        onchange={settle}
    />
    {#if stated}
        <button
            type="button"
            class="revert"
            title="Make this profile climb-blind"
            aria-label="Reset {label} to climb-blind"
            onclick={() => {
                onhint(null);
                onclear();
            }}>↺</button
        >
    {/if}
</div>

<style>
    .cell {
        display: flex;
        align-items: center;
        gap: 2px;
    }

    .num {
        width: 52px;
        padding: 2px 4px;
        font-size: 12px;
        border-radius: 5px;
        text-align: right;
    }

    .cell.unstated .num {
        color: var(--ink-faint);
        font-style: italic;
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
