<script lang="ts">
  import type { Requirement, RequirementSuggestionReview } from '$lib/types';
  import { day } from './api';
  import Markdown from './Markdown.svelte';
  export let suggestions: RequirementSuggestionReview[] = [];
  export let requirements: Requirement[] = [];
  export let revisionId: number;
  export let busy = false;
  export let onselect: (requirementId: string) => void;
  export let onclose: () => void;
  /** Resolves true once the console has recorded the decision. */
  export let ondecide: (id: string, accept: boolean, feedback?: string) => Promise<boolean>;
  /** The item whose body is open. Bound by the parent so a requirement can point at its own suggestion. */
  export let expanded = '';
  /** Ticked in this session. The item stays in place, struck through, until the page is loaded again. */
  let done: Record<string, boolean> = {};
  let dismissing = '';
  let feedback = '';
  $: standing = suggestions.filter(s => s.status === 'open' || done[s.id]);
  $: groups = [
    { name: 'New requirements', items: standing.filter(s => !s.requirementId) },
    { name: 'Changes to requirements', items: standing.filter(s => s.requirementId) }
  ];
  $: decided = suggestions.filter(s => (s.status === 'accepted' || s.status === 'dismissed') && !done[s.id]);
  function subject(suggestion: RequirementSuggestionReview) { return requirements.find(r => r.id === suggestion.requirementId); }
  function toggle(id: string) { expanded = expanded === id ? '' : id; }
  /** Who wrote it and when, after the link or the group. Svelte trims separators written as markup. */
  function meta(suggestion: RequirementSuggestionReview) {
    const parts = [suggestion.author, day(suggestion.createdAt)];
    if (!suggestion.requirementId && suggestion.group) parts.unshift(suggestion.group);
    return (suggestion.requirementId ? ' · ' : '') + parts.join(' · ');
  }
  async function accept(suggestion: RequirementSuggestionReview) {
    if (busy || done[suggestion.id]) return;
    if (await ondecide(suggestion.id, true)) done = { ...done, [suggestion.id]: true };
  }
  async function dismiss(id: string) {
    if (busy) return;
    if (await ondecide(id, false, feedback)) { dismissing = ''; feedback = ''; }
  }
