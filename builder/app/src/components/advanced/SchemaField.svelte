<script lang="ts">
    // Generic editor for a style field known only from the served schema —
    // the forward-compatibility net: a knob added to obc-pack's schema shows
    // up here (typed input, bounds, enum options) before it gets bespoke UI.
    import OptionalColorControl from "./OptionalColorControl.svelte";
    import {
        enumDisplayValue,
        resolveSchemaField,
        stringEnumOptions,
        type JsonSchema,
    } from "../../lib/config/schema_fields";

    let {
        name,
        prop,
        schemaRoot,
        value,
        onchange,
    }: {
        name: string;
        prop: JsonSchema;
        schemaRoot: JsonSchema;
        value: unknown;
        onchange: (v: unknown) => void;
    } = $props();

    const resolved = $derived(resolveSchemaField(schemaRoot, prop));
    const field = $derived(resolved.schema);

    // Color-shaped fields (v10 `color2`, …) get the real picker. The primary
    // `color` is bespoke in StyleTable, so any color reaching SchemaField is a
    // secondary/optional one: render it via OptionalColorControl, which keeps
    // an explicit unset state (absence ⇒ key deleted, not coerced to black).
    // The schema may spell the color shape either as a $ref to $defs/color
    // (as `color2` does) or inline its string-or-int oneOf, so match on both.
    const isColor = $derived(
        name.startsWith("color") ||
            (typeof field.$ref === "string" && field.$ref.endsWith("/color")) ||
            (Array.isArray(field.oneOf) &&
                (field.oneOf as { pattern?: string }[]).some((o) => o.pattern?.includes("0[xX]"))),
    );
    const options = $derived(stringEnumOptions(field));
    const isNumber = $derived(field.type === "integer" || field.type === "number");
    const isBool = $derived(field.type === "boolean");
    const fallback = $derived(field.default);
</script>

{#if isColor}
    <OptionalColorControl
        value={value as string | number | null | undefined}
        onchange={(v) => onchange(v)}
    />
{:else if options}
    <select
        value={enumDisplayValue(value, field, options)}
        onchange={(e) => onchange(e.currentTarget.value)}
        title={String(field.description ?? name)}
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
        min={field.minimum as number | undefined}
        max={field.maximum as number | undefined}
        value={(value ?? fallback ?? 0) as number}
        oninput={(e) => {
            const v = parseFloat(e.currentTarget.value);
            if (Number.isFinite(v)) onchange(field.type === "integer" ? Math.trunc(v) : v);
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
