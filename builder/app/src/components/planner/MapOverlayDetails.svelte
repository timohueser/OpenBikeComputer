<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import TrailMarker from './TrailMarker.svelte';
    import { trailMarker } from '../../lib/planner/trail-markers';
    import { networkName, routeKindTitles, routeWebsite, type OverlaySelection } from '../../lib/planner/route-overlays';
    let { selection, onclose, onuse }: { selection: OverlaySelection; onclose: () => void; onuse: () => void } = $props();
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
    <div class="heading"><strong>{title}</strong><button class="close" aria-label="Close map details" onclick={onclose}><Icon name="close" size={16} /></button></div>
    {#if selection.kind === 'access'}
        <b>{[selection.name, selection.ref].filter(Boolean).join(' · ') || 'Road or path'}</b>
        <p>{restriction.detail}</p>
        <dl class="permissions"><dt>Riding</dt><dd>{permission(selection.riding, 1)}</dd><dt>Pushing a bike</dt><dd>{permission(selection.pushing, 4)}</dd><dt>Walking</dt><dd>{permission(selection.walking, 2)}</dd></dl>
        <details><summary>Mapped access details</summary><dl>{#each Object.entries(selection.tags ?? {}) as [key, value] (key)}<dt>{key}</dt><dd>{value}</dd>{/each}</dl></details>
        <a href={`https://www.openstreetmap.org/way/${selection.way}`} target="_blank" rel="noreferrer">View road in OpenStreetMap</a>
    {:else}
        <ul>{#each selection.routes ?? [] as route (route.id)}
            {@const website = routeWebsite(route.website)}
            <li>
                {#if route.kind === 'hiking' && route.symbol && trailMarker(route.symbol)}<TrailMarker symbol={route.symbol} description={route.symbol_text ?? ''} />{/if}
                <div>
                    {#if website}<a href={website} target="_blank" rel="noreferrer">{route.name || route.ref || 'Unnamed route'}{route.name && route.ref ? ` · ${route.ref}` : ''}</a>
                    {:else}<b class="route-name">{route.name || route.ref || 'Unnamed route'}{route.name && route.ref ? ` · ${route.ref}` : ''}</b>{/if}
                    <span>{networkName(route)}</span>
                    {#if route.kind === 'hiking' && route.symbol_text}<span>{route.symbol_text}</span>{/if}
                </div>
            </li>
        {/each}</ul>
    {/if}
    <p class="source">From the regional OSM snapshot.{selection.kind === 'access' ? ' This is not live closure information.' : ''}</p>
    <button class="use" onclick={onuse}>Use this location</button>
</section>

<style>
    .overlay-details { position: absolute; z-index: 3; left: 12px; bottom: 38px; width: min(320px, calc(100% - 80px)); max-height: calc(100% - 54px); overflow: auto; padding: 14px; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); color: var(--ink); font-size: 13px; }
    .heading { display: flex; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 8px; }
    strong { font-size: 15px; }
    b { font-weight: 600; }
    p { margin: 8px 0; line-height: 1.45; }
    .source { color: var(--ink-soft); font-size: 12px; margin-top: 12px; }
    button { cursor: pointer; color: var(--ink); background: var(--panel); border: 1px solid var(--line); border-radius: 6px; padding: 6px 10px; font: inherit; }
    button:hover { background: var(--parchment-2); }
    .close { display: grid; place-items: center; padding: 6px; border: 0; }
    .use { width: 100%; margin-top: 4px; }
    a { color: var(--link); text-underline-offset: 3px; overflow-wrap: anywhere; }
    details { margin: 10px 0; }
    summary { cursor: pointer; }
    dl { margin: 8px 0; overflow-wrap: anywhere; }
    dt { color: var(--ink-soft); font-size: 12px; }
    dd { margin: 0 0 8px; }
    .permissions { display: grid; grid-template-columns: 1fr auto; gap: 5px 16px; }
    .permissions dd { margin: 0; font-size: 12px; }
    ul { margin: 0; padding: 0; list-style: none; }
    li { display: flex; align-items: flex-start; gap: 10px; }
    li + li { margin-top: 12px; }
    .route-name { font-weight: 500; overflow-wrap: anywhere; }
    li span { display: block; color: var(--ink-soft); font-size: 12px; margin-top: 3px; }
</style>
