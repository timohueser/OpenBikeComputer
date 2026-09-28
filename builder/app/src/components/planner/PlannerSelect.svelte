<script lang="ts">
    import Icon from './PlannerIcon.svelte';

    let { label, value, options, onChange }: {
        label: string;
        value: string;
        options: { value: string; label: string }[];
        onChange: (value: string) => void;
    } = $props();

    const id = $props.id();
    let root: HTMLDivElement;
    let open = $state(false);
    let active = $state(0);
    let prefix = '';
    let typedAt = 0;
    const selected = $derived(options.findIndex(option => option.value === value));

    function show() {
        active = Math.max(0, selected);
        prefix = '';
        open = true;
    }

    function choose(index: number) {
        open = false;
        if (options[index].value !== value) onChange(options[index].value);
    }

    function keydown(event: KeyboardEvent) {
        if (event.key === 'Tab' || event.key === 'Escape') {
            if (open && event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); }
            open = false;
            return;
        }
        if (['ArrowDown', 'ArrowUp', 'Home', 'End', 'Enter', ' '].includes(event.key)) {
            event.preventDefault();
            if (!open) { show(); return; }
            if (event.key === 'Enter' || event.key === ' ') choose(active);
            else if (event.key === 'Home') active = 0;
            else if (event.key === 'End') active = options.length - 1;
            else active = Math.max(0, Math.min(options.length - 1, active + (event.key === 'ArrowDown' ? 1 : -1)));
        } else if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
            event.preventDefault();
            if (!open) show();
            prefix = (Date.now() - typedAt < 700 ? prefix : '') + event.key.toLowerCase();
            typedAt = Date.now();
            const match = options.findIndex(option => option.label.toLowerCase().startsWith(prefix));
            if (match >= 0) active = match;
        }
    }
</script>

<svelte:window onpointerdown={(event) => { if (!root.contains(event.target as Node)) open = false; }} onblur={() => open = false} />

<div class="selector" class:open bind:this={root}>
    <button type="button" class="trigger" role="combobox" aria-label={label} aria-expanded={open}
        aria-controls={`${id}-options`} aria-haspopup="listbox" aria-activedescendant={open ? `${id}-${active}` : undefined}
        onclick={() => open ? open = false : show()} onkeydown={keydown} onblur={() => open = false}>
        <span>{options[selected]?.label ?? value}</span><Icon name="down" size={14} />
    </button>
    {#if open}
        <div class="options" id={`${id}-options`} role="listbox" aria-label={label}>
            {#each options as option, index (option.value)}
                <button type="button" role="option" id={`${id}-${index}`} tabindex="-1" aria-selected={option.value === value}
                    class:active={index === active} onpointermove={() => active = index}
                    onmousedown={(event) => event.preventDefault()} onclick={() => choose(index)}>
                    <span>{option.label}</span>
                    {#if option.value === value}<Icon name="check" size={15} />{/if}
                </button>
            {/each}
        </div>
    {/if}
</div>

<style>
    .selector { position: relative; flex: none; min-width: 0; }
    .selector.open { z-index: 40; }
    button { font: 600 13px var(--sans); color: var(--ink); cursor: pointer; }
    .trigger { display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 32px; padding: 6px 9px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--panel); }
    .trigger:hover, .open .trigger { border-color: var(--ink-faint); background: var(--parchment-2); }
    .trigger > :global(svg) { flex: none; color: var(--ink-soft); }
    .options { position: absolute; top: calc(100% + 6px); left: 0; min-width: max(100%, 180px); padding: 4px; border: 1px solid var(--line-strong); border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); }
    .options button { display: flex; align-items: center; justify-content: space-between; gap: 20px; width: 100%; min-height: 36px; padding: 8px; border: 0; border-radius: 4px; background: transparent; text-align: left; white-space: nowrap; font-weight: 400; }
    .options button.active { background: var(--parchment-2); }
    .options button[aria-selected="true"] { font-weight: 600; }
    .options :global(svg) { color: var(--forest); }
</style>
