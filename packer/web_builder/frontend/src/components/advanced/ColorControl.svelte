<script lang="ts">
    import { onMount } from "svelte";
    import { getPalette, type Palette } from "../../lib/api/palette";
    import { formatRgb565, hexToRgb565, parseRgb565, rgb565ToDeviceHex, rgb565ToHex } from "../../lib/color/rgb565";

    let {
        value,
        onchange,
        showLabel = true,
    }: {
        value: string | number;
        onchange: (v: string) => void;
        showLabel?: boolean;
    } = $props();

    let palette = $state<Palette>({ columns: 8, colors: [] });
    let open = $state(false);
    let pos = $state({ left: 0, top: 0 });
    let anchor: HTMLButtonElement;
    let pop = $state<HTMLDivElement | null>(null);

    const canonical = $derived(formatRgb565(parseRgb565(value)));
    const deviceHex = $derived(rgb565ToDeviceHex(value));

    onMount(async () => {
        palette = await getPalette();
    });

    function toggle() {
        if (open) {
            open = false;
            return;
        }
        const r = anchor.getBoundingClientRect();
        pos = { left: r.left, top: r.bottom + 6 };
        open = true;
    }

    // Clamp into the viewport once the popover has a size (flip above if needed).
    $effect(() => {
        if (!open || !pop) return;
        const r = anchor.getBoundingClientRect();
        const pw = pop.offsetWidth;
        const ph = pop.offsetHeight;
        let left = Math.max(8, Math.min(r.left, window.innerWidth - 8 - pw));
        let top = r.bottom + 6;
        if (top + ph > window.innerHeight - 8) top = Math.max(8, r.top - 6 - ph);
        pos = { left, top };

        const onDocDown = (ev: MouseEvent) => {
            if (!pop?.contains(ev.target as Node) && !anchor.contains(ev.target as Node)) open = false;
        };
        const onKey = (ev: KeyboardEvent) => {
            if (ev.key === "Escape") open = false;
        };
        const close = () => (open = false);
        document.addEventListener("mousedown", onDocDown);
        document.addEventListener("keydown", onKey);
        window.addEventListener("resize", close);
        document.addEventListener("scroll", close, { passive: true, capture: true });
        return () => {
            document.removeEventListener("mousedown", onDocDown);
            document.removeEventListener("keydown", onKey);
            window.removeEventListener("resize", close);
            document.removeEventListener("scroll", close, { capture: true });
        };
    });

    function pick(v: string, keepOpen = false) {
        onchange(v);
        if (!keepOpen) open = false;
    }
</script>

<span class="control">
    <button
        type="button"
        class="swatch"
        bind:this={anchor}
        style:background={deviceHex}
        title="Pick a color"
        aria-label="Pick a color"
        onclick={toggle}
    ></button>
    {#if showLabel}
        <span class="mono small muted">{canonical}</span>
    {/if}
</span>

{#if open}
    <div class="popover" bind:this={pop} style:left="{pos.left}px" style:top="{pos.top}px">
        <div class="title small muted">Device palette</div>
        <div class="grid" style:grid-template-columns="repeat({palette.columns}, 1fr)">
            {#each palette.colors as hex (hex)}
                <button
                    type="button"
                    class="cell"
                    class:current={hex.toUpperCase() === deviceHex.toUpperCase()}
                    style:background={hex}
                    title="{hex} · {hexToRgb565(hex)}"
                    aria-label="Palette color {hex}"
                    onclick={() => pick(hexToRgb565(hex))}
                ></button>
            {/each}
        </div>
        <div class="custom">
            <span class="small muted">Custom</span>
            <input
                type="color"
                value={rgb565ToHex(value)}
                oninput={(e) => pick(hexToRgb565(e.currentTarget.value), true)}
            />
            <span class="small muted">→ device</span>
            <span class="preview" style:background={deviceHex} title="How the device shows this color"
            ></span>
        </div>
    </div>
{/if}

<style>
    .control {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }

    .swatch {
        width: 20px;
        height: 20px;
        border: 1px solid var(--line-strong);
        border-radius: 5px;
        padding: 0;
    }

    .popover {
        position: fixed;
        z-index: 2000;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        box-shadow: 0 10px 28px rgba(36, 51, 28, 0.24);
        padding: 10px;
        width: 224px;
    }

    .title {
        margin-bottom: 6px;
    }

    .grid {
        display: grid;
        gap: 3px;
    }

    .cell {
        aspect-ratio: 1;
        border: 1px solid rgba(36, 51, 28, 0.18);
        border-radius: 4px;
        padding: 0;
    }

    .cell.current {
        outline: 2px solid var(--forest);
        outline-offset: 1px;
    }

    .custom {
        display: flex;
        align-items: center;
        gap: 6px;
        margin-top: 9px;
    }

    .custom input[type="color"] {
        width: 34px;
        height: 24px;
        padding: 0;
        border: 1px solid var(--parchment-3);
        border-radius: 5px;
        background: none;
    }

    .preview {
        width: 18px;
        height: 18px;
        border-radius: 5px;
        border: 1px solid var(--line-strong);
    }
</style>
