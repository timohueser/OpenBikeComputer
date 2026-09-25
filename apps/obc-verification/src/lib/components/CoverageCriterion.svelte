<script lang="ts">
  import type { AcceptanceCriterion, Catalog, CoverageEvidence, Requirement, VerificationTest } from '$lib/types';
  import { criterionCovered, evidenceKey, evidenceTest, proposedCovered } from '$lib/coverage';
  import Markdown from './Markdown.svelte';
  import Files from './Files.svelte';
  import TestKindPill from './TestKindPill.svelte';
  export let criterion: AcceptanceCriterion;
  export let requirement: Requirement;
  export let catalog: Catalog | undefined = undefined;
  /** Position in the plan, so "criterion 2 of SYS-003" is the card labelled 2. Removed criteria have none. */
  export let number: number | undefined = undefined;
  /** When set, the card shows what changed since this earlier version of the criterion. */
  export let previous: AcceptanceCriterion | undefined = undefined;
  export let tag = '';
  export let removed = false;
  /** A proposed criterion is judged on its own evidence, which is linked when the plan is approved. */
  export let proposed = false;
  /** Manual procedures a proposal brings; they exist on the requirement only after approval. */
  export let procedures: VerificationTest[] = [];
  /** The release view sets this to show each evidence test's outcome in one candidate. */
  export let result: ((test: VerificationTest) => { outcome: string; detail?: string; label?: string; disabled?: boolean; onrun?: () => void }) | undefined = undefined;
  $: covered = !removed && (proposed ? proposedCovered(criterion) : criterionCovered(requirement, criterion)) && criterion.evidence.every(e => e.rationale.trim());
  /** A removed criterion takes all of its evidence with it. */
  $: kept = removed ? [] : criterion.evidence;
  $: dropped = removed ? criterion.evidence : previous?.evidence.filter(e => !criterion.evidence.some(n => evidenceKey(n) === evidenceKey(e))) ?? [];
  $: footer = !!(criterion.gap || previous?.gap || criterion.next || previous?.next);
  const resolve = (e: CoverageEvidence) => evidenceTest(requirement, e) ?? procedures.find(p => p.id === e.testId);
  const title = (e: CoverageEvidence) => resolve(e)?.title ?? catalog?.cases.find(c => c.id === e.caseId)?.name ?? evidenceKey(e);
  /** Catalogue names carry the whole nested test title, so the card clamps them and the tooltip holds the full text. */
  const fullName = (e: CoverageEvidence) => `${title(e)}\n${e.caseId ?? 'manual procedure'}`;
  const isNew = (e: CoverageEvidence) => !!previous && !previous.evidence.some(p => evidenceKey(p) === evidenceKey(e));
  const isProposedProcedure = (e: CoverageEvidence) => !!e.testId && !evidenceTest(requirement, e) && procedures.some(p => p.id === e.testId);
  const nextChanged = () => !!previous && JSON.stringify(previous.next ?? null) !== JSON.stringify(criterion.next ?? null);
