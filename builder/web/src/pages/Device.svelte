<script lang="ts">
    import { onMount } from 'svelte';
    import Card from '../lib/ui/Card.svelte';
    import Icon from '../components/planner/PlannerIcon.svelte';
    import Drawing from './device/Drawing.svelte';
    import Software from './device/Software.svelte';
    import { deviceHolder } from '../lib/device/session.svelte';
    import { deviceName, rememberDevice, remembered, rememberedReady } from '../lib/device-sync/remembered.svelte';
    import { deviceSync } from '../lib/device-sync/state.svelte';
    import { formatBytes } from '../lib/format';
    import type { RouteRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { planTitle } from '../lib/planner/editor';
    import { isPlan } from '../lib/planner/library';
    import { ROUTES } from '../lib/routes';

    let { library }: { library: Library; active: boolean } = $props();

    type Named = { id: string; name: string };
    const siteBase = import.meta.env.VITE_SITE_BASE || '/';
    /** What the library holds for the remembered device: the routes that wait or leave, the routes on its card, and the
     * rides that came from its card. */
    let held = $state.raw<{ waiting: Named[]; leaving: number; sent: Named[]; rides: number; last: number | null }>(
        { waiting: [], leaving: 0, sent: [], rides: 0, last: null });
    let reads = 0;
    /** The rider clicked Disconnect, and the device did not connect again since. */
    let released = $state(false);
    /** The software row uses the client; a sync now would fail with `busy`. */
    let updating = $state(false);

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
    /** A device on the cable that is not the remembered one; Connect uses it with no chooser. */
    const other = $derived(view !== 'connected' && status === 'ready' && session?.info ? { name: deviceName(session.info.serialNumber), formatted: !!session.store } : null);
    $effect(() => { if (status === 'ready') released = false; });

    const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;
    const day = (date: Date) => date.toLocaleDateString('en-GB', { day: 'numeric', month: 'long' });
    function lastSync(at: number | null): string {
        if (at === null) return 'No sync yet';
        const date = new Date(at);
        return date.toDateString() === new Date().toDateString()
            ? `Last sync today at ${date.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' })}`
            : `Last sync on ${day(date)}`;
    }
    const report = $derived(deviceSync.report);
    const did = $derived(report ? [
        report.rides && `${plural(report.rides, 'ride')} came in`,
        report.sent && `${plural(report.sent, 'route')} sent`,
        report.removed && `${plural(report.removed, 'route')} removed`,
    ].filter(Boolean).join(' · ') : '');
    const more = $derived(device ? device.routes - held.sent.length : 0);
    const parts = $derived(device ? [
        { label: 'Map', bytes: device.bytes.map, colour: 'var(--water)' },
        { label: 'Routes', bytes: device.bytes.routes, colour: 'var(--route)' },
        { label: 'Rides', bytes: device.bytes.rides, colour: 'var(--rust)' },
    ] : []);

    const nameOf = (route: RouteRecord) => route.name || (isPlan(route) ? planTitle(route.trip) : 'Route');
    async function named(ids: string[]): Promise<Named[]> {
        const routes = await Promise.all(ids.map(id => library.item(id)));
        return routes.flatMap(route => route && 'revision' in route ? [{ id: route.id, name: nameOf(route) }] : []);
    }

    /** The rides come from `library.rides()`: a ride record names its card in `device`, and the library has no index of it. */
    async function read(card: string | undefined) {
        const mine = ++reads;
        const [waiting, leaving, sent, rides] = await Promise.all([library.state('waiting'), library.state('leaving'), library.state('sent'),
            card ? library.rides() : []]);
        const onCard = Object.entries(sent ?? {}).filter(([, copy]) => copy.device.startsWith(`${card}:`)).map(([id]) => id);
        const fromCard = rides.filter(ride => ride.device?.startsWith(`${card}:`));
        const next = { waiting: await named(waiting ?? []), leaving: leaving?.length ?? 0, sent: await named(onCard),
            rides: fromCard.length, last: fromCard.length ? Math.max(...fromCard.map(ride => ride.summary.start)) : null };
        if (mine === reads) held = next;
    }

    onMount(() => {
        void deviceHolder.open();
        return library.onChange(store => { if (store === 'state' || store === 'rides' || store === 'routes') void read(remembered.device?.storeId); });
    });
    $effect(() => { void read(device?.storeId); });

    /** A device that this page adopted is remembered as it is: a second chooser would open a second link to it. */
    function connect() {
        const current = session;
        if (!current) return;
        const adopted = current.status === 'ready' ? Promise.resolve(true) : current.requestDevice();
        void adopted.then(ok => {
            if (ok && current.status === 'ready' && current.info && current.store) rememberDevice(current.info, current.store);
        });
    }

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
</script>

{#snippet connectButton(label: string)}
    {#if !other?.formatted && other}
        <p class="note">The memory card of {other.name} needs a format. <a href={`${siteBase}builder/`} onclick={openBuilder}>The map builder</a> formats it.</p>
    {:else}
        <button type="button" class="button primary" onclick={connect} disabled={status === 'connecting'}>
            {status === 'connecting' ? 'Connecting' : other ? `Use ${other.name}` : label}
        </button>
    {/if}
{/snippet}

{#snippet links(routes: Named[])}
    {#each routes as route, i (route.id)}{#if i}{' · '}{/if}<a href={`${ROUTES.route}/${route.id}`}>{route.name}</a>{/each}
{/snippet}

{#if (view === 'connected' || view === 'before') && device}
    {@const connected = view === 'connected'}
    <main class="known">
        <header>
            <h1>{deviceName(device.serial)}</h1>
            <span class="state"><span class="dot" class:on={connected}></span>{connected ? 'Connected' : released ? 'Disconnected' : 'Not connected'}</span>
            <span class="actions">
                {#if connected}
                    <button type="button" class="button" onclick={release} disabled={deviceSync.running || updating}>Disconnect</button>
                {:else}
                    {@render connectButton('Connect')}
                {/if}
            </span>
        </header>
        {#if !connected && (other?.formatted || released || error)}
            <p class="note" class:problem={!!error} role={error ? 'alert' : undefined}>
                {error ?? (other ? `${other.name} is on the cable.` : 'The map builder can use the device now. Connect takes it back.')}
            </p>
        {/if}
        <div class="columns">
            <div class="stage">{#key connected}<Drawing live={connected} />{/key}</div>
            <Card>
                <div class="rows">
                    <div class="row">
                        <strong>Sync</strong>
                        <div>
                            {#if connected && deviceSync.running}
                                <p class="syncing"><span class="turning"><Icon name="sync" size={16} /></span>Syncing</p>
                                <p class="second">{lastSync(device.at)}</p>
                            {:else if deviceSync.problem}
                                {#if !connected}<p>{lastSync(device.at)}</p>{/if}
                                <p class="problem" class:second={!connected} role="alert">{deviceSync.problem}</p>
                            {:else if connected}
                                <p>{held.waiting.length || held.leaving ? `${plural(held.waiting.length + held.leaving, 'route')} to sync.` : 'In sync.'} {lastSync(device.at)}.</p>
                                {#if did}<p class="second">{did}</p>{/if}
                            {:else}
                                <p>{lastSync(device.at)}</p>
                            {/if}
                        </div>
                        {#if connected}
                            <div><button type="button" class="button" onclick={() => deviceSync.run()} disabled={deviceSync.running || updating}>{deviceSync.problem ? 'Try again' : 'Sync now'}</button></div>
                        {/if}
                    </div>
                    {#if connected}
                        {#if device.at !== null}
                        <div class="row">
                            <strong>Routes</strong>
                            <div>
                                <p>{device.routes} on the device</p>
                                {#if held.sent.length}<p class="second">{@render links(held.sent)}{#if more > 0}{` and ${more} more`}{/if}</p>{/if}
                            </div>
                        </div>
                        {/if}
                        <div class="row">
                            <strong>Rides</strong>
                            <div>
                                <p>{held.rides ? `${held.rides} in your library came from this device` : 'None in your library came from this device'}</p>
                                {#if held.last !== null}<p class="second">The last on {day(new Date(held.last * 1000))}</p>{/if}
                            </div>
                        </div>
                        {#if device.at !== null}
                            <div class="row">
                                <strong>Memory card</strong>
                                <div>
                                    <div class="bar">
                                        {#each parts as part (part.label)}
                                            {#if part.bytes}<span style:flex-grow={part.bytes} style:background={part.colour}></span>{/if}
                                        {/each}
                                    </div>
                                    <p class="second legend">
                                        {#each parts as part (part.label)}<span><i style:background={part.colour}></i>{part.label} {formatBytes(part.bytes)}</span>{/each}
                                    </p>
                                </div>
                            </div>
                            <div class="row"><strong>Map</strong><div><p>{device.map ?? 'No map'}</p></div></div>
                        {/if}
                        {#if session?.client}
                            <div class="row">
                                <strong>Software</strong>
                                <Software client={session.client} running={session.info?.firmwareRevision ?? null} busy={deviceSync.running} bind:working={updating} />
                            </div>
                        {/if}
                    {:else}
                        <div class="row">
                            <strong>Waits for the device</strong>
                            <div>
                                <p>{#if held.waiting.length}{plural(held.waiting.length, 'route')}: {@render links(held.waiting)}{:else}Nothing{/if}</p>
                                {#if held.leaving}<p class="second">{plural(held.leaving, 'route')} to remove from the device</p>{/if}
                            </div>
                        </div>
                        <p class="hint">Routes go to the device when you connect it.</p>
                    {/if}
                </div>
            </Card>
        </div>
    </main>
{:else}
    <main class="alone">
        {#if view === 'loading'}
            <h1>{device ? deviceName(device.serial) : 'Device'}</h1>
        {:else}
            <Card>
                <div class="part">
                    <div class="device"><span></span></div>
                    {#if view === 'cannot'}
                        <h1>This browser cannot connect to the device</h1>
                        <p>The cable connection works in Chrome and in Edge. Your routes and rides are stored in this browser, and another browser does not have them.</p>
                        <div class="box foot"><strong>What works here</strong>Plan routes, keep rides, and export GPX and FIT files. Only the cable connection is missing.</div>
                    {:else}
                        <h1>Connect your OpenBikeComputer</h1>
                        <p>{other ? `${other.name} is on the cable.` : 'Connect it with the USB cable. Then press the button, and choose the device in the window of the browser.'}</p>
                        {@render connectButton('Connect a device')}
                        {#if error}<p class="problem" role="alert">{error}</p>{/if}
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
        min-height: 34px;
        padding: 5px 14px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        white-space: nowrap;
    }
    .button:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .primary, .primary:hover:not(:disabled) {
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

    .known {
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
        border: 1.5px solid var(--ink-soft);
        border-radius: 50%;
    }
    .dot.on {
        border-color: var(--connected);
        background: var(--connected);
    }
    .actions {
        display: flex;
        align-items: center;
        gap: 12px;
        margin-left: auto;
    }
    .note {
        color: var(--ink-soft);
    }
    .columns {
        display: grid;
        flex: 1;
        grid-template-columns: minmax(0, 1fr) minmax(0, 1.6fr);
        gap: 14px;
        align-items: start;
        min-height: 0;
    }
    .stage {
        display: grid;
        place-items: center;
        align-self: stretch;
        padding: 32px 24px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--parchment-2);
    }
    .rows {
        margin: -16px 0;
    }
    .row {
        display: grid;
        grid-template-columns: 150px minmax(0, 1fr) auto;
        gap: 4px 20px;
        align-items: start;
        padding: 16px 0;
    }
    .row + .row {
        border-top: 1px solid var(--line);
    }
    .row strong {
        padding-top: 1px;
    }
    .second {
        margin-top: 4px;
        color: var(--ink-soft);
        font-size: 13.5px;
    }
    .rows a {
        color: var(--ink);
    }
    .hint {
        padding: 14px 0 16px;
        border-top: 1px solid var(--line);
        color: var(--ink-soft);
        font-size: 13.5px;
    }
    .syncing {
        display: flex;
        align-items: center;
        gap: 6px;
    }
    .turning {
        display: inline-grid;
        animation: turn 1.2s linear infinite;
    }
    @keyframes turn {
        to { transform: rotate(360deg); }
    }
    @media (prefers-reduced-motion: reduce) {
        .turning { animation: none; }
    }
    /* The used parts only: the cable reports no card size. A small part keeps a few pixels. */
    .bar {
        display: flex;
        gap: 2px;
        height: 10px;
        margin-top: 5px;
    }
    .bar span {
        flex-basis: 0;
        min-width: 4px;
        border-radius: 2px;
    }
    .legend {
        display: flex;
        flex-wrap: wrap;
        gap: 4px 16px;
    }
    .legend span {
        display: flex;
        align-items: center;
        gap: 6px;
    }
    .legend i {
        width: 10px;
        height: 10px;
        border-radius: 2px;
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
    /* A device with no picture. */
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
        .row { grid-template-columns: minmax(0, 1fr) auto; }
        .row strong { grid-column: 1 / -1; }
    }
</style>
