<script lang="ts">
  import { onMount } from 'svelte';
  import LocalLogin from '$lib/components/LocalLogin.svelte';
  let oauth: boolean | null = null;
  let error = '';
  onMount(async () => {
    error = new URLSearchParams(window.location.search).get('error') || '';
    try {
      const response = await fetch('/api/login');
      if (!response.ok) throw new Error('Could not load sign-in options. Try the local administrator account.');
      oauth = (await response.json()).oauth === true;
    } catch (e) { oauth = false; error ||= e instanceof Error ? e.message : 'Could not load sign-in options. Try the local administrator account.'; }
  });
</script>

<svelte:head><title>Sign in · OpenBikeComputer verification</title></svelte:head>
<main class="login-shell">
  <a class="brand" href="https://openbikecomputer.com"><img class="brand-icon" src="/brand/app-icon.svg" width="32" height="32" alt="" />OpenBikeComputer</a>
  <section class="login-card">
    <div class="eyebrow">Maintainer workspace</div><h1>Verification & releases</h1>
    <p class="muted">Requirements, their tests, and the evidence for each release.</p>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if oauth === null}<p class="muted" role="status">Loading sign-in options…</p>
    {:else if oauth}
      <a href="/auth/github" class="button primary full-width">Continue with GitHub</a>
      <p class="small muted">Your GitHub account must be approved by a workspace administrator.</p>
      <details class="local-login"><summary>Local administrator sign-in</summary><LocalLogin primary={false} /></details>
    {:else}<LocalLogin />{/if}
    <p class="small muted">Access is restricted to approved maintainers.</p>
  </section>
  <a class="small muted" href="https://openbikecomputer.com">← OpenBikeComputer website</a>
</main>