</script>
<article class="criterion" class:covered class:removed>
  <span class="mark" aria-hidden="true">{covered ? '✓' : ''}</span>
  <div class="body">
    <div class="head">
      <p class="statement wrap">{#if number}<span class="num" aria-label={`Criterion ${number}`}>{number}</span>{/if}{#if previous && previous.statement !== criterion.statement}<del>{previous.statement}</del> {/if}{criterion.statement}</p>
      {#if tag}<span class="badge" class:success={tag === 'New'} class:warning={tag === 'Changed'} class:error={tag === 'Removed'}>{tag}</span>{/if}
    </div>
    {#each kept as e}
      {@const test = resolve(e)}
      {@const found = test && result ? result(test) : undefined}
      <div class="evidence" class:added={isNew(e)}>
        <div class="name wrap" title={fullName(e)}><span class="label"><strong>{title(e)}</strong>{#if isProposedProcedure(e)}<span class="tag">new procedure</span>{:else if isNew(e)}<span class="tag">new</span>{/if}</span><span class="pills">{#if e.level}<TestKindPill kind={e.level} />{/if}{#if test?.kind === 'manual'}<TestKindPill kind={test.manualReason ?? 'manual'} />{/if}</span>{#if e.caseId}<small>{e.caseId}</small>{/if}</div>
        <div class="why wrap">
          <p class="line">{e.rationale}{#if found}<span class="badge outcome" class:success={found.outcome === 'pass'} class:error={found.outcome === 'fail' || found.outcome === 'error'} class:warning={!['pass', 'fail', 'error'].includes(found.outcome)}>{found.outcome}</span>{#if found.onrun}<button class="run small" disabled={found.disabled} on:click={found.onrun}>{found.label}</button>{/if}{/if}</p>
          {#if test?.kind === 'manual'}<details class="small"><summary>Procedure</summary><Markdown text={test.steps || ''} /><h4>Expected result</h4><Markdown text={test.expected || ''} />{#if test.inputs.length}<Files files={test.inputs} label="Input files" />{/if}</details>{/if}
          {#if found?.detail}<details class="small"><summary>Test output</summary><pre class="wrap">{found.detail}</pre></details>{/if}
        </div>
      </div>
    {/each}
    {#each dropped as e}<div class="evidence dropped"><div class="name wrap" title={fullName(e)}><span class="label"><del><strong>{title(e)}</strong></del></span><small>{e.caseId ?? 'manual'}</small></div><div class="why wrap"><p class="line"><del>{e.rationale}</del></p></div></div>{/each}
    {#if !criterion.evidence.length && !removed}<p class="none small">No evidence yet</p>{/if}
    {#if footer}
      <div class="foot">
        {#if criterion.gap}<p class="gap wrap"><strong>Gap</strong> · {criterion.gap}</p>{:else if previous?.gap}<p class="gap wrap"><del>Gap · {previous.gap}</del></p>{/if}
        {#if criterion.next}<p class="next wrap">{#if nextChanged() && previous?.next}<del>{previous.next.summary}</del> {/if}<TestKindPill kind={criterion.next.level} /> {criterion.next.summary}</p>
        {:else if previous?.next}<p class="next wrap"><del><TestKindPill kind={previous.next.level} /> {previous.next.summary}</del></p>{/if}
      </div>
    {/if}
  </div>
</article>
<style>
  .criterion { display: flex; gap: 12px; padding: 12px 14px; border: 1px solid var(--line); border-radius: 8px; background: var(--surface); min-width: 0; }
  .covered { border-color: var(--good-line); }
  .removed { opacity: .65; border-style: dashed; }
  .mark { flex-shrink: 0; width: 20px; height: 20px; margin-top: 1px; border-radius: 50%; border: 1.5px solid var(--warn); color: white; font-size: 13px; font-weight: 700; display: grid; place-items: center; }
  .covered .mark { background: var(--good); border-color: var(--good); }
  .removed .mark { border-color: var(--line); }
  .body { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .head { display: flex; justify-content: space-between; gap: 10px; align-items: flex-start; }
  .statement { margin: 0; font-weight: 600; line-height: 1.45; }
  .num { font: 600 12px/1 var(--mono); color: var(--muted); margin-right: 8px; vertical-align: 1px; }
  .evidence { display: grid; grid-template-columns: minmax(150px, 34%) minmax(0, 1fr); gap: 2px 14px; margin: 8px 0 0; font-size: 13px; line-height: 1.5; }
  .name strong { font-weight: 600; }
  .label { display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
  .name, .why { min-width: 0; }
  .name small { display: block; font: 12px/1.4 var(--mono); color: var(--muted); overflow-wrap: anywhere; }
  .added .name strong { color: var(--good); }
  .dropped { opacity: .7; }
  .why { color: var(--muted); }
  .line { margin: 0; }
  .outcome { margin-left: 6px; }
  .run { margin-left: 8px; padding: 3px 9px; }
  .none { margin: 6px 0 0; color: var(--warn); }
  .tag { font-size: 11.5px; text-transform: uppercase; letter-spacing: .6px; color: var(--good); font-weight: 700; margin-left: 6px; }
  .why details { margin-top: 2px; }
  .why h4 { margin: 8px 0 2px; }
  .foot { margin: 10px -14px -12px -46px; padding: 8px 14px 9px 46px; border-top: 1px solid var(--line); background: var(--paper); border-radius: 0 0 8px 8px; font-size: 13px; display: flex; flex-direction: column; gap: 3px; }
  .covered .foot { border-top-color: var(--good-line); }
  .gap { margin: 0; color: var(--warn); }
  .gap strong { font-weight: 650; }
  .next { margin: 0; }
  .pills { display: flex; flex-wrap: wrap; gap: 4px; margin: 3px 0 1px; }
  del { color: var(--muted); }
  @media (max-width: 650px) { .evidence { grid-template-columns: minmax(0, 1fr); } }
</style>
