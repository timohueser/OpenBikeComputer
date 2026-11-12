<script lang="ts">
    import { onMount } from 'svelte';
    import Card from '../lib/ui/Card.svelte';
    import Segmented from '../lib/ui/Segmented.svelte';
    import Window from '../lib/ui/Window.svelte';
    import { saveBlob } from '../lib/download';
    import { loadBackup, saveBackup } from '../lib/library/backup';
    import type { Library } from '../lib/library/store';
    import { readSettings, settings, updateSettings, type RiderValue, type Units } from '../lib/settings/settings.svelte';
    import { appearance, setLook, type Look } from '../lib/theme.svelte';

    let { library }: { library: Library; active: boolean } = $props();

    const units: { value: Units; label: string }[] = [{ value: 'km', label: 'Kilometres' }, { value: 'mi', label: 'Miles' }];
    const looks: { value: Look; label: string }[] = [{ value: 'system', label: 'As the computer' }, { value: 'light', label: 'Light' }, { value: 'dark', label: 'Dark' }];
    const values = [
        { key: 'maxHr', label: 'Maximum heart rate', unit: 'bpm' },
        { key: 'ftp', label: 'Threshold power (FTP)', unit: 'W' },
    ] as const;

    let counts = $state<{ routes: number; rides: number } | null>(null);
    let message = $state('');
    let busy = $state(false);
    let confirming = $state(false);
    let failure = $state('');
    let file: HTMLInputElement;

    const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;
    const text = (error: unknown) => error instanceof Error ? error.message : String(error);
    const day = (at: number) => new Date(at * 1000).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });

    async function count() {
        const [routes, rides] = await Promise.all([library.routes(), library.rides()]);
        counts = { routes: routes.length, rides: rides.length };
    }
    onMount(() => { count().catch(error => message = text(error)); });

    /** Empty is "not set"; `readSettings` moves a value out of range to the nearest end, and the field shows it. */
    function setValue(key: 'maxHr' | 'ftp', field: HTMLInputElement) {
        // An entry that the browser cannot read as a number also has an empty value.
        if (field.validity.badInput) {
            field.value = String(settings[key]?.value ?? '');
            return;
        }
        const value: RiderValue | null = field.value === '' ? null : { value: Math.round(Number(field.value)), at: Math.floor(Date.now() / 1000), device: false };
        updateSettings({ [key]: value });
        field.value = String(settings[key]?.value ?? '');
    }

    async function run(work: () => Promise<string | void>) {
        busy = true;
        message = '';
        try {
            message = await work() ?? '';
            await count();
        } catch (error) {
            message = text(error);
        } finally {
            busy = false;
        }
    }

    const save = () => run(async () => {
        saveBlob(await saveBackup(library, $state.snapshot(settings)), `openbikecomputer-${new Date().toLocaleDateString('en-CA')}.zip`);
    });

    function load(chosen: File | undefined) {
        file.value = '';
        if (!chosen) return;
        void run(async () => {
            const added = await loadBackup(library, chosen);
            if (added.settings) updateSettings(readSettings(added.settings));
            return `Added ${plural(added.routes, 'route')} and ${plural(added.rides, 'ride')}.`;
        });
    }

    async function deleteAll() {
        busy = true;
        failure = '';
        try {
            await library.clear();
            location.reload();
        } catch (error) {
            failure = text(error);
            busy = false;
        }
    }
</script>

<main>
    <h1>Settings</h1>
    <div class="cards">
        <Card label="Rider">
            <div class="row">
                <strong>Units</strong>
                <Segmented label="Units" options={units} value={settings.units} onChange={value => updateSettings({ units: value })} />
            </div>
            <div class="row">
                <strong>Look</strong>
                <Segmented label="Look" options={looks} value={appearance.look} onChange={setLook} />
            </div>
            {#each values as { key, label, unit } (key)}
                {@const value = settings[key]}
                <div class="row">
                    <div>
                        <label for={key}><strong>{label}</strong></label>
                        {#if value?.device}<p>From the OpenBikeComputer, with the ride of {day(value.at)}</p>{/if}
                    </div>
                    <span class="value">
                        <input id={key} type="number" inputmode="numeric" value={value?.value ?? ''} onchange={event => setValue(key, event.currentTarget)} />
                        <span class="unit">{unit}</span>
                    </span>
                </div>
            {/each}
            <p class="note">The zones come from these two values. A new ride from the OpenBikeComputer brings the values of the device, and they replace the values here. A ride from a file takes the values that are here. A ride keeps its values when you change them later.</p>
        </Card>
        <Card label="Your library">
            <p class="count">
                {#if counts}<strong>{plural(counts.routes, 'route')} and {plural(counts.rides, 'ride')}</strong> are saved in this browser, on this computer only.{/if}
            </p>
            <div class="buttons">
                <button type="button" onclick={save} disabled={busy || !counts || counts.routes + counts.rides === 0}>Save a backup file</button>
                <button type="button" onclick={() => file.click()} disabled={busy}>Load a backup file</button>
                <input bind:this={file} type="file" accept=".zip,application/zip" hidden onchange={event => load(event.currentTarget.files?.[0])} />
                <button type="button" class="danger" onclick={() => confirming = true} disabled={busy}>Delete all</button>
            </div>
            <p class="message" role="status">{message}</p>
        </Card>
    </div>
</main>

{#if confirming}
    <Window title="Delete all?" onClose={() => { confirming = false; failure = ''; }}>
        {#snippet children(close)}
        <p>Your routes and rides leave this browser. The device keeps its own, and its rides come in again at the next connection. Close other tabs of this app first.</p>
        {#if failure}<p class="message" role="alert">{failure}</p>{/if}
        <div class="buttons end">
            <button type="button" onclick={close}>Cancel</button>
            <button type="button" class="danger" onclick={deleteAll} disabled={busy}>Delete all</button>
        </div>
        {/snippet}
    </Window>
{/if}

<style>
    main {
        max-width: 1280px;
        padding: 16px 18px 24px;
    }
    h1 {
        margin: 0 0 16px;
        font: 700 26px/1.2 var(--sans);
    }
    .cards {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 16px;
        align-items: start;
    }
    .row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        padding: 9px 0;
        border-top: 1px solid var(--line);
    }
    p {
        margin: 4px 0 0;
    }
    .row p, .note {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .note {
        margin: 0;
        padding-top: 12px;
        border-top: 1px solid var(--line);
    }
    .value {
        display: flex;
        flex: none;
        align-items: center;
        gap: 10px;
    }
    input {
        width: 72px;
        min-height: 32px;
        padding: 4px 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        text-align: right;
    }
    .unit {
        width: 28px;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .count {
        margin: 0 0 12px;
    }
    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }
    .end {
        justify-content: flex-end;
        margin-top: 16px;
    }
    button {
        min-height: 32px;
        padding: 5px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
    }
    button:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .danger {
        border-color: var(--coral);
        color: var(--coral);
    }
    .buttons:not(.end) .danger {
        margin-left: auto;
    }
    .message {
        margin-top: 10px;
    }
    .message:empty {
        margin: 0;
    }
    @media (max-width: 760px) {
        main { padding-inline: 16px; }
        .cards { grid-template-columns: minmax(0, 1fr); }
        .row { flex-wrap: wrap; }
    }
</style>
