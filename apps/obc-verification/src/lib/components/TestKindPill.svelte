<script lang="ts">
  // One pill per test kind. Every pill opens the same panel, which lists all the kinds, because a
  // level pill and a manual pill look alike and answer different questions.
  //
  // The panel is placed by [`place`] rather than by CSS, and it is moved to `document.body` by
  // [`portal`] first. A pill can sit anywhere on a long page: a panel drawn below its pill by CSS
  // alone falls off the bottom of the window, where nothing can scroll to it. The move to the body
  // is what makes `position: fixed` mean the window — inside the criterion card it resolved against
  // the card instead — and it also takes the panel out of any container that clips its overflow.
  import { tick } from 'svelte';
  import { TEST_KIND_NOTES } from '$lib/types';
  /** `unit`, `integration`, `system`, `human`, `until-automated`, or `manual` when the type of a
   *  manual test is not set. */
  export let kind: string;
  $: text = kind === 'human' ? 'human check' : kind === 'until-automated' ? 'until automated' : kind;

  /** Distance kept from every window edge, and between the panel and its pill. */
  const EDGE = 8, OFFSET = 6, WIDTH = 330;
  /** Grace period for the pointer to travel from the pill to the panel, which a long list has to be
   *  reachable to scroll. */
  const GRACE = 140;
  let holder: HTMLElement;
  let pop: HTMLElement;
  let open = false;
  let placed = false;
  let closing: ReturnType<typeof setTimeout> | undefined;
  let box = '';

  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy: () => node.remove() };
  }
  /**
   * Put the panel where it is wholly visible: below the pill when the room below holds it, above
   * when it does not, and inside both side edges. `max-height` is the room it actually has, so a
   * window too short for the whole list gives the panel its own scrollbar instead of a part that
   * cannot be reached.
   *
   * The width is fixed before this runs and is not changed here. Measuring an unconstrained panel
   * reports the height of text on fewer lines than it will actually wrap to, which put the panel
   * about 40 px past the bottom of the window.
   */
  function place(width: number): void {
    const pill = holder.getBoundingClientRect();
    const below = window.innerHeight - pill.bottom - EDGE - OFFSET;
    const above = pill.top - EDGE - OFFSET;
    const room = Math.max(below, above, 0);
    const height = Math.min(pop.scrollHeight, room);
    const top = below >= height ? pill.bottom + OFFSET : Math.max(EDGE, pill.top - OFFSET - height);
    const left = Math.min(Math.max(EDGE, pill.left), window.innerWidth - width - EDGE);
    box = `top:${top}px;left:${left}px;width:${width}px;max-height:${room}px`;
    placed = true;
  }
  async function show(): Promise<void> {
    clearTimeout(closing);
    if (open) return;
    open = true; placed = false;
    const width = Math.min(WIDTH, window.innerWidth - 2 * EDGE);
    // Lay the panel out at its final width first; `placed` keeps it hidden while it is measured.
    box = `top:0;left:0;width:${width}px`;
    await tick();
    place(width);
  }
  function hide(): void { clearTimeout(closing); open = false; placed = false; box = ''; }
  function leave(): void { closing = setTimeout(hide, GRACE); }
</script>
<!-- The panel is fixed to the window, so a scroll would strand it beside the wrong pill. Capture,
     because these pages scroll in their own containers as well as in the window. -->
<svelte:window on:scroll|capture={hide} on:resize={hide} />
<span
  class="holder"
  bind:this={holder}
  role="presentation"
  on:mouseenter={show}
  on:mouseleave={leave}
  on:focusin={show}
  on:focusout={leave}
  on:keydown={e => { if (e.key === 'Escape') hide(); }}
>
  <button type="button" class="pill k-{kind}" aria-expanded={open} aria-label="Test kinds">{text}</button>
</span>
{#if open}
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <span
    class="pop"
    class:placed={placed}
    style={box}
    bind:this={pop}
    use:portal
    role="tooltip"
    on:mouseenter={show}
    on:mouseleave={leave}
  >
    <span class="pop-head">The level is how much of the product the test runs. A manual test is run by a person.</span>
    {#each TEST_KIND_NOTES as note (note.key)}
      <span class="kind" class:here={note.key === kind}><span class="pill k-{note.key} static">{note.title}</span><span class="note">{note.note}</span></span>
    {/each}
  </span>
{/if}
<style>
  .holder { display: inline-block; vertical-align: 1px; }
  .pill {
    display: inline-block; padding: 1px 7px; border-radius: 4px; font-size: 10px; font-weight: 650;
    letter-spacing: .3px; text-transform: uppercase; line-height: 1.6; cursor: help;
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
  .pop-head { color: var(--muted); padding-bottom: 6px; border-bottom: 1px solid var(--line); }
  /* Not `.row`: the global stylesheet owns that name and its flex-wrap breaks this layout. */
  .kind { display: flex; flex-direction: column; align-items: flex-start; gap: 3px; opacity: .66; }
  .kind.here { opacity: 1; }
  .note { min-width: 0; }
</style>
