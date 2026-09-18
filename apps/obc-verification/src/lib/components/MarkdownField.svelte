<script lang="ts">
  import { createEventDispatcher } from 'svelte';
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
</script>
<div class="field">
  <div class="row"><label class="field-label"><span>{label} <span class="muted small">· Markdown</span></span>
    <textarea bind:this={field} class:visually-hidden={preview} bind:value {required} {rows} on:input></textarea>
  </label></div>
  <div class="editor-tools"><button type="button" class="text-button" on:click={() => preview = !preview}>{preview ? 'Continue editing' : 'Preview formatting'}</button></div>
  {#if preview}<div class="inset preview"><Markdown text={value || '*Nothing to preview yet.*'} /></div>{/if}
  <ProseCheck text={value} field={preview ? undefined : field} onfix={fix} />
</div>
