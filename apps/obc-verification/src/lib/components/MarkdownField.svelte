<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { listEnter } from '$lib/markdown-keys';
  import Markdown from './Markdown.svelte';
  import ProseCheck from './ProseCheck.svelte';
  export let value = '';
  export let label = 'Description';
  export let required = false;
  export let rows = 6;
  let preview = false;
  let field: HTMLTextAreaElement;
  const dispatch = createEventDispatcher();
  /** A fix from the writing notes is an edit like any other, so it tells the page as typing does. */
  function fix(next: string) { value = next; dispatch('input'); }
  /**
   * Enter carries a Markdown list on. The edit goes through the browser's own editing commands
   * rather than through `value`, so the field keeps one undo history with the typing around it.
   */
  function keys(event: KeyboardEvent) {
    if (event.key !== 'Enter' || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
    if (field.selectionStart !== field.selectionEnd) return;
    const edit = listEnter(value, field.selectionStart);
    if (!edit) return;
    event.preventDefault();
    field.setSelectionRange(edit.from, edit.to);
    if (edit.text) document.execCommand('insertText', false, edit.text); else document.execCommand('delete');
  }
</script>
<div class="field">
  <div class="row"><label class="field-label"><span>{label} <span class="muted small">· Markdown</span></span>
    <textarea bind:this={field} class:visually-hidden={preview} bind:value {required} {rows} on:input on:keydown={keys}></textarea>
  </label></div>
  <div class="editor-tools"><button type="button" class="text-button" on:click={() => preview = !preview}>{preview ? 'Continue editing' : 'Preview formatting'}</button></div>
  {#if preview}<div class="inset preview"><Markdown text={value || '*Nothing to preview yet.*'} /></div>{/if}
  <ProseCheck text={value} field={preview ? undefined : field} onfix={fix} />
</div>
