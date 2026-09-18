<script lang="ts">
  // The writing notes under a prose field. Each note names the problem text, says what is wrong,
  // and offers Harper's fixes as buttons. A note's offsets belong to the text it was found in, so a
  // fix is applied only while the field still holds that text; the next check replaces the notes.
  import { onMount } from 'svelte';
  import { check, apply, type Note, type Fix } from '$lib/prose';
  export let text = '';
  export let language: 'plaintext' | 'markdown' = 'markdown';
  /** The field the notes belong to. Choosing a note selects the problem text in it. */
  export let field: HTMLTextAreaElement | HTMLInputElement | undefined = undefined;
  export let onfix: (next: string) => void;

  /** Long enough that a check does not run on every keystroke. */
  const WAIT = 450;
  let mounted = false;
  let notes: Note[] = [];
  /** The text `notes` describes. */
  let checked = '';
  let failed = false;
  let loading = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let run = 0;

  onMount(() => { mounted = true; return () => clearTimeout(timer); });
  $: if (mounted) schedule(text);

  function schedule(next: string): void {
    clearTimeout(timer);
    if (next === checked) return;
    timer = setTimeout(() => void look(), WAIT);
  }
  async function look(): Promise<void> {
    const mine = ++run, source = text;
    if (!source.trim()) { notes = []; checked = source; return; }
    loading = !checked && !failed;
    try {
      const found = await check(source, language);
      if (mine !== run) return;
      notes = found; checked = source; failed = false;
    } catch {
      if (mine === run) failed = true;
    } finally {
      if (mine === run) loading = false;
    }
  }
  function fix(note: Note, choice: Fix): void {
    if (text !== checked) return;
    onfix(apply(text, note, choice));
  }
  function locate(note: Note): void {
    if (!field || text !== checked) return;
    field.focus();
    field.setSelectionRange(note.start, note.end);
  }
</script>
{#if failed}
  <p class="state small muted">The writing checker did not load. Your text is unaffected.</p>
{:else if loading}
  <p class="state small muted">Starting the writing checker…</p>
{:else if notes.length}
  <div class="notes">
    <p class="notes-head small"><strong>{notes.length} writing {notes.length === 1 ? 'note' : 'notes'}</strong><span class="muted">Checked on this computer</span></p>
    {#each notes as note}
      <div class="note">
        <button type="button" class="source" title="Show this in the text" on:click={() => locate(note)}>{note.source}</button>
        <span class="message">{note.message}</span>
        {#each note.fixes.slice(0, 3) as choice}
          <button type="button" class="fix" on:click={() => fix(note, choice)}>{choice.label}</button>
        {/each}
      </div>
    {/each}
  </div>
{/if}
<style>
  .state { margin: 6px 0 0; }
  .notes {
    margin-top: 8px; padding: 9px 12px 11px; border: 1px solid #e6d9bd; border-left: 3px solid #d5a147;
    border-radius: 8px; background: #fffaef;
  }
  .notes-head { display: flex; gap: 10px; align-items: baseline; margin: 0 0 4px; }
  .note {
    display: flex; gap: 8px; align-items: baseline; flex-wrap: wrap;
    padding: 6px 0; border-top: 1px solid #efe3ca; font-size: 13px;
  }
  .notes-head + .note { border-top: 0; }
  .message { flex: 1; min-width: 180px; color: var(--muted); }
  .source, .fix { padding: 2px 8px; min-height: 0; border-radius: 5px; font-size: 12px; line-height: 1.5; }
  .source { border-color: transparent; background: #f4e7c9; font-weight: 600; }
  .source:hover { background: #ebdab3; }
  .fix { border-color: #d9d3c2; background: var(--surface); }
  .fix:hover { background: var(--soft); }
</style>
