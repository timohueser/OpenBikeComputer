<script lang="ts">
  import type { Actor } from '$lib/types';
  import { api, message } from './api';
  import Users from './Users.svelte';
  import Maintenance from './Maintenance.svelte';
  import AgentAccess from './AgentAccess.svelte';

  export let actor: Actor;
  export let dirty = false;
  export let onback: () => void;
  export let workspaceDirty = false;
  export let onreset: () => void;
  export let resetBusy = false;
  let view: 'account' | 'users' | 'agents' | 'maintenance' = 'account';
  let agentBusy = false;
  let currentPassword = '';
  let newPassword = '';
  let confirmation = '';
  let busy = false;
  let error = '';
  let notice = '';
  $: dirty = !!(currentPassword || newPassword || confirmation);

  async function changePassword() {
    error = ''; notice = '';
    if (newPassword !== confirmation) { error = 'The new passwords do not match.'; return; }
    busy = true;
    try {
      await api('/api/account/password', 'POST', { currentPassword, newPassword });
      currentPassword = ''; newPassword = ''; confirmation = '';
      notice = 'Password changed. You remain signed in. Other local administrator sessions have been signed out.';
    } catch (e) { error = message(e); }
    finally { busy = false; }
  }
</script>

<div class="account-shell">
  <button class="back" disabled={resetBusy || agentBusy} on:click={onback}>← Back to workspace</button>
  <div class="page-heading"><div class="eyebrow">Workspace access</div><h1>Account</h1><p class="muted">Signed in as <strong>{actor.name}</strong> · {actor.admin ? 'Administrator' : 'Maintainer'}</p></div>
  {#if actor.admin}
    <nav class="account-nav" aria-label="Account settings"><button disabled={resetBusy || agentBusy} aria-current={view === 'account' ? 'page' : undefined} on:click={() => view = 'account'}>Your account</button><button disabled={resetBusy || agentBusy} aria-current={view === 'users' ? 'page' : undefined} on:click={() => view = 'users'}>Users</button><button disabled={resetBusy || agentBusy} aria-current={view === 'agents' ? 'page' : undefined} on:click={() => view = 'agents'}>Agent access</button><button disabled={resetBusy || agentBusy} aria-current={view === 'maintenance' ? 'page' : undefined} on:click={() => view = 'maintenance'}>Maintenance</button></nav>
  {/if}
  <section class="panel account-panel" hidden={view !== 'account'}>
    {#if actor.provider === 'local'}
      <h2>Local administrator password</h2>
      <p class="muted">This account remains available as a fallback to GitHub sign-in.</p>
      <form on:submit|preventDefault={changePassword}>
        <fieldset disabled={busy}>
          <label>Current password<input type="password" required maxlength={1024} autocomplete="current-password" bind:value={currentPassword} /></label>
          <label>New password<input type="password" required minlength={16} maxlength={1024} autocomplete="new-password" bind:value={newPassword} aria-describedby="password-guidance" /></label>
          <p class="small muted" id="password-guidance">Use at least 16 characters. A long, unique passphrase works well.</p>
          <label>Confirm new password<input type="password" required minlength={16} maxlength={1024} autocomplete="new-password" bind:value={confirmation} /></label>
          {#if error}<div class="alert error" role="alert">{error}</div>{/if}
          {#if notice}<div class="alert success" role="status">{notice}</div>{/if}
          <button class="primary">{busy ? 'Changing password…' : 'Change password'}</button>
        </fieldset>
      </form>
    {:else if actor.provider === 'github'}
      <h2>Signed in with GitHub</h2>
      <p>Your password and two-factor authentication are managed by GitHub.</p>
      <a class="button" href="https://github.com/settings/security" target="_blank" rel="noreferrer">GitHub security settings ↗</a>
    {:else}
      <h2>Signed in as {actor.name}</h2>
      <p class="muted">Sign out and sign in again to refresh your account information.</p>
    {/if}
  </section>
  {#if view === 'users' && actor.admin}<section class="panel account-panel"><Users {actor} /></section>{/if}
  {#if view === 'agents' && actor.admin}<section class="panel account-panel"><AgentAccess bind:busy={agentBusy} /></section>{/if}
  {#if view === 'maintenance' && actor.admin}<section class="panel account-panel"><Maintenance bind:busy={resetBusy} workspaceDirty={workspaceDirty || dirty} onsuccess={onreset} /></section>{/if}
</div>

<style>
  .account-nav { flex-wrap: wrap; }
</style>
