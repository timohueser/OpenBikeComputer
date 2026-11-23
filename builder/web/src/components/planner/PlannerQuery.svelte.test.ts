// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { writable, fromStore } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';
import Query from './PlannerQuery.svelte';
import type { SearchContext, SearchState, QueryRequest } from '../../lib/planner/search/types';
import '../../../test-support/planner/animations';

const context: SearchContext = {view:[7.8,47.9,8,48.1],plan:{coordinates:[[7.8,48],[7.9,48]],km:[0,7.44],days:[{number:1,from:0,to:4,rest:false},{number:2,from:4,to:7,rest:false}],points:[]}};
const reply = (request: QueryRequest) => ({ok:true,json:async()=>({type:'places',request,results:[]})}) as Response;
/** The requests of the query model for the sentences of these tests. */
const READ: Record<string, QueryRequest> = {
    'drinking water in the middle of day 1': { type: 'places', what: ['drinking_water'], where: { day: 1, part: 'middle' } },
    'supermarket': { type: 'places', what: ['supermarket'] },
    'sunny cafe or bakery': { type: 'places', what: ['cafe', 'bakery'], ignored: ['sunny'] },
    'water today': { type: 'places', what: ['water'], where: { day: 'today' } },
};
/** A server that answers the request it was sent, else the request that the model reads. */
const server = () => vi.fn(async (_url: string, init: RequestInit) => { const input = JSON.parse(String(init.body)); return reply(input.request ?? READ[input.q] ?? { type: 'place', name: input.q }); });
const mounted: ReturnType<typeof mount>[] = [];
afterEach(async()=>{ for(const component of mounted.splice(0))await unmount(component); vi.unstubAllGlobals(); vi.unstubAllEnvs(); vi.useRealTimers(); document.body.replaceChildren(); });
async function setup(props: Record<string, unknown> = {}) {
    vi.useFakeTimers({toFake:['setTimeout','clearTimeout','setInterval','clearInterval','Date']});
    const target=document.createElement('div');document.body.append(target);
    // The getters of `props` stay getters, so a test can change a prop.
    const all=Object.defineProperties({context,onSearch:()=>{},onClear:()=>{},onLocation:async()=>undefined},Object.getOwnPropertyDescriptors(props));
    const component=mount(Query,{target,props:all});
    mounted.push(component);await tick();
    const input=target.querySelector('input')!;
    /** Types `text` and presses Enter. */
    const enter=async(text:string)=>{input.value=text;input.dispatchEvent(new Event('input',{bubbles:true}));await tick();
        input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));await vi.advanceTimersByTimeAsync(0);await tick();};
    const slot=(id:string)=>target.querySelector<HTMLButtonElement>(`[data-slot="${id}"]`)!;
    const words=(id:string)=>slot(id).querySelector('strong')!.textContent;
    const press=(text:string)=>[...target.querySelectorAll<HTMLButtonElement>('.search-panel button')].find(button=>button.textContent?.trim()===text)!.click();
    return {target,input,enter,slot,words,press,component};
}
const sent = (fetch: ReturnType<typeof vi.fn>) => fetch.mock.calls.map(([, init]) => JSON.parse(String((init as RequestInit).body)));
/** The search area of the stretch from km `from` to km `to`. */
const km = (from: number, to: number) => ({ along: { ref: 'km', from: { value: from, unit: 'km' }, to: { value: to, unit: 'km' } } });

