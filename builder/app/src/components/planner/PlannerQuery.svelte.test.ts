// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { writable, fromStore } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';
import Query from './PlannerQuery.svelte';
import type { SearchContext, QueryRequest } from '../../lib/planner/search/types';

const context: SearchContext = {view:[7.8,47.9,8,48.1],plan:{coordinates:[[7.8,48],[7.9,48]],days:[{number:1,from:0,to:7,rest:false}],points:[]}};
const reply = (request: QueryRequest) => ({ok:true,json:async()=>({type:'places',request,results:[]})}) as Response;
const mounted: ReturnType<typeof mount>[] = [];
afterEach(async()=>{ for(const component of mounted.splice(0))await unmount(component); vi.unstubAllGlobals(); vi.useRealTimers(); document.body.replaceChildren(); });
async function setup(extra: Record<string, unknown> = {}) {
    vi.useFakeTimers({toFake:['setTimeout','clearTimeout','setInterval','clearInterval']});
    const target=document.createElement('div');document.body.append(target);
    const component=mount(Query,{target,props:{context,onSearch:()=>{},onClear:()=>{},onLocation:()=>{},onSample:()=>{},onDate:()=>{},onPointing:()=>{},...extra}});
    mounted.push(component);await tick();
    const input=target.querySelector('input')!;
    const type=async(text:string)=>{input.value=text;input.dispatchEvent(new Event('input',{bubbles:true}));await tick();await vi.advanceTimersByTimeAsync(351);};
    return {target,type,component};
}
describe('planner query requests',()=>{
    it('discards stale responses and keeps a cleared search empty',async()=>{
        const pending: ((response:Response)=>void)[]=[];
        const fetch=vi.fn(()=>new Promise<Response>(resolve=>pending.push(resolve)));
        vi.stubGlobal('fetch',fetch);
        const {target,type}=await setup();
        await type('first sentence');await type('second sentence');
        pending[1](reply({type:'places',what:['hotel']}));await vi.advanceTimersByTimeAsync(0);await tick();
        pending[0](reply({type:'places',what:['campsite']}));await vi.advanceTimersByTimeAsync(0);await tick();
        expect(target.querySelector('.meaning')?.textContent).toContain('hotel');
        expect(target.querySelector('.meaning')?.textContent).not.toContain('campsite');
        await type('third sentence');await type('');
        pending[2](reply({type:'places',what:['campsite']}));await vi.advanceTimersByTimeAsync(0);await tick();
        expect(target.querySelector('.meaning')).toBeNull();
    });
    it('retries interpretation after a temporary model failure instead of keeping the fallback',async()=>{
        const fetch=vi.fn(async()=>reply({type:'place',name:'hotels near day two'}));
        vi.stubGlobal('fetch',fetch);
        const {type,component}=await setup();await type('hotels near day two');await tick();
        component.retry();await vi.advanceTimersByTimeAsync(0);await tick();
        const options=(fetch.mock.lastCall as unknown as [string,RequestInit])[1];
        expect(JSON.parse(String(options.body)).request).toBeUndefined();
    });
    it('sends chip edits as typed requests and keeps the rider’s sentence intact',async()=>{
        const fetch=vi.fn(async(_url:string,init:RequestInit)=>reply(JSON.parse(String(init.body)).request??{type:'places',what:['hotel']}));
        vi.stubGlobal('fetch',fetch);
        const {target,type}=await setup();await type('hotels nearby');await tick();
        (target.querySelector('.chip') as HTMLButtonElement).click();await tick();
        const button=(text:string)=>[...target.querySelectorAll('.choices button')].find(b=>b.textContent?.trim()===text)!;
        button('hotel').click();await tick();
        expect((target.querySelector('.apply') as HTMLButtonElement).disabled).toBe(true);
        button('campsite').click();await tick();
        expect(fetch).toHaveBeenCalledTimes(1);
        (target.querySelector('.apply') as HTMLButtonElement).click();
        await vi.advanceTimersByTimeAsync(0);await tick();
        const input=JSON.parse(String(fetch.mock.lastCall![1].body));
        expect(input.request.what).toEqual(['campsite']);expect(input.q).toBe('hotels nearby');
        expect(target.textContent).toContain('Edited request');
        expect(target.querySelector('.picker')).toBeNull();
    });
    it('keeps one editor open and discards an unapplied area edit on Escape', async()=>{
        const fetch=vi.fn(async()=>reply({type:'places',what:['pharmacy'],where:{near:[{name:'Teningen'}]}}));
        vi.stubGlobal('fetch',fetch);
        const {target,type}=await setup();await type('pharmacies in Teningen');await tick();
        const chips=target.querySelectorAll<HTMLButtonElement>('.chip');
        chips[0].click();await tick();chips[1].click();await tick();
        expect(target.querySelectorAll('.picker')).toHaveLength(1);
        const input=target.querySelector<HTMLInputElement>('.picker input')!;
        input.value='Bad ';input.dispatchEvent(new Event('input',{bubbles:true}));await tick();
        expect(input.value).toBe('Bad ');
        input.value='Bad Krozingen';input.dispatchEvent(new Event('input',{bubbles:true}));await tick();
        input.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));await tick();
        expect(fetch).toHaveBeenCalledTimes(1);
        expect(chips[1].textContent).toContain('Teningen');
        expect(document.activeElement).toBe(chips[1]);
        expect(target.querySelector('.picker')).toBeNull();
    });
    it('frames fresh results, keeps fitted bounds, and lets the rider pan without framing again',async()=>{
        const state=writable({context,viewRevision:0});
        const current=fromStore(state);
        const onResults=vi.fn();
        const fetch=vi.fn(async()=>({ok:true,json:async()=>({type:'places',request:{type:'places',what:['pharmacy']},results:[{lon:7.81,lat:48.12},{lon:7.85,lat:48.14}]})}));
        vi.stubGlobal('fetch',fetch);
        vi.useFakeTimers({toFake:['setTimeout','clearTimeout','setInterval','clearInterval']});
        const target=document.createElement('div');document.body.append(target);
        const component=mount(Query,{target,props:{get context(){return current.current.context;},get viewRevision(){return current.current.viewRevision;},onResults,onSearch:()=>{},onClear:()=>{},onLocation:()=>{},onSample:()=>{},onDate:()=>{},onPointing:()=>{}}});
        mounted.push(component);await tick();
        const input=target.querySelector('input')!;input.value='pharmacies';input.dispatchEvent(new Event('input',{bubbles:true}));await tick();await vi.advanceTimersByTimeAsync(351);
        expect(onResults).toHaveBeenCalledExactlyOnceWith([[7.81,48.12],[7.85,48.14]]);
        const fitted: SearchContext={...context,view:[7.8,48.1,7.9,48.2]};
        state.set({context:fitted,viewRevision:0});await tick();await vi.advanceTimersByTimeAsync(250);
        expect(fetch).toHaveBeenCalledTimes(1);
        const panned: SearchContext={...context,view:[7.9,48.1,8,48.2]};
        state.set({context:panned,viewRevision:1});await tick();await vi.advanceTimersByTimeAsync(250);
        expect(fetch).toHaveBeenCalledTimes(2);
        expect(onResults).toHaveBeenCalledTimes(1);
        const body=JSON.parse(String((fetch.mock.lastCall as unknown as [string,RequestInit])[1].body));
        expect(body.view).toEqual(panned.view);
    });
    it('preserves pending result framing when initial map bounds interrupt the first request',async()=>{
        const state=writable({context,viewRevision:0});const current=fromStore(state);
        const onResults=vi.fn();
        const fetch=vi.fn().mockImplementationOnce(()=>new Promise(()=>{})).mockResolvedValue({ok:true,json:async()=>({type:'places',request:{type:'places',what:['pharmacy']},results:[{lon:7.81,lat:48.12}]})});
        vi.stubGlobal('fetch',fetch);vi.useFakeTimers({toFake:['setTimeout','clearTimeout','setInterval','clearInterval']});
        const target=document.createElement('div');document.body.append(target);
        const component=mount(Query,{target,props:{get context(){return current.current.context;},get viewRevision(){return current.current.viewRevision;},onResults,onSearch:()=>{},onClear:()=>{},onLocation:()=>{},onSample:()=>{},onDate:()=>{},onPointing:()=>{}}});
        mounted.push(component);await tick();
        const input=target.querySelector('input')!;input.value='pharmacies';input.dispatchEvent(new Event('input',{bubbles:true}));await tick();await vi.advanceTimersByTimeAsync(351);
        state.set({context:{...context,view:[7,47,9,49]},viewRevision:1});await tick();await vi.advanceTimersByTimeAsync(250);
        expect(fetch).toHaveBeenCalledTimes(2);
        expect(onResults).toHaveBeenCalledExactlyOnceWith([[7.81,48.12]]);
    });
    it('refreshes current hours without a new search presentation or another map fit', async()=>{
        const onResults=vi.fn(), onSearch=vi.fn();
        const fetch=vi.fn(async()=>({ok:true,json:async()=>({type:'places',request:{type:'places',what:['pharmacy']},results:[{lon:7.81,lat:48.12,opening_hours:'24/7'}]})}));
        vi.stubGlobal('fetch',fetch);
        const currentContext={...context};
        const {type}=await setup({onResults,onSearch,context:currentContext});await type('pharmacies');await tick();
        currentContext.view=[7,47,9,49];
        const searches=onSearch.mock.calls.length;
        await vi.advanceTimersByTimeAsync(60_000);await tick();
        expect(fetch).toHaveBeenCalledTimes(2);
        expect(onSearch).toHaveBeenCalledTimes(searches);
        expect(JSON.parse(String((fetch.mock.lastCall as unknown as [string,RequestInit])[1].body)).view).toEqual(context.view);
        expect(onResults).toHaveBeenCalledTimes(1);
    });
});
