<script lang="ts">
  import { onMount } from 'svelte';
  import type { Actor, ApprovedGitHubUser } from '$lib/types';
  import { api, message } from './api';

  export let actor: Actor;
  let users: ApprovedGitHubUser[] = [];
  let oauth = false;
  let loading = true;
  let busy = false;
  let login = '';
  let admin = false;
  let error = '';
  let notice = '';

  async function load() {
    loading = true; error = '';
    try { ({ users, oauth } = await api<{ users: ApprovedGitHubUser[]; oauth: boolean }>('/api/users')); }
    catch (e) { error = message(e); }
    finally { loading = false; }
  }

  async function add() {
    busy = true; error = ''; notice = '';
    try {
      ({ users, oauth } = await api<{ users: ApprovedGitHubUser[]; oauth: boolean }>('/api/users', 'POST', { login: login.trim(), admin }));
      login = ''; admin = false; notice = 'GitHub account approved. They can now sign in when GitHub login is configured.';
    } catch (e) { error = message(e); }
    finally { busy = false; }
  }

  async function remove(user: ApprovedGitHubUser) {
    if (!confirm(`Remove ${user.login}'s access? Their active sessions will end. Saved revisions and test evidence will remain.`)) return;
    busy = true; error = ''; notice = '';
    try {
      await api(`/api/users/${encodeURIComponent(user.id)}`, 'DELETE');
      users = users.filter(value => value.id !== user.id);
      notice = `${user.login}'s access was removed. Their existing records are preserved.`;
    } catch (e) { error = message(e); }
    finally { busy = false; }
  }

  onMount(load);
</script>

<div class="row"><h2>Approved GitHub accounts</h2><button disabled={loading || busy} on:click={load}>Refresh</button></div>
<p class="muted">Approved users can edit requirements, record test results, and manage releases. Administrators can also add and remove users.</p>
{#if error}<div class="alert error" role="alert">{error}</div>{/if}
{#if notice}<div class="alert success" role="status">{notice}</div>{/if}
{#if loading}
  <p class="muted" role="status">Loading users…</p>
{:else}
  {#if !oauth}<div class="alert warning">GitHub sign-in is not configured yet. You can prepare the approved list now. A server administrator must configure GitHub login before these accounts can sign in.</div>{/if}
  <form class="inset" on:submit|preventDefault={add}>
    <h3>Add a user</h3>
    <fieldset disabled={busy}>
      <label>GitHub username<input required maxlength={39} pattern={'[A-Za-z0-9](?:[A-Za-z0-9]|-){0,38}'} title="Enter a GitHub username without @" bind:value={login} placeholder="octocat" autocomplete="off" autocapitalize="none" spellcheck={false} /></label>
      <label class="check"><input type="checkbox" bind:checked={admin} />Administrator — can manage the approved users</label>
      <button class="primary">{busy ? 'Saving…' : 'Approve GitHub account'}</button>
    </fieldset>
  </form>
  <div class="user-list">
    {#each users as user (user.id)}
      {@const ownAccount = actor.provider === 'github' && actor.userId === user.id}
      <div class="test row">
        <div><a href={`https://github.com/${encodeURIComponent(user.login)}`} target="_blank" rel="noreferrer">{user.login} ↗</a><div class="small muted">{user.admin ? 'Administrator' : 'Maintainer'}{ownAccount ? ' · Your account' : ''}</div></div>
        <button class="text-button danger" disabled={busy || ownAccount} title={ownAccount ? 'You cannot remove your own access.' : undefined} on:click={() => remove(user)}>Remove access</button>
      </div>
    {:else}
      <p class="muted">No GitHub accounts are approved yet. The local administrator can still sign in.</p>
    {/each}
  </div>
  <p class="small muted section">The local administrator is a separate recovery account. Removing a GitHub user does not remove their saved work.</p>
{/if}
