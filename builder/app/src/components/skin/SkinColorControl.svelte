<script lang="ts">
    import { rgb565ToDeviceHex, rgb565ToHex, hexToRgb565, parseRgb565 } from "../../lib/color/rgb565";

    let {
        value,
        label,
        onchange,
    }: {
        value: number;
        label: string;
        onchange: (value: number) => void;
    } = $props();

    const LEVELS = [0, 85, 170, 255] as const;
    const palette = LEVELS.flatMap((r) =>
        LEVELS.flatMap((g) => LEVELS.map((b) => `#${[r, g, b].map((v) => v.toString(16).padStart(2, "0")).join("")}`)),
    );

    let open = $state(false);
    let anchor = $state<HTMLButtonElement>();
    let pop = $state<HTMLDivElement>();
    let pos = $state({ left: 0, top: 0 });

    function toggle() {
        open = !open;
        if (open && anchor) {
            const rect = anchor.getBoundingClientRect();
            pos = { left: rect.left, top: rect.bottom + 6 };
        }
    }

    $effect(() => {
        if (!open || !anchor || !pop) return;
        const rect = anchor.getBoundingClientRect();
        const left = Math.max(8, Math.min(rect.left, window.innerWidth - pop.offsetWidth - 8));
        let top = rect.bottom + 6;
        if (top + pop.offsetHeight > window.innerHeight - 8) top = Math.max(8, rect.top - pop.offsetHeight - 6);
        pos = { left, top };

        const outside = (event: MouseEvent) => {
            if (!pop?.contains(event.target as Node) && !anchor?.contains(event.target as Node)) open = false;
        };
        const key = (event: KeyboardEvent) => {
            if (event.key === "Escape") open = false;
        };
        document.addEventListener("mousedown", outside);
        document.addEventListener("keydown", key);
        return () => {
            document.removeEventListener("mousedown", outside);
            document.removeEventListener("keydown", key);
        };
    });

    function pick(hex: string, keepOpen = false) {
        onchange(parseRgb565(hexToRgb565(hex)));
        if (!keepOpen) open = false;
    }
</script>

<button
    type="button"
    class="swatch"
    bind:this={anchor}
    style:background={rgb565ToDeviceHex(value)}
    title={`${label}: ${rgb565ToDeviceHex(value)}`}
    aria-label={`Edit ${label}`}
    onclick={toggle}
></button>

{#if open}
    <div class="popover" bind:this={pop} style:left="{pos.left}px" style:top="{pos.top}px">
        <div class="small faint">Device palette</div>
        <div class="palette">
            {#each palette as hex (hex)}
                <button
                    type="button"
                    class="cell"
                    class:current={hex === rgb565ToDeviceHex(value)}
                    style:background={hex}
                    title={hex}
                    aria-label={`${label} ${hex}`}
                    onclick={() => pick(hex)}
                ></button>
            {/each}
        </div>
        <label class="custom small">
            Custom
            <input type="color" value={rgb565ToHex(value)} oninput={(event) => pick(event.currentTarget.value, true)} />
            <span class="shown" style:background={rgb565ToDeviceHex(value)}></span>
        </label>
    </div>
{/if}

<style>
    .swatch {
        width: 30px;
        height: 24px;
        padding: 0;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
    }

    .popover {
        position: fixed;
        z-index: 2600;
        width: 220px;
        padding: 10px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        box-shadow: 0 12px 32px rgba(36, 51, 28, 0.24);
    }

    .palette {
        display: grid;
        grid-template-columns: repeat(8, 1fr);
        gap: 3px;
        margin-top: 6px;
    }

    .cell {
        aspect-ratio: 1;
        min-width: 0;
        padding: 0;
        border: 1px solid rgba(36, 51, 28, 0.2);
        border-radius: 4px;
    }

    .cell.current {
        outline: 2px solid var(--forest);
        outline-offset: 1px;
    }

    .custom {
        display: flex;
        align-items: center;
        gap: 7px;
        margin-top: 10px;
        color: var(--ink-faint);
    }

    .custom input {
        width: 38px;
        height: 26px;
        padding: 0;
        border: 1px solid var(--line);
        border-radius: 5px;
        background: transparent;
    }

    .shown {
        width: 20px;
        height: 20px;
        border: 1px solid var(--line-strong);
        border-radius: 5px;
    }
</style>
