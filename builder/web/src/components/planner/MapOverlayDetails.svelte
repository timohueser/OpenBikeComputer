<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import TrailMarker from './TrailMarker.svelte';
    import { networkLevels, networkScope, routeKindTitles, routeWebsite, type NetworkRoute, type OverlaySelection } from '../../lib/planner/route-overlays';
    let { selection, theme = 'light', onclose }: { selection: OverlaySelection; theme?: 'light' | 'dark'; onclose: () => void } = $props();
    // Relations with one name, number and network are one route to a rider: a route and its parts, or its two directions.
    const routes = $derived.by(() => {
        const groups = new Map<string, NetworkRoute[]>();
        for (const route of selection.routes ?? []) {
            const key = route.name || route.ref ? JSON.stringify([route.name, route.ref, route.network]) : String(route.id);
            groups.set(key, [...groups.get(key) ?? [], route]);
        }
        return [...groups.values()].map(members => ({ route: members[0], website: members.map(route => routeWebsite(route.website)).find(Boolean) }));
    });
    const levelColor = (rank: number) => networkLevels.find(level => level.rank === Math.min(rank, 3))?.[theme === 'dark' ? 'dark' : 'color'];
    const access: Record<string, { title: string; detail: string }> = {
        construction: { title: 'Under construction', detail: 'OSM maps this section as construction. The router excludes it.' },
        closed: { title: 'No access', detail: 'The mapped rules exclude this travel mode. This is not just a requirement to dismount.' },
        private: { title: 'Private access', detail: 'Permission is required. The router avoids this road where it can, and the route notes it.' },
        limited: { title: 'Limited access', detail: 'Access is limited, for example to destinations, farm traffic or permit holders. The router avoids this road where it can, and the route notes it.' },
        push: { title: 'Dismount and push', detail: 'Riding is not allowed here. You can push your bike; the router can include this as a walking section.' },
        no_bikes: { title: 'No bicycles', detail: 'Walking is allowed, but taking a bicycle through is restricted, including pushing.' },
        directional: { title: 'Directional access', detail: 'Access differs by direction. Check the mapped rules below.' },
        conditional: { title: 'Conditional access', detail: 'Access depends on conditions. The router does not evaluate them; the route notes them.' },
    };
    const restriction = $derived(access[selection.status ?? ''] ?? access.closed);
    const title = $derived(selection.kind === 'access'
        ? restriction.title
        : routeKindTitles[selection.kind]);
    function permission(values: boolean[] | undefined, bit: number) {
        if ((selection.conditional ?? 0) & bit) return 'Conditional';
        return values?.every(Boolean) ? 'Allowed' : values?.some(Boolean) ? 'One direction' : 'Not allowed';
    }
</script>

<section class="overlay-details" aria-label={title}>
    <h2>{title}</h2>
    <button class="close" aria-label="Close map details" onclick={onclose}><Icon name="close" size={15} /></button>
    {#if selection.kind === 'access'}
        <strong>{[selection.name, selection.ref].filter(Boolean).join(' · ') || 'Road or path'}</strong>
        <p>{restriction.detail}</p>
        <dl class="permissions"><dt>Riding</dt><dd>{permission(selection.riding, 1)}</dd><dt>Pushing a bike</dt><dd>{permission(selection.pushing, 4)}</dd><dt>Walking</dt><dd>{permission(selection.walking, 2)}</dd></dl>
        <details><summary>Mapped access details</summary><dl>{#each Object.entries(selection.tags ?? {}) as [key, value] (key)}<dt>{key}</dt><dd>{value}</dd>{/each}</dl></details>
        <p class="note"><a href={`https://www.openstreetmap.org/way/${selection.way}`} target="_blank" rel="noreferrer">OpenStreetMap</a> · Closure reports are not live.</p>
    {:else}
        <!-- The badge shows the first of the numbers that OSM joins with ";". -->
        <ul>{#each routes as { route, website } (route.id)}
            {@const scope = networkScope(route)}
            <li>
                <span class="marker"><TrailMarker symbol={route.symbol} description={route.symbol_text ?? ''} ref={route.ref?.split(';')[0]} /></span>
                <span class="body">
                    <strong>{route.name || route.ref || 'Unnamed route'}</strong>
                    {#if scope || website}<small>
                        {#if scope}<i style:background={levelColor(route.rank)}></i>{scope}{/if}{#if scope && website}{' · '}{/if}
                        {#if website}<a href={website} target="_blank" rel="noreferrer">{new URL(website).hostname.replace(/^www\./, '')}</a>{/if}
                    </small>{/if}
                </span>
            </li>
        {/each}</ul>
    {/if}
</section>

<style>
    /* The map sets the free room; the content scrolls as a last resort. */
    .overlay-details { position: relative; box-sizing: border-box; width: 340px; max-width: var(--callout-width); max-height: var(--callout-room); overflow-y: auto; padding: 16px; color: var(--ink); font: 400 13px var(--sans); }
    h2 { margin: 0 28px 12px 0; font: 600 14px var(--sans); }
    button { border: 0; background: none; color: inherit; font: inherit; cursor: pointer; }
    .close { position: absolute; top: 10px; right: 10px; display: grid; place-items: center; width: 28px; height: 28px; border-radius: 6px; color: var(--ink-soft); }
    .close:hover { background: var(--parchment-2); color: var(--ink); }
    strong { display: block; font: 600 14px/1.4 var(--sans); overflow-wrap: anywhere; }
    p { margin: 8px 0; line-height: 1.5; }
    a { color: var(--link); text-underline-offset: 3px; overflow-wrap: anywhere; }
    .note { margin-bottom: 0; color: var(--ink-soft); font-size: 12px; }
    details { margin: 10px 0; }
    summary { cursor: pointer; }
    dl { margin: 8px 0; overflow-wrap: anywhere; }
    dt { color: var(--ink-soft); font-size: 12px; }
    dd { margin: 0 0 8px; }
    .permissions { display: grid; grid-template-columns: 1fr auto; gap: 5px 16px; }
    .permissions dd { margin: 0; font-size: 12px; }
    ul { display: grid; gap: 12px; margin: 0; padding: 0; list-style: none; }
    li { display: flex; align-items: flex-start; gap: 10px; }
    .marker { flex: none; display: flex; min-width: 24px; }
    .body { flex: 1; min-width: 0; }
    small { display: block; margin-top: 2px; font: 400 13px/1.4 var(--sans); color: var(--ink-soft); }
    /* The colour of the route's network on the map. */
    small i { display: inline-block; width: 8px; height: 8px; margin-right: 6px; border-radius: 50%; }
</style>
