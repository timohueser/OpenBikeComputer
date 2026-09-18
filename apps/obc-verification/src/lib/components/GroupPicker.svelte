<script context="module" lang="ts">
  /** Ids for the label and the list, which the list needs even though it is moved to the body. */
  let seq = 0;
</script>
<script lang="ts">
  // Choose the group a requirement belongs to: type to filter the groups that exist, or type a new
  // name and create it. The list is a real list rather than a `datalist`, which every browser draws
  // differently and which hides the counts that tell one group from another.
  //
  // The typed name is kept here until it is chosen or the field is left. A group applied on every
  // keystroke makes a group for each prefix of the name, which rewrites the sidebar under the
  // reader's hands and leaves the list with nothing new to offer.
  import { tick } from 'svelte';
  import { portal, place, sizing, widthFor } from '$lib/popover';
  export let value = '';
  export let groups: { name: string; count: number }[] = [];
  export let label = 'Group';
  export let hint = '';
  export let disabled = false;
  export let onchange: (value: string) => void;

  /** An existing group, the name being typed, or no group at all. */
  type Option = { name: string; label: string; note: string };
  const id = `group-picker-${++seq}`;
  let control: HTMLElement;
  let list: HTMLElement;
  let open = false;
  let placed = false;
  let box = '';
  let at = 0;
  let text = value;
  let last = value;
  /** A different requirement, or a renamed group, refills the box. */
  $: if (value !== last) { last = value; text = value; }

  /** The list filters on what is being typed. Opened on the group a requirement already has, it
   *  shows every group instead, the way a list of choices should. */
  $: typing = text.trim() !== value.trim();
  $: term = typing ? text.trim().toLowerCase() : '';
  $: options = [
    ...(typing && term && !groups.some(group => group.name.toLowerCase() === term)
      ? [{ name: text.trim(), label: text.trim(), note: 'New group' }] : []),
    ...groups.filter(group => group.name.toLowerCase().includes(term))
      .map(group => ({ name: group.name, label: group.name, note: `${group.count} ${group.count === 1 ? 'requirement' : 'requirements'}` })),
    ...(value || term ? [{ name: '', label: 'No group', note: 'Ungrouped' }] : [])
  ] as Option[];
  $: if (at >= options.length) at = 0;

  async function show(): Promise<void> {
    if (open || disabled) return;
    open = true; placed = false; at = 0;
    const width = widthFor(Math.max(control.offsetWidth, 240));
    box = sizing(width);
    await tick();
    box = place(control, list, width); placed = true;
  }
  function hide(): void { open = false; placed = false; box = ''; }
  function commit(next: string): void {
    text = next;
    last = next.trim();
    if (last !== value) onchange(last);
  }
  /** The field keeps the focus throughout, so choosing must not focus it again: that would reopen
   *  the list the choice just closed. */
  function choose(option: Option): void { commit(option.name); hide(); }
  function keys(event: KeyboardEvent): void {
    if (event.key === 'Escape') { event.stopPropagation(); text = value; hide(); return; }
    if (event.key === 'Tab') { hide(); return; }
    if (!open && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) { void show(); return; }
    if (event.key === 'Enter') { event.preventDefault(); choose(open && options[at] ? options[at] : { name: text.trim(), label: '', note: '' }); }
    else if (open && options.length && event.key === 'ArrowDown') { event.preventDefault(); at = (at + 1) % options.length; }
    else if (open && options.length && event.key === 'ArrowUp') { event.preventDefault(); at = (at - 1 + options.length) % options.length; }
  }
  function pressed(event: PointerEvent): void {
    const target = event.target as Node;
    if (open && !control.contains(target) && !list?.contains(target)) hide();
  }
  /** The page moving under the list strands it, but the box scrolls its own text as it is typed
   *  and selected, and that must not close the list. */
  function scrolled(event: Event): void {
    if (open && !control.contains(event.target as Node)) hide();
  }
</script>
<svelte:window on:scroll|capture={scrolled} on:resize={hide} on:pointerdown={pressed} />
<div class="field">
  <label class="picker-label" for={id}>{label} {#if hint}<span class="small muted">· {hint}</span>{/if}</label>
  <div class="picker" bind:this={control}>
    <input
      {id}
      {disabled}
      bind:value={text}
      maxlength={80}
      role="combobox"
      aria-expanded={open}
      aria-controls="{id}-options"
      aria-autocomplete="list"
      aria-activedescendant={open && options[at] ? `${id}-option-${at}` : undefined}
      autocomplete="off"
      placeholder="Choose a group or type a new name"
      on:input={show}
      on:keydown={keys}
      on:focus={show}
      on:click={show}
      on:blur={() => commit(text)}
    />
    <button type="button" class="reveal" {disabled} tabindex="-1" aria-label={open ? 'Hide the groups' : 'Show the groups'} on:click={() => open ? hide() : show()}>▾</button>
  </div>
</div>
{#if open}
  <!-- The press is stopped so that the field keeps the focus: a blur here would apply a half-typed
       name before the chosen one. `click` still arrives. -->
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <ul class="options" class:placed id="{id}-options" role="listbox" style={box} bind:this={list} use:portal on:pointerdown|preventDefault>
    {#each options as option, index}
      <li
        id="{id}-option-{index}"
        role="option"
        class="option"
        class:active={index === at}
        aria-selected={option.name === value}
        on:click={() => choose(option)}
        on:pointerenter={() => at = index}
      >
        <span class="label">{option.label}</span><span class="note small muted">{option.note}</span>
      </li>
    {:else}
      <li class="option empty small muted">No groups yet. Type a name to make the first one.</li>
    {/each}
  </ul>
{/if}
<style>
  .picker-label { margin: 0; }
  .picker { position: relative; display: flex; align-items: center; }
  .picker input { padding-right: 34px; }
  .reveal {
    position: absolute; right: 1px; top: 7px; width: 30px; padding: 0; min-height: 0; height: 32px;
    border-color: transparent; background: none; color: var(--muted); font-size: 11px;
  }
  .options {
    position: fixed; z-index: 20; visibility: hidden; overflow-y: auto; overscroll-behavior: contain;
    margin: 0; padding: 5px; list-style: none;
    background: var(--surface); border: 1px solid var(--line); border-radius: 8px;
    box-shadow: 0 6px 24px #24352c1f;
  }
  .options.placed { visibility: visible; }
  .option {
    display: flex; gap: 10px; align-items: baseline; justify-content: space-between;
    padding: 8px 10px; border-radius: 5px; cursor: pointer; font-size: 13px;
  }
  .option.active { background: var(--soft); }
  .option.empty { cursor: default; }
  .label { overflow-wrap: anywhere; }
  .note { flex-shrink: 0; }
</style>
