<!-- Opens a night to choose where it is: "Choose" on a suggested night, a quiet "Change" on a night that the rider set. -->
<script lang="ts">
    let { night, set, onclick }: { night: number; set: boolean; onclick: () => void } = $props();
</script>

<button type="button" class={set ? 'change' : 'planner-action'} data-night={night} aria-label={`${set ? 'Change' : 'Choose'} night ${night}`} {onclick}>{set ? 'Change' : 'Choose'}</button>

<style>
    button.planner-action, .change { flex: none; min-height: 30px; font-size: 13px; }
    .change {
        padding: 0 6px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font-family: inherit;
        font-weight: 600;
        cursor: pointer;
    }
    .change:hover { color: var(--ink); }
    .change:focus-visible { outline: 2px solid var(--ink); outline-offset: -2px; }
</style>