</script>
<section class="suggestions" aria-label="Suggestions">
  <div class="panel-head">
    <div class="row"><h2>Suggestions</h2><button class="text-button small" on:click={onclose}>Close</button></div>
    <p>Tick an item once you have written it yourself. Nothing here changes the draft.</p>
  </div>
  <div class="panel-scroll">
    {#each groups as group (group.name)}
      {#if group.items.length}
        <span class="eyebrow">{group.name} · {group.items.length}</span>
        {#each group.items as s (s.id)}
          {@const current = subject(s)}
          <div class="item" class:open={expanded === s.id} class:done={done[s.id]}>
            <input type="checkbox" checked={!!done[s.id]} disabled={busy || done[s.id] || s.missing} aria-label={`Done: ${s.title}`} on:change={() => accept(s)} />
            <div>
              <button class="title" on:click={() => toggle(s.id)} aria-expanded={expanded === s.id}>{s.title}</button>
              <div class="sub">{#if s.requirementId}<a href="#requirement" on:click|preventDefault={() => onselect(s.requirementId ?? '')}>{s.requirementId}{current ? ` · ${current.title}` : ''}</a>{/if}{meta(s)}{#if s.sourceSha}{' · commit '}<code>{s.sourceSha.slice(0, 10)}</code>{/if}{#if s.requirementId}{` · against r${s.baseRevision}`}{/if}</div>
              {#if expanded === s.id}
                <div class="body">
                  {#if s.missing}<p class="stale">{s.requirementId} is no longer in r{revisionId}. You can only dismiss this suggestion.</p>{/if}
                  {#if current}<span class="eyebrow">Current</span><Markdown text={current.statement} />{/if}
                  <span class="eyebrow">{current ? 'Suggested' : 'Suggested statement'}</span>
                  <div class="statement"><Markdown text={s.statement} /></div>
                  <span class="eyebrow">Why</span>{s.reason}
                  {#if s.stale}<p class="stale">{s.stale}</p>{/if}
                </div>
              {/if}
              {#if dismissing === s.id}
                <div class="decide">
                  <label>Feedback for the agent (optional)<textarea rows={2} maxlength={5000} bind:value={feedback} placeholder="Why this is not a requirement, or what should change"></textarea></label>
                  <div class="actions"><button class="danger" disabled={busy} on:click={() => dismiss(s.id)}>Dismiss</button><button disabled={busy} on:click={() => { dismissing = ''; feedback = ''; }}>Keep</button></div>
                </div>
              {/if}
            </div>
            <button class="x" aria-label={`Dismiss: ${s.title}`} disabled={busy || done[s.id]} on:click={() => { dismissing = dismissing === s.id ? '' : s.id; feedback = ''; }}>×</button>
          </div>
        {/each}
      {/if}
    {/each}
    {#if !standing.length}<p class="muted small empty">Nothing is waiting. Agents suggest requirements with <code>obc req suggest</code>.</p>{/if}
    {#if decided.length}
      <details class="decided"><summary>Decided ({decided.length})</summary>
        {#each decided as s (s.id)}<p><span class="badge" class:success={s.status === 'accepted'}>{s.status === 'accepted' ? 'done' : 'dismissed'}</span>{#if s.requirementId}{s.requirementId} · {/if}{s.title} · {s.author} · {day(s.createdAt)}{#if s.decidedBy} · {s.decidedBy}{/if}{#if s.feedback} · “{s.feedback}”{/if}</p>{/each}
      </details>
    {/if}
  </div>
</section>
<style>
  .suggestions { display: flex; flex-direction: column; min-width: 0; background: var(--slate-bg); border-left: 1px solid var(--slate-line); }
  .panel-head { padding: 18px 18px 12px; border-bottom: 1px solid var(--slate-line); }
  .panel-head h2 { font-size: 15px; color: var(--slate-strong); }
  .panel-head p { margin: 4px 0 0; font-size: 12px; color: var(--muted); }
  .panel-scroll { flex: 1; overflow: auto; padding: 8px 12px 18px; scrollbar-width: thin; }
  .eyebrow { display: block; margin: 14px 6px 6px; color: var(--slate); }
  .empty { margin: 14px 6px; }
  .item { display: grid; grid-template-columns: 22px minmax(0, 1fr) auto; gap: 10px; align-items: start; margin: 6px 0; padding: 10px 12px 10px 10px; background: var(--surface); border: 1px solid var(--slate-line); border-radius: 8px; }
  .item.open { border-color: var(--slate); box-shadow: 0 2px 8px #2e4a6114; }
  .item.done { opacity: .55; }
  .item input[type=checkbox] { margin: 3px 0 0; width: 16px; height: 16px; accent-color: var(--slate); }
  .item .title { display: block; width: 100%; padding: 0; text-align: left; font-weight: 600; line-height: 1.35; background: transparent; border-color: transparent; }
  .item .title:hover { background: transparent; text-decoration: underline; }
  .item.done .title { text-decoration: line-through; }
  .sub { margin-top: 2px; font-size: 11px; color: var(--muted); }
  .sub a { color: var(--slate); font-weight: 600; text-decoration: underline; }
  .body { margin-top: 6px; font-size: 13px; line-height: 1.5; }
  .body .eyebrow { margin: 10px 0 3px; color: var(--muted); }
  .body .statement { padding: 8px 10px; background: var(--slate-bg); border-left: 3px solid var(--slate); border-radius: 0 6px 6px 0; }
  .stale { margin: 8px 0 0; padding: 6px 9px; font-size: 12px; color: var(--amber); background: #fff6e2; border: 1px solid #ead9b3; border-radius: 6px; }
  .x { padding: 2px 4px; font-size: 16px; line-height: 1; color: var(--muted); background: transparent; border-color: transparent; }
  .x:hover { color: var(--bad); background: transparent; }
  .decide { margin-top: 6px; padding-top: 8px; border-top: 1px dashed var(--slate-line); }
  .decide label { margin: 0; font-size: 12px; }
  .decide textarea { font-size: 13px; }
  .decide .actions { margin-top: 8px; }
  .decide button { padding: 5px 10px; font-size: 12px; }
  .decided { margin: 18px 6px 0; font-size: 12px; }
  .decided summary { padding: 3px 0; color: var(--muted); cursor: pointer; }
  .decided p { margin: 6px 0; overflow-wrap: anywhere; }
  .decided .badge { margin-right: 4px; }
  @media (max-width: 650px) { .suggestions { border-left: 0; border-top: 1px solid var(--slate-line); } }
</style>
