<!--
  One control, one requirement (#901). Where the requirement holds this renders
  the real thing; where it doesn't, it leaves a dead copy of the control on
  screen with the reason underneath — so the feature is discovered at the moment
  someone reaches for it, and the answer arrives with it.

  Nothing platform-specific lives here or in any caller: the requirement name is
  the whole interface, and `lib/platform/gating.ts` owns which ones hold and
  what each one says.

  Usage — a control backed by a platform member, checked once:

      <Gated need="build" value={platform.buildMap}>
          {#snippet children(buildMap)}<BuildCard {buildMap} />{/snippet}
          {#snippet unavailable()}<button class="btn primary" disabled>Build map</button>{/snippet}
      </Gated>

  Usage — a plain gate, tier first and the browser second, so each failure gets
  its own sentence:

      <Gated need={["deviceUsb", "webUsb"]}>…</Gated>

  `unavailable` is optional: omit it where there is no control worth showing
  dead and the reason line should simply take its place.
-->
<script lang="ts" generics="T">
    import type { Snippet } from "svelte";
    import { DESKTOP_LINK, GATES, unmet, type Requirement } from "../lib/platform/gating";

    let {
        need,
        value,
        children,
        unavailable,
    }: {
        /** The capability this control needs. An array is checked in order. */
        need: Requirement | readonly Requirement[];
        /** The platform member behind the control, handed to `children`. */
        value?: T | null;
        children?: Snippet<[T]>;
        /** The control, rendered inert. */
        unavailable?: Snippet;
    } = $props();

    const blocked = $derived(unmet(need, value));
</script>

{#if blocked}
    {#if unavailable}
        <!-- `inert` rather than trusting each stand-in to disable itself: it
             takes the whole subtree out of the tab order and the accessibility
             tree in one attribute, so what a screen reader reaches is the
             reason and its link. -->
        <div class="stand-in" inert>{@render unavailable()}</div>
    {/if}
    <p class="reason small">
        <span class="muted">{GATES[blocked].reason}</span>
        <a href={DESKTOP_LINK.href}>{DESKTOP_LINK.label}</a>
    </p>
{:else if children}
    {@render children(value as T)}
{/if}

<style>
    /* Faded rather than translucent: `.btn:disabled` already sets an opacity,
       and two stacked opacities land somewhere close to invisible. */
    .stand-in {
        color: var(--ink-faint);
    }

    .reason {
        margin: 8px 0 0;
        display: flex;
        flex-wrap: wrap;
        gap: 4px 8px;
    }
</style>