describe('the planner search',()=>{
    it('fills the slots from what the query model read, and a slot that the answer leaves empty keeps its value', async () => {
        const fetch = server();
        vi.stubGlobal('fetch', fetch);
        const { input, enter, words } = await setup();
        await enter('drinking water in the middle of day 1');
        expect([words('find'), words('where'), words('when')]).toEqual(['Drinking water', 'Middle of Day 1', 'Any time']);
        expect(input.value).toBe('');
        await enter('supermarket');
        expect([words('find'), words('where')]).toEqual(['Supermarket', 'Middle of Day 1']);
        // The slots send the middle of day 1 as its stretch, and the area that the slot kept, so they search again with no
        // model after each answer.
        expect(sent(fetch).map(body => [body.q, body.request])).toEqual([
            ['drinking water in the middle of day 1', undefined],
            ['drinking water in the middle of day 1', { type: 'places', what: ['drinking_water'], where: km(1.2, 2.8) }],
            ['supermarket', undefined], ['supermarket', { type: 'places', what: ['supermarket'], where: km(1.2, 2.8) }],
        ]);
    });

    it('reads a sentence of only search areas and times from the word list, with no model', async () => {
        const fetch = server();
        vi.stubGlobal('fetch', fetch);
        const { enter, words } = await setup();
        await enter('drinking water in the middle of day 1');
        await enter('on day 2, open now');
        expect([words('where'), words('when')]).toEqual(['Middle of Day 2', 'Open now']);
        expect(sent(fetch).at(-1).request).toEqual({ type: 'places', what: ['drinking_water'], where: km(4.9, 6.1), open: { now: true } });
        expect(fetch).toHaveBeenCalledTimes(3);
    });

    it('keeps the words that the model ignored and finds one thing, the first of two kinds with no kind over both', async () => {
        const fetch = server();
        vi.stubGlobal('fetch', fetch);
        const { target, input, enter, words } = await setup();
        await enter('sunny cafe or bakery');
        expect(input.value).toBe('sunny');
        expect(words('find')).toBe('Cafe');
        expect(target.querySelector('[role="status"]')?.textContent).toBe('One thing at a time: bakery was left out.');
        expect(sent(fetch).at(-1).request).toEqual({ type: 'places', what: ['cafe'] });
    });

    it('searches with the request of a panel and no model, and turns "When" off for a fact of the route', async () => {
        const fetch = server();
        vi.stubGlobal('fetch', fetch);
        const { enter, slot, press } = await setup();
        await enter('supermarket open now');
        slot('find').click(); await tick();
        press('Food'); await vi.advanceTimersByTimeAsync(0); await tick();
        expect(sent(fetch).at(-1)).toMatchObject({ q: 'supermarket open now', request: { type: 'places', what: ['food'] } });
        press('Climbs'); await vi.advanceTimersByTimeAsync(0); await tick();
        expect(sent(fetch).at(-1).request).toEqual({ type: 'stretches', what: 'climb' });
        expect(slot('when').disabled).toBe(true);
    });

    it('sets "Where" on the profile of the route: a day, a part of it, a handle, and the places of "Find" along the route as dots', async () => {
        const along = (km: number) => ({ name: `${km} km`, kind: 'supermarket', lon: 7.8, lat: 48, distance: 0, position: { along: km, distance: 0 } });
        const fetch = vi.fn(async (_url: string, init: RequestInit) => {
            const input = JSON.parse(String(init.body)), request = input.request ?? READ[input.q];
            return { ok: true, json: async () => ({ type: 'places', request, results: input.limit === 100 ? [along(1), along(5)] : [] }) } as Response;
        });
        vi.stubGlobal('fetch', fetch);
        const { target, enter, slot, words, press } = await setup();
        await enter('supermarket');
        slot('where').click(); await vi.advanceTimersByTimeAsync(0); await tick();
        // The panel asks for the places along the whole route; with no stretch, each is in the area.
        expect(sent(fetch).at(-1)).toMatchObject({ limit: 100, request: { type: 'places', what: ['supermarket'], where: km(0, 7.5) } });
        const dots = () => [...target.querySelectorAll('.dot')].map(dot => dot.classList.contains('in'));
        expect(dots()).toEqual([true, true]);
        const search = async (button: string) => { press(button); await vi.advanceTimersByTimeAsync(0); await tick(); return sent(fetch).at(-1).request.where; };
        expect(await search('Day 2')).toEqual(km(4, 7));
        expect([words('where'), dots()]).toEqual(['Day 2', [false, true]]);
        expect(await search('Middle')).toEqual(km(4.9, 6.1));
        expect(words('where')).toBe('Middle of Day 2');
        // A second press removes the day and keeps the part, of the whole route.
        expect(await search('Day 2')).toEqual(km(2.2, 5.3));
        expect(words('where')).toBe('Middle of the route');
        // A handle searches when it rests.
        const end = target.querySelector<HTMLInputElement>('input[aria-label="End of the stretch"]')!;
        const searches = fetch.mock.calls.length;
        for (const value of ['4', '3']) { end.value = value; end.dispatchEvent(new Event('input', { bubbles: true })); await tick(); }
        expect(fetch).toHaveBeenCalledTimes(searches);
        await vi.advanceTimersByTimeAsync(250); await tick();
        expect(fetch).toHaveBeenCalledTimes(searches + 1);
        expect(sent(fetch).at(-1).request.where).toEqual(km(2.2, 3));
        expect([words('where'), dots()]).toEqual(['2.2–3 km', [false, false]]);
    });

    it('keeps a part on a day that the profile cannot show, and searches near the selected point and ahead of the rider', async () => {
        const fetch = server();
        vi.stubGlobal('fetch', fetch);
        const { enter, slot, words, press } = await setup({ selection: { anchor: [7.9, 48] } });
        await enter('water today');
        slot('where').click(); await tick();
        const search = async (button: string) => { press(button); await vi.advanceTimersByTimeAsync(0); await tick(); return sent(fetch).at(-1).request.where; };
        expect(await search('Start')).toEqual({ day: 'today', part: 'start' });
        expect(words('where')).toBe('Start of today');
        expect(await search('Selected point')).toEqual({ anchor: [7.9, 48] });
        expect(words('where')).toBe('Near the selected point');
        expect(await search('Nearest to me')).toEqual({ along: { ref: 'here', from: { value: 0, unit: 'km' }, to: { value: 20, unit: 'km' } } });
        expect(words('where')).toBe('Nearest to me');
    });

    it('says in the panel of "Where" that "Near me" has no location, and offers no part of an area off the route', async () => {
        const fetch = vi.fn(async (_url: string, init: RequestInit) => {
            const input = JSON.parse(String(init.body)), request = input.request ?? READ[input.q];
            return { ok: true, json: async () => ({ type: 'places', request, results: [], ...request.where?.scope === 'here' ? { code: 'location' } : {} }) } as Response;
        });
        vi.stubGlobal('fetch', fetch);
        const { target, enter, slot, press } = await setup({ onLocation: async () => 'denied' });
        await enter('supermarket');
        slot('where').click(); await tick();
        const part = target.querySelector<HTMLFieldSetElement>('.search-panel fieldset')!;
        expect([part.disabled, target.querySelector('.search-panel .note')]).toEqual([false, null]);
        press('Near me'); await vi.advanceTimersByTimeAsync(0); await tick();
        expect([part.disabled, target.querySelector('.search-panel .note')?.textContent]).toEqual([true, 'Allow location access to search near you.']);
    });

    it('keeps a change of the plan when it searches again with the position of the rider', async () => {
        const route: QueryRequest = { type: 'route', from: { here: true }, to: { name: 'Konstanz' } };
        const fetch = vi.fn(async (_url: string, init: RequestInit) => {
            const input = JSON.parse(String(init.body));
            if (input.request?.type !== 'route' && input.q !== 'route from my location to Konstanz') return reply(input.request ?? READ[input.q]);
            return { ok: true, json: async () => input.here ? { type: 'change', request: route, description: 'Route from Your location to Konstanz', changes: [] }
                : { type: 'unresolved', code: 'location', request: route, results: [] } } as Response;
        });
        vi.stubGlobal('fetch', fetch);
        const view = fromStore(writable(context)), revision = fromStore(writable(0));
        let state: SearchState = { loading: false, error: '', answer: null };
        const onLocation = vi.fn(async () => { view.current = { ...context, here: [7.85, 48] }; revision.current++; return undefined; });
        const { target, enter, words } = await setup({ get context() { return view.current; }, get revision() { return revision.current; }, onLocation,
            get searchState() { return state; }, set searchState(next: SearchState) { state = next; } });
        await enter('supermarket');
        await enter('route from my location to Konstanz'); await vi.advanceTimersByTimeAsync(250); await tick();
        expect(onLocation).toHaveBeenCalledOnce();
        expect(sent(fetch).at(-1)).toMatchObject({ here: [7.85, 48], request: route });
        expect(state).toMatchObject({ loading: false, answer: { type: 'change' } });
        // The slots keep the search before, and the empty field offers nothing over "Apply change".
        expect(words('find')).toBe('Supermarket');
        expect(target.querySelector('[role="listbox"]')).toBeNull();
        // The cross takes the change away, and "Undo" asks for it again.
        target.querySelector<HTMLButtonElement>('[aria-label="Clear the search"]')!.click(); await tick();
        expect([state.answer, words('find')]).toEqual([null, 'Choose']);
        target.querySelector<HTMLButtonElement>('.undo')!.click(); await vi.advanceTimersByTimeAsync(0); await tick();
        expect(sent(fetch).at(-1)).toMatchObject({ request: route });
        expect([state.answer?.type, words('find')]).toEqual(['change', 'Supermarket']);
    });

    it('searches a name in the map view, and keeps the notice of an answer without the model when the slots search again', async () => {
        const notice = 'The query model is unavailable. Showing ordinary place search.';
        const answers: Record<string, object> = {
            'pharmacy in Teningen': { request: { type: 'places', what: ['pharmacy'], where: { near: [{ name: 'Teningen' }] } } },
            'cafes': { request: { type: 'places', what: ['cafe'] }, notice, canRetry: true },
            'Freiburg Hauptbahnhof': { request: { type: 'place', name: 'Freiburg Hauptbahnhof' }, notice, canRetry: true },
        };
        const fetch = vi.fn(async (_url: string, init: RequestInit) => {
            const input = JSON.parse(String(init.body));
            return { ok: true, json: async () => ({ type: 'places', results: [], ...input.request ? { request: input.request } : answers[input.q] }) } as Response;
        });
        vi.stubGlobal('fetch', fetch);
        let state: SearchState = { loading: false, error: '', answer: null };
        const { enter, words } = await setup({ get searchState() { return state; }, set searchState(next: SearchState) { state = next; } });
        await enter('pharmacy in Teningen');
        await enter('cafes');
        expect(sent(fetch).at(-1).request).toEqual({ type: 'places', what: ['cafe'], where: { near: [{ name: 'Teningen' }] } });
        expect(state.answer).toMatchObject({ notice, canRetry: true });
        await enter('Freiburg Hauptbahnhof');
        expect(fetch).toHaveBeenCalledTimes(4);
        expect(words('where')).toBe('In this map view');
        expect(state.answer).toMatchObject({ request: { type: 'place', name: 'Freiburg Hauptbahnhof' }, notice, canRetry: true });
    });

    it('discards stale responses and keeps a cleared search empty',async()=>{
        const pending: ((response:Response)=>void)[]=[];
        const fetch=vi.fn(()=>new Promise<Response>(resolve=>pending.push(resolve)));
        vi.stubGlobal('fetch',fetch);
        const {target,enter,words}=await setup();
        await enter('first sentence');await enter('second sentence');
        pending[1](reply({type:'places',what:['hotel']}));await vi.advanceTimersByTimeAsync(0);await tick();
        pending[0](reply({type:'places',what:['campsite']}));await vi.advanceTimersByTimeAsync(0);await tick();
        expect(words('find')).toBe('Hotel');
        await enter('third sentence');
        target.querySelector<HTMLButtonElement>('[aria-label="Clear the search"]')!.click();await tick();
        pending[2](reply({type:'places',what:['campsite']}));await vi.advanceTimersByTimeAsync(0);await tick();
        expect(words('find')).toBe('Choose');
    });

    it('asks for the position of the rider once and searches again when the context has it',async()=>{
        const near:QueryRequest={type:'places',what:['bakery'],where:{scope:'here'}};
        const fetch=vi.fn(async(_url:string,init:RequestInit)=>({ok:true,json:async()=>JSON.parse(String(init.body)).here
            ?{type:'places',request:near,results:[]}:{type:'unresolved',code:'location',request:near,results:[]}}) as Response);
        vi.stubGlobal('fetch',fetch);
        const view=fromStore(writable(context)), revision=fromStore(writable(0));
        const onLocation=vi.fn(async()=>{ view.current={...context,here:[7.85,48]}; revision.current++; return undefined; });
        const {enter}=await setup({get context(){return view.current;},get revision(){return revision.current;},onLocation});
        await enter('bakery near me');await vi.advanceTimersByTimeAsync(250);await tick();
        expect(onLocation).toHaveBeenCalledOnce();
        expect(fetch).toHaveBeenCalledTimes(2);
        expect(sent(fetch).at(-1)).toMatchObject({here:[7.85,48],request:near});
    });

    it('retries interpretation after a temporary model failure instead of keeping the fallback',async()=>{
        const fetch=vi.fn(async()=>reply({type:'place',name:'hotels near day two'}));
        vi.stubGlobal('fetch',fetch);
        const {enter,words,component}=await setup();await enter('hotels near day two');
        expect(words('find')).toBe('hotels near day two');
        component.retry();await vi.advanceTimersByTimeAsync(0);await tick();
        expect(sent(fetch).at(-1).q).toBe('hotels near day two');
        expect(sent(fetch).at(-1).request).toBeUndefined();
    });

    it('keeps searches stable through map movement until the rider requests the new view',async()=>{
        const view=fromStore(writable(context)), viewRevision=fromStore(writable(0));
        const fetch=vi.fn(async()=>({ok:true,json:async()=>({type:'places',request:{type:'places',what:['pharmacy']},results:[{lon:7.81,lat:48.12},{lon:7.85,lat:48.14}]})}));
        vi.stubGlobal('fetch',fetch);
        const {target,enter,component}=await setup({get context(){return view.current;},get viewRevision(){return viewRevision.current;}});
        await enter('pharmacies');
        const fitted: SearchContext={...context,view:[7.8,48.1,7.9,48.2]};
        view.current=fitted;await tick();await vi.advanceTimersByTimeAsync(250);
        expect(fetch).toHaveBeenCalledTimes(1);
        fetch.mockRejectedValueOnce(new TypeError('Offline'));
        component.more();await vi.advanceTimersByTimeAsync(0);await tick();
        expect(sent(fetch).at(-1)).toMatchObject({view:context.view,limit:40});
        component.retry();await vi.advanceTimersByTimeAsync(0);await tick();
        expect(sent(fetch).at(-1).view).toEqual(context.view);
        expect(fetch).toHaveBeenCalledTimes(3);
        const panned: SearchContext={...context,view:[7.9,48.1,8,48.2]};
        view.current=panned;viewRevision.current=1;await tick();await vi.advanceTimersByTimeAsync(250);
        expect(fetch).toHaveBeenCalledTimes(3);
        target.querySelector<HTMLButtonElement>('.search-view')!.click();
        await vi.advanceTimersByTimeAsync(0);await tick();
        expect(fetch).toHaveBeenCalledTimes(4);
        expect(target.querySelector('.search-view')).toBeNull();
        expect(sent(fetch).at(-1).view).toEqual(panned.view);
        view.current=fitted;await tick();
        component.more();await vi.advanceTimersByTimeAsync(0);await tick();
        expect(sent(fetch).at(-1)).toMatchObject({view:panned.view,limit:60});
    });
});
