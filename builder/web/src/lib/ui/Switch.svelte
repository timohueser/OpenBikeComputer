<script lang="ts">
    /** `note` is a second line under the label. It describes the switch; the name of the switch is the label alone. */
    let { checked = $bindable(false), label, note }: { checked?: boolean; label: string; note?: string } = $props();
    const id = $props.id();
</script>

<button type="button" role="switch" aria-checked={checked} aria-labelledby={`${id}-label`} aria-describedby={note ? `${id}-note` : undefined}
    onclick={() => checked = !checked}>
    <span class="label"><span id={`${id}-label`}>{label}</span>{#if note}<small id={`${id}-note`}>{note}</small>{/if}</span>
    <span class="track" aria-hidden="true"><span class="knob"></span></span>
</button>

<style>
    button {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        width: 100%;
        min-height: 32px;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
        cursor: pointer;
    }
    small {
        display: block;
        color: var(--ink-soft);
        font-size: 13.5px;
        font-weight: 400;
    }
    .track {
        flex: none;
        width: 36px;
        height: 20px;
        padding: 2px;
        border-radius: 10px;
        background: var(--line-strong);
        transition: background 150ms;
    }
    .knob {
        display: block;
        width: 16px;
        height: 16px;
        border-radius: 50%;
        background: var(--panel);
        box-shadow: 0 1px 2px rgb(0 0 0 / 0.25);
        transition: transform 150ms;
    }
    [aria-checked="true"] .track { background: var(--forest); }
    [aria-checked="true"] .knob { transform: translateX(16px); }
    @media (prefers-reduced-motion: reduce) {
        .track, .knob { transition: none; }
    }
</style>
