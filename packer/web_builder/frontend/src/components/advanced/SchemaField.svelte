<script lang="ts">
    // Generic editor for a style field known only from the served schema —
    // the forward-compatibility net: a knob added to obc-pack's schema shows
    // up here (typed input, bounds, enum options) before it gets bespoke UI.
    import ColorControl from "./ColorControl.svelte";

    let {
        name,
        prop,
        value,
        onchange,
    }: {
        name: string;
        prop: Record<string, unknown>;
        value: unknown;
        onchange: (v: unknown) => void;
    } = $props();

    // Color-shaped fields (v6 `color2`, …) get the real picker: the schema
    // marks them with the shared color definition's string-or-int oneOf.
    const isColor = $derived(
        name.startsWith("color") ||
            (Array.isArray(prop.oneOf) &&
                (prop.oneOf as { pattern?: string }[]).some((o) => o.pattern?.includes("0[xX]"))),
    );
    const options = $derived(
        Array.isArray(prop.enum) ? (prop.enum as string[]) : null,
    );
    const isNumber = $derived(prop.type === "integer" || prop.type === "number");
    const isBool = $derived(prop.type === "boolean");
    const fallback = $derived(prop.default);
</script>

{#if isColor}
    <ColorControl
        value={(value ?? fallback ?? "0x0000") as string}
        showLabel={false}
        onchange={(v) => onchange(v)}
    />
{:else if options}
    <select
        value={(value ?? fallback ?? options[0]) as string}
        onchange={(e) => onchange(e.currentTarget.value)}
        title={String(prop.description ?? name)}
    >
        {#each options as opt (opt)}
            <option value={opt}>{opt}</option>
        {/each}
    </select>
{:else if isBool}
    <input
        type="checkbox"
        checked={Boolean(value ?? fallback)}
        onchange={(e) => onchange(e.currentTarget.checked)}
    />
{:else if isNumber}
    <input
        type="number"
        class="num"
        min={prop.minimum as number | undefined}
        max={prop.maximum as number | undefined}
        value={(value ?? fallback ?? 0) as number}
        oninput={(e) => {
            const v = parseFloat(e.currentTarget.value);
            if (Number.isFinite(v)) onchange(prop.type === "integer" ? Math.trunc(v) : v);
        }}
    />
{:else}
    <input
        type="text"
        class="num"
        value={String(value ?? fallback ?? "")}
        oninput={(e) => onchange(e.currentTarget.value)}
    />
{/if}

<style>
    select,
    .num {
        padding: 3px 6px;
        font-size: 12.5px;
        border-radius: 6px;
        max-width: 86px;
    }
</style>
