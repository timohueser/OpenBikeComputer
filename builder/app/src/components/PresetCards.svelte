<script lang="ts">
    import type { BuildablePreset } from "../lib/config/model";
    import { working } from "../lib/config/storage.svelte";
    import { presetTagline } from "../lib/preview/copy";
    import Gated from "./Gated.svelte";
    import PresetPreview from "./PresetPreview.svelte";
    import { ADVANCED_ROUTE } from "../lib/routes";

    // Written once so the live link and its dead twin can't drift apart.
    const FINE_TUNE = "Fine-tune colors, features and detail levels in the advanced editor";

    // Config-carrying presets only: picking one here *is* copying its config
    // into the working copy, so a catalog preset (which names a baked artifact
    // and has no config) has no meaning in this card. The hosted tier renders
    // components/catalog/PresetStep.svelte instead.
    let { presets }: { presets: BuildablePreset[] } = $props();

    const basedOn = $derived(working.envelope?.based_on?.id ?? null);
    const modified = $derived(working.envelope?.modified ?? false);
    const basedOnName = $derived(presets.find((p) => p.id === basedOn)?.name ?? basedOn);

    function pick(preset: BuildablePreset) {
        if (modified && basedOn === preset.id) return; // keep custom edits; reset lives in the editor
        working.applyPreset(preset);
    }
</script>

<div class="cards">
    {#each presets as preset (preset.id)}
        <div class="card" class:selected={basedOn === preset.id}>
            <!-- Same shape as the catalog tier's card: the selected preview is live, so it must
                 not sit inside a button (a drag inside one is a click); every other picture is
                 the card's main hit target. -->
            {#if basedOn === preset.id}
                <PresetPreview presetId={preset.id} label={preset.name} interactive />
            {:else}
                <button
                    type="button"
                    class="shot"
                    aria-label={`Choose ${preset.name}`}
                    onclick={() => pick(preset)}
                >
                    <PresetPreview presetId={preset.id} label={preset.name} />
                </button>
            {/if}
            <button type="button" class="pick" onclick={() => pick(preset)}>
                <span class="name">
                    {preset.name}
                    {#if basedOn === preset.id && !modified}<span class="check">✓</span>{/if}
                </span>
                <span class="swatches">
                    {#each preset.swatch ?? [] as c (c)}
                        <span class="sw" style:background={c}></span>
                    {/each}
                </span>
                <span class="desc small muted">{presetTagline(preset.id, preset.description)}</span>
            </button>
        </div>
    {/each}
</div>

{#if modified}
    <div class="custom small">
        <span class="badge">{basedOnName ? `Custom — based on ${basedOnName}` : "Custom"}</span>
        <span class="muted">your edits are kept in this browser</span>
    </div>
{/if}

<div class="advanced small">
    <Gated need="styleEditor">
        <a href={ADVANCED_ROUTE}>{FINE_TUNE} →</a>
        <!-- A plain span, not the <a>: there is nothing to follow, so the
             stand-in must not be focusable. -->
        {#snippet unavailable(reason)}
            <span aria-describedby={reason}>{FINE_TUNE}</span>
        {/snippet}
    </Gated>
</div>

<style>
    .cards {
        display: grid;
        /* Wider than the old text-only cards: these hold a 3:4 portrait picture, and below
           ~160 px the panel's own hairlines stop resolving. */
        grid-template-columns: repeat(auto-fit, minmax(168px, 1fr));
        gap: 10px;
    }

    .card {
        display: flex;
        flex-direction: column;
        align-items: stretch;
        gap: 8px;
        text-align: left;
        background: var(--parchment);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        padding: 11px 12px;
        transition:
            border-color 0.15s,
            box-shadow 0.15s;
    }

    .card:hover {
        border-color: var(--wood);
    }

    .card.selected {
        border: 2px solid var(--forest);
        padding: 10px 11px;
        box-shadow: 0 2px 10px rgba(60, 107, 57, 0.16);
    }

    .shot {
        display: block;
        background: none;
        border: none;
        padding: 0;
        border-radius: 8px;
    }

    .shot:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }

    .pick {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 5px;
        text-align: left;
        background: none;
        border: none;
        padding: 0;
    }

    .name {
        font-weight: 600;
        font-size: 14px;
        color: var(--ink);
    }

    .check {
        color: var(--forest);
        margin-left: 4px;
    }

    .swatches {
        display: flex;
        gap: 4px;
    }

    .sw {
        width: 13px;
        height: 13px;
        border-radius: 4px;
        border: 1px solid var(--line-strong);
    }

    .desc {
        line-height: 1.35;
    }

    .custom {
        margin-top: 10px;
        display: flex;
        align-items: center;
        gap: 8px;
        flex-wrap: wrap;
    }

    .badge {
        background: rgba(227, 173, 51, 0.28);
        border: 1px solid var(--amber);
        border-radius: 999px;
        padding: 2px 10px;
        color: var(--ink);
        font-weight: 600;
    }

    .advanced {
        margin-top: 10px;
    }
</style>
