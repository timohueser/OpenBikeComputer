<script lang="ts">
    let { checked = $bindable(false), label }: { checked?: boolean; label: string } = $props();
</script>

<button type="button" role="switch" aria-checked={checked} onclick={() => checked = !checked}>
    <span class="label">{label}</span>
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
