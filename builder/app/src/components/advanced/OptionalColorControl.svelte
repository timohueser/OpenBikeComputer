<script lang="ts">
    // An OPTIONAL color (v10 `color2`): the primary ColorControl plus an
    // explicit unset state. Unset ⇒ onchange(undefined) ⇒ the caller deletes
    // the key from the config — NOT null, NOT "0x0000". Black is a legit color
    // (rails are black); the packer treats *absence* of the key as "no
    // secondary color", so the affordance must distinguish "black" from "none".
    // Setting seeds a neutral color so ColorControl (and its RGB222-quantized
    // picker) appears; clearing removes the key again.
    import ColorControl from "./ColorControl.svelte";

    let {
        value,
        onchange,
    }: {
        value: string | number | null | undefined;
        onchange: (v: string | undefined) => void;
    } = $props();

    // Absent/null ⇒ unset. An empty string is treated as unset too (defensive).
    const isSet = $derived(value != null && value !== "");

    // Where the picker starts when the user opts in; they pick from here.
    const SEED = "0x0000";
</script>

{#if isSet}
    <span class="opt">
        <ColorControl value={value as string | number} showLabel={false} onchange={(v) => onchange(v)} />
        <button
            type="button"
            class="clear"
            title="Clear secondary color (removes it entirely)"
            aria-label="Clear secondary color"
            onclick={() => onchange(undefined)}>×</button
        >
    </span>
{:else}
    <button
        type="button"
        class="set"
        title="Add a secondary color (casing / railway stripe / polygon outline)"
        aria-label="Set secondary color"
        onclick={() => onchange(SEED)}
    >
        <span class="empty" aria-hidden="true"></span>
        <span class="small muted">none</span>
    </button>
{/if}

<style>
    .opt {
        display: inline-flex;
        align-items: center;
        gap: 4px;
    }

    .clear {
        background: none;
        border: none;
        color: var(--ink-faint);
        font-size: 15px;
        line-height: 1;
        padding: 0;
        cursor: pointer;
    }

    .clear:hover {
        color: var(--coral);
    }

    .set {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        background: none;
        border: none;
        padding: 0;
        cursor: pointer;
    }

    /* Mirror ColorControl's 20px swatch so the set/unset states line up. */
    .empty {
        width: 20px;
        height: 20px;
        border: 1px dashed var(--line-strong);
        border-radius: 5px;
        background:
            linear-gradient(
                to top right,
                transparent calc(50% - 1px),
                var(--line-strong),
                transparent calc(50% + 1px)
            );
    }

    .set:hover .empty {
        border-color: var(--forest);
    }
</style>
