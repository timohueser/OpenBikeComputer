<script lang="ts">
  // One small pill per test kind, and the explainer every pill opens.
  //
  // Two questions are being answered at once, and the explainer exists because they look alike on
  // the page: the level says how much of the product a test exercises, and manual says who runs it.
  // Every pill opens the same list, so whichever one a reader lands on tells them the whole scheme.
  import { TEST_KIND_NOTES } from '$lib/types';
  /** `unit` | `integration` | `system` | `human` | `until-automated`, or `manual` for a procedure
   *  whose reason nobody has stated yet — that one gets the plain pill, which is the honest look for
   *  "not said". */
  export let kind: string;
  $: entry = TEST_KIND_NOTES.find(n => n.key === kind);
  $: text = kind === 'human' ? 'manual · human' : kind === 'until-automated' ? 'manual · for now' : kind;
</script>
<span class="holder">
  <button type="button" class="pill k-{kind}" aria-label={`${entry?.title ?? kind} — what the test kinds mean`}>{text}</button>
  <span class="pop" role="tooltip">
    <span class="pop-head">Level is how much of the product runs. Manual is who runs it.</span>
    {#each TEST_KIND_NOTES as note (note.key)}
      <span class="kind" class:here={note.key === kind}><span class="pill k-{note.key} static">{note.title}</span><span class="note">{note.note}</span></span>
    {/each}
  </span>
</span>
<style>
  .holder { position: relative; display: inline-block; vertical-align: 1px; }
  .pill {
    display: inline-block; padding: 1px 7px; border-radius: 4px; font-size: 10px; font-weight: 650;
    letter-spacing: .3px; text-transform: uppercase; line-height: 1.6; cursor: help;
    border: 1px solid var(--line); background: var(--soft); color: var(--muted); min-height: 0;
  }
  .pill.static { cursor: default; flex-shrink: 0; }
  /* Low-chroma tints from the same family as the existing badges: told apart at a glance, never loud. */
  .k-unit { background: #e7eee3; border-color: #cbdac2; color: #3d5a41; }
  .k-integration { background: #e2eceb; border-color: #c4d9d6; color: #2f5a55; }
  .k-system { background: #f4ecda; border-color: #e0d3b3; color: #6f5622; }
  .k-human { background: #f4e8e3; border-color: #e0cabe; color: #8a4a33; }
  .k-until-automated { background: #ecebe6; border-color: #d7d6cc; color: #5f6158; }
  .pop {
    position: absolute; left: 0; top: calc(100% + 6px); z-index: 6; width: 330px; max-width: 74vw;
    display: none; flex-direction: column; gap: 7px; padding: 11px 12px; text-align: left;
    background: var(--surface); border: 1px solid var(--line); border-radius: 8px;
    box-shadow: 0 6px 24px #24352c1f; font-size: 12px; line-height: 1.45; color: var(--ink);
    text-transform: none; letter-spacing: 0; font-weight: 400;
  }
  .holder:hover .pop, .holder:focus-within .pop { display: flex; }
  .pop-head { color: var(--muted); padding-bottom: 6px; border-bottom: 1px solid var(--line); }
  /* Not `.row`: the global stylesheet owns that name and its flex-wrap breaks this layout. */
  .kind { display: flex; flex-direction: column; align-items: flex-start; gap: 3px; opacity: .66; }
  .kind.here { opacity: 1; }
  .note { min-width: 0; }
  @media (max-width: 650px) { .pop { left: auto; right: 0; } }
</style>
