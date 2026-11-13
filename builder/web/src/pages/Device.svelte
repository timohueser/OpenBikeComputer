<script lang="ts">
    import { onMount } from 'svelte';
    import Card from '../lib/ui/Card.svelte';
    import Drawing from './device/Drawing.svelte';
    import { deviceHolder } from '../lib/device/session.svelte';
    import { deviceName, rememberDevice, remembered, rememberedReady } from '../lib/device-sync/remembered.svelte';
    import { deviceSync } from '../lib/device-sync/state.svelte';
    import { formatBytes } from '../lib/format';
    import type { Library } from '../lib/library/store';
    import { planTitle } from '../lib/planner/editor';
    import { isPlan } from '../lib/planner/library';

    let { library }: { library: Library; active: boolean } = $props();

    const siteBase = import.meta.env.VITE_SITE_BASE || '/';
    /** The names of the routes that wait; a waiting id with no route does not count. */
    let waits = $state.raw<{ names: string[]; leaving: number }>({ names: [], leaving: 0 });
    let reads = 0;
    /** The rider clicked Disconnect, and the device did not connect again since. */
    let released = $state(false);

    const session = $derived(deviceHolder.session);
    const status = $derived(session?.status);
    const device = $derived(remembered.device);
    const raw = $derived(deviceHolder.error ?? (status === 'error' ? session?.error : null));
    // Chrome names `claimInterface` when another page holds the device.
    const error = $derived(raw && /claim/i.test(raw) ? 'Another tab or the map builder uses the device. Close it, then connect again.' : raw);
    const view = $derived(
        status === 'unsupported' ? 'cannot'
            : !session && !deviceHolder.error ? 'loading'
            : rememberedReady() ? 'connected'
            : device ? 'before' : 'never');
    $effect(() => { if (status === 'ready') released = false; });

    function release() {
        released = true;
        void session?.disconnect();
    }

    /** The map builder can claim the device only after this tab lets it go. */
    function openBuilder(event: MouseEvent) {
        event.preventDefault();
        const href = (event.currentTarget as HTMLAnchorElement).href;
        void Promise.resolve(session?.disconnect()).finally(() => location.assign(href));
    }

    /** A device on the cable that is not the remembered one; Connect uses it with no chooser. */
    const other = $derived(view !== 'connected' && status === 'ready' && session?.info ? { name: deviceName(session.info.serialNumber), formatted: !!session.store } : null);

    const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;
    const day = (date: Date) => date.toLocaleDateString('en-GB', { day: 'numeric', month: 'long' });
    const today = () => new Date().toLocaleDateString('en-CA');
    const came = $derived(!device?.came ? 'None came in yet'
        : `${device.came.count} came in ${device.came.day === today() ? 'today' : `on ${day(new Date(`${device.came.day}T12:00`))}`}`);
    const count = $derived(waits.names.length);
    const sends = $derived(count || waits.leaving
        ? [count && `${count} wait${count === 1 ? 's' : ''}`, waits.leaving && `${waits.leaving} leave${waits.leaving === 1 ? 's' : ''}`].filter(Boolean).join(' · ')
        : 'Nothing waits');

    async function read() {
        const mine = ++reads;
        const [waiting, leaving] = await Promise.all([library.state('waiting'), library.state('leaving')]);
        const routes = await Promise.all((waiting ?? []).map(id => library.item(id)));
        const names = routes.flatMap(route => route && 'revision' in route ? [route.name || (isPlan(route) ? planTitle(route.trip) : 'Route')] : []);
        if (mine === reads) waits = { names, leaving: leaving?.length ?? 0 };
    }

    onMount(() => {
        void deviceHolder.open();
        void read();
        return library.onChange(store => { if (store === 'state') void read(); });
    });

    /** A device that this page adopted is remembered as it is: a second chooser would open a second link to it. */
    function connect() {
        const current = session;
        if (!current) return;
        const adopted = current.status === 'ready' ? Promise.resolve(true) : current.requestDevice();
        void adopted.then(ok => {
            if (ok && current.status === 'ready' && current.info && current.store) rememberDevice(current.info, current.store);
        });
    }
</script>

