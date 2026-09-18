<script lang="ts">
  // One pill per test kind. Every pill opens the same panel, which lists all the kinds, because a
  // level pill and a manual pill look alike and answer different questions. A press opens and
  // closes the panel; a press outside it, the Escape key, and the close button also close it. It
  // does not open on hover: the list is long enough to read and to scroll, which a panel that
  // leaves with the pointer cannot be.
  //
  // `$lib/popover` places the panel against the window; see it for why CSS alone cannot.
  import { tick } from 'svelte';
  import { portal, place, sizing, widthFor } from '$lib/popover';
  import { TEST_KIND_NOTES } from '$lib/types';
  /** `unit`, `integration`, `system`, `human`, `until-automated`, or `manual` when the type of a
   *  manual test is not set. */
  export let kind: string;
  $: text = kind === 'human' ? 'human check' : kind === 'until-automated' ? 'until automated' : kind;

  /** The panel is this wide unless the window is narrower. */
  const WIDTH = 330;
  let holder: HTMLElement;
  let pop: HTMLElement;
  let open = false;
  let placed = false;
  let box = '';

  async function show(): Promise<void> {
    open = true; placed = false;
    // Lay the panel out at its final width first; `placed` keeps it hidden while it is measured.
    const width = widthFor(WIDTH);
    box = sizing(width);
    await tick();
    box = place(holder, pop, width); placed = true;
  }
  function hide(): void { open = false; placed = false; box = ''; }
  function toggle(): void { if (open) hide(); else void show(); }
  /** A press anywhere but the pill and the panel closes it. The pill's own press is a toggle. */
  function pressed(event: PointerEvent): void {
    const target = event.target as Node;
    if (open && !holder.contains(target) && !pop?.contains(target)) hide();
  }
</script>
<!-- The panel is fixed to the window, so a scroll would strand it beside the wrong pill. Capture,
     because these pages scroll in their own containers as well as in the window. -->
<svelte:window
  on:scroll|capture={hide}
  on:resize={hide}
  on:pointerdown={pressed}
  on:keydown={e => { if (open && e.key === 'Escape') hide(); }}
/>
<span class="holder" bind:this={holder}>
  <button type="button" class="pill k-{kind}" aria-expanded={open} aria-label="Test kinds" on:click={toggle}>{text}</button>
</span>
{#if open}
  <span class="pop" class:placed={placed} style={box} bind:this={pop} use:portal>
    <span class="pop-head">
      <span>The level is how much of the product the test runs. A manual test is run by a person.</span>
      <button type="button" class="close" aria-label="Close" on:click={hide}>×</button>
    </span>
    {#each TEST_KIND_NOTES as note (note.key)}
      <span class="kind" class:here={note.key === kind}><span class="pill k-{note.key} static">{note.title}</span><span class="note">{note.note}</span></span>
    {/each}
  </span>
{/if}
<style>
  .holder { display: inline-block; vertical-align: 1px; }
  .pill {
    display: inline-block; padding: 1px 7px; border-radius: 4px; font-size: 10px; font-weight: 650;
    letter-spacing: .3px; text-transform: uppercase; line-height: 1.6; cursor: pointer;
    border: 1px solid var(--line); background: var(--soft); color: var(--muted); min-height: 0;
  }
  .pill.static { cursor: default; flex-shrink: 0; }
  /* Low-chroma tints, in the same family as the existing badges. */
  .k-unit { background: #e7eee3; border-color: #cbdac2; color: #3d5a41; }
  .k-integration { background: #e2eceb; border-color: #c4d9d6; color: #2f5a55; }
  .k-system { background: #f4ecda; border-color: #e0d3b3; color: #6f5622; }
  .k-human { background: #f4e8e3; border-color: #e0cabe; color: #8a4a33; }
  .k-until-automated { background: #ecebe6; border-color: #d7d6cc; color: #5f6158; }
  .pop {
    position: fixed; z-index: 20; visibility: hidden; overflow-y: auto; overscroll-behavior: contain;
    display: flex; flex-direction: column; gap: 7px; padding: 11px 12px; text-align: left;
    background: var(--surface); border: 1px solid var(--line); border-radius: 8px;
    box-shadow: 0 6px 24px #24352c1f; font-size: 12px; line-height: 1.45; color: var(--ink);
  }
  .pop.placed { visibility: visible; }
  .pop-head {
    display: flex; align-items: flex-start; gap: 8px; color: var(--muted);
    padding-bottom: 6px; border-bottom: 1px solid var(--line);
  }
  .close {
    flex-shrink: 0; margin: -3px -3px 0 auto; padding: 0; width: 20px; height: 20px; min-height: 0;
    border: 0; border-radius: 4px; background: none; color: var(--muted);
    font-size: 15px; line-height: 1; cursor: pointer;
  }
  .close:hover { background: var(--soft); color: var(--ink); }
  /* Not `.row`: the global stylesheet owns that name and its flex-wrap breaks this layout. */
  .kind { display: flex; flex-direction: column; align-items: flex-start; gap: 3px; opacity: .66; }
  .kind.here { opacity: 1; }
  .note { min-width: 0; }
</style>
