<script lang="ts">
    import SkinColorControl from "./SkinColorControl.svelte";

    let {
        value,
        label,
        onchange,
    }: {
        value: number | null;
        label: string;
        onchange: (value: number | null) => void;
    } = $props();
</script>

{#if value === null}
    <button type="button" class="none" title={`Add ${label}`} aria-label={`Add ${label}`} onclick={() => onchange(0)}>
        <span aria-hidden="true"></span>
        none
    </button>
{:else}
    <span class="set">
        <SkinColorControl {value} {label} {onchange} />
        <button type="button" class="clear" title={`Remove ${label}`} aria-label={`Remove ${label}`} onclick={() => onchange(null)}>×</button>
    </span>
{/if}

<style>
    .none,
    .set {
        display: inline-flex;
        align-items: center;
        gap: 4px;
    }

    .none {
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink-faint);
        font-size: 11px;
    }

    .none span {
        width: 30px;
        height: 24px;
        border: 1px dashed var(--line-strong);
        border-radius: 6px;
        background: linear-gradient(to top right, transparent 47%, var(--line-strong) 49%, var(--line-strong) 51%, transparent 53%);
    }

    .clear {
        padding: 0 2px;
        border: 0;
        background: none;
        color: var(--ink-faint);
        font-size: 17px;
    }

    .clear:hover {
        color: var(--coral);
    }
</style>