{#snippet connectButton(label: string)}
    {#if other && !other.formatted}
        <p>The memory card of {other.name} needs a format. <a href={`${siteBase}builder/`} onclick={openBuilder}>The map builder</a> formats it.</p>
    {:else}
        {#if other}<p>{other.name} is on the cable.</p>{/if}
        <button type="button" class="button primary" onclick={connect} disabled={status === 'connecting'}>
            {status === 'connecting' ? 'Connecting' : other ? `Use ${other.name}` : label}
        </button>
    {/if}
{/snippet}

{#snippet problem(text: string | null | undefined)}
    {#if text}<p class="problem" role="alert">{text}</p>{/if}
{/snippet}

{#if view === 'connected' && device}
    <main class="connected">
        <header>
            <h1>{deviceName(device.serial)}</h1>
            <span class="state"><span class="dot"></span>{deviceSync.running ? 'Updating' : 'Connected'}</span>
            <button type="button" class="button disconnect" onclick={release} disabled={deviceSync.running}>Disconnect</button>
            <button type="button" class="button primary" onclick={() => deviceSync.run()} disabled={deviceSync.running}>Update the device</button>
        </header>
        {@render problem(deviceSync.problem)}
        <div class="columns">
            <Card label="The update sends">
                <div class="rows">
                    <div class="row"><strong>Routes</strong><span class="tag" class:quiet={!count && !waits.leaving}>{sends}</span></div>
                </div>
                <h2>On the device</h2>
                <div class="rows">
                    <div class="row"><strong>Rides</strong><span>{came}</span></div>
                    {#if device.at !== null}
                        <div class="row"><strong>Memory card</strong><span>map {formatBytes(device.bytes.map)} · routes {formatBytes(device.bytes.routes)} · rides {formatBytes(device.bytes.rides)}</span></div>
                    {/if}
                </div>
            </Card>
            <div class="stage"><Drawing /></div>
        </div>
    </main>
{:else}
    <main class="alone">
        {#if view === 'loading'}
            <h1>{device ? deviceName(device.serial) : 'Device'}</h1>
        {:else}
            <Card>
                <div class="part">
                    <div class="device" class:known={view === 'before'}><span></span></div>
                    {#if view === 'cannot'}
                        <h1>This browser cannot connect to the device</h1>
                        <p>The cable connection works in Chrome and in Edge. Your routes and rides are stored in this browser, and another browser does not have them.</p>
                        <div class="box foot"><strong>What works here</strong>Plan routes, keep rides, and export GPX and FIT files. Only the cable connection is missing.</div>
                    {:else if view === 'before' && device}
                        <h1>{released ? 'Your device is disconnected' : 'Your device is not connected'}</h1>
                        {#if released}<p>The map builder can use it now. “Connect it again” takes it back.</p>{/if}
                        {#if device.at !== null}<p>Last update on {day(new Date(device.at))}.</p>{/if}
                        {@render connectButton('Connect it again')}
                        {@render problem(error)}
                        <div class="box">
                            <strong>Waits for the next connection</strong>
                            {#if waits.names.length}<span>{plural(count, 'route')}: {waits.names.map(name => `“${name}”`).join(', ')}</span>{/if}
                            {#if waits.leaving}<span>{plural(waits.leaving, 'route')} to remove from the device</span>{/if}
                            {#if !count && !waits.leaving}<span>Nothing</span>{/if}
                        </div>
                        {#if device.at !== null}
                            <div class="box">
                                <strong>On the device at the last update</strong>
                                {#if device.map}<span>Map: {device.map}</span>{/if}
                                <span>{plural(device.routes, 'route')}</span>
                            </div>
                        {/if}
                        {@render problem(deviceSync.problem)}
                    {:else}
                        <h1>Connect your OpenBikeComputer</h1>
                        {#if !other}<p>Connect it with the USB cable. Then press the button, and choose the device in the window of the browser.</p>{/if}
                        {@render connectButton('Connect a device')}
                        {@render problem(error)}
                        <div class="box foot">
                            <strong>You have no OpenBikeComputer?</strong>
                            All of this product works without one. Take a route to another device as a GPX or FIT file from its page.
                            <a href={siteBase}>What the OpenBikeComputer is</a>
                        </div>
                    {/if}
                </div>
            </Card>
        {/if}
    </main>
{/if}

<style>
    main {
        padding: 16px 20px 24px;
    }
    h1 {
        margin: 0;
        font: 700 26px/1.2 var(--sans);
    }
    p {
        margin: 0;
    }
    .button {
        min-height: 36px;
        padding: 6px 16px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font-size: 15px;
    }
    .primary {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    .button:disabled {
        opacity: .6;
    }
    .problem {
        color: var(--coral);
    }

    .connected {
        display: flex;
        flex-direction: column;
        gap: 14px;
        height: 100%;
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 10px 24px;
    }
    .state {
        display: flex;
        align-items: center;
        gap: 7px;
        color: var(--ink-soft);
    }
    .dot {
        width: 9px;
        height: 9px;
        border-radius: 50%;
        background: var(--forest);
    }
    .disconnect {
        margin-left: auto;
    }
    .columns {
        display: grid;
        flex: 1;
        grid-template-columns: 300px 1fr;
        gap: 14px;
        min-height: 0;
    }
    h2 {
        margin: 20px 0 0;
        color: var(--ink-soft);
        font: 700 11px/1.3 var(--sans);
        letter-spacing: .06em;
        text-transform: uppercase;
    }
    .rows {
        margin-top: 8px;
    }
    .row {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        justify-content: space-between;
        gap: 4px 12px;
        padding: 10px 0;
        border-top: 1px solid var(--line);
        color: var(--ink);
        text-decoration: none;
    }
    .row span {
        color: var(--ink-soft);
        font-size: 13px;
    }
    .tag {
        padding: 1px 7px;
        border-radius: 4px;
        background: color-mix(in srgb, var(--amber) 28%, var(--panel));
    }
    .tag.quiet {
        background: none;
    }
    .stage {
        display: grid;
        place-items: center;
        padding: 24px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--parchment-2);
    }

    .alone {
        display: grid;
        justify-items: center;
    }
    .alone > :global(section) {
        width: min(440px, 100%);
        padding: 28px 24px;
    }
    .part {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 14px;
        text-align: center;
    }
    .part p {
        color: var(--ink-soft);
    }
    /* A device with no picture: grey, or dark with a green screen once it was connected. */
    .device {
        display: grid;
        place-items: center;
        width: 120px;
        height: 170px;
        border-radius: 16px;
        background: color-mix(in srgb, var(--ink-faint) 70%, var(--panel));
    }
    .device span {
        width: 98px;
        height: 124px;
        border-radius: 3px;
        background: color-mix(in srgb, var(--ink-faint) 40%, var(--panel));
    }
    .device.known {
        background: #55534a;
    }
    .device.known span {
        background: #cdeebd;
    }
    .box {
        display: flex;
        flex-direction: column;
        gap: 2px;
        align-self: stretch;
        padding: 10px 12px;
        border-radius: 8px;
        background: var(--parchment-2);
        font-size: 13px;
        text-align: left;
    }
    .box a {
        color: var(--ink);
    }
    .foot {
        margin-top: 24px;
    }
    @media (max-width: 760px) {
        main { padding-inline: 16px; }
        .columns { grid-template-columns: minmax(0, 1fr); }
    }
</style>
