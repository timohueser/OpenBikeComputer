<script lang="ts">
  import type { Requirement } from '$lib/types';
  export let requirement: Requirement;
  export let editable = false;
  export let disabled = false;
  export let onchange: (requirement: Requirement) => void = () => {};
  const labels = [
    { key: 'todo', title: 'Definition incomplete' },
    { key: 'implementationNeeded', title: 'Implementation needed' },
    { key: 'excluded', title: 'Excluded from releases' }
  ] as const;
  function applied(value: Requirement, key: typeof labels[number]['key']) { return key === 'excluded' ? !value.active : !!value[key]; }
  function toggle(key: typeof labels[number]['key']) {
    if (disabled) return;
    onchange(key === 'excluded' ? { ...requirement, active: !requirement.active } : { ...requirement, [key]: !requirement[key] || undefined });
  }
</script>
<span class="requirement-labels" aria-label="Requirement labels">
  {#each labels as label}
    {#if editable}<button type="button" class="requirement-label" class:applied={applied(requirement, label.key)} aria-pressed={applied(requirement, label.key)} {disabled} on:click={() => toggle(label.key)}>{label.title}</button>
    {:else if applied(requirement, label.key)}<span class="requirement-label applied">{label.title}</span>{/if}
  {/each}
</span>
