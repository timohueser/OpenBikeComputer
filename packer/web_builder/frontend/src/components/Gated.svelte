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
          {#snippet unavailable(reason)}
              <button class="btn primary" disabled aria-describedby={reason}>Build map</button>
          {/snippet}
      </Gated>

  Usage — a plain gate, tier first and the browser second, so each failure gets
  its own sentence:

      <Gated need={["deviceUsb", "webUsb"]}>…</Gated>

  `unavailable` is optional: omit it where there is no control worth showing
  dead and the reason line should simply take its place.

  **What a stand-in must be.** The same reach that makes gating work for a
  sighted person has to work for a screen reader, so the stand-in stays in the
  accessibility tree and carries its own state:

    * Prefer a **natively disable-able control** — `button`, `input`, `select`,
      `textarea`, `fieldset` — with `disabled`. That alone takes it out of the
      tab order *and* gets it announced as unavailable, which nothing we could
      write by hand does as well.
    * Apply `aria-describedby={reason}` to that control. The snippet's argument
      is the id of this instance's reason sentence (unique per instance, so
      several gates on one page don't collide), and the pairing is what turns
      two adjacent things on screen into one utterance: *"Build map,
      unavailable — Maps are built on your own machine."*
    * A stand-in that **isn't** natively disable-able must still be
      unfocusable — a plain `<span>` rather than an `<a href>`, or failing that
      `tabindex="-1"` with `aria-disabled="true"`. Nothing here will do it for
      you: `inert` on the wrapper would, but it also hides the control from
      assistive tech entirely, which is the thing this is deliberately not
      doing.

  The reason's link stays outside the association on purpose. It is a real,
  focusable link and should be reached as one — folding it into the description
  would flatten it into recited text and leave the next step unreachable by
  keyboard navigation.
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
        /** The control, disabled. Its argument is the reason sentence's id —
         *  put it on `aria-describedby`. */
        unavailable?: Snippet<[string]>;
    } = $props();

    const blocked = $derived(unmet(need, value));
    // Per instance, so two gates in one column describe their own controls.
    // `$props.id()` has to be a declaration initializer on its own, hence two
    // lines; the prefix is only there to make the markup readable.
    const uid = $props.id();
    const reasonId = `gate-reason-${uid}`;
</script>

{#if blocked}
    {#if unavailable}
        <div class="stand-in">{@render unavailable(reasonId)}</div>
    {/if}
    <p class="reason small">
        <!-- The id sits on the sentence rather than the paragraph: a
             description is read as flat text, so pointing at the whole
             paragraph would recite the link's label as prose *and* still leave
             the link to be found separately. -->
        <span id={reasonId} class="muted">{GATES[blocked].reason}</span>
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
