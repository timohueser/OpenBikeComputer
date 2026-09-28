// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import Query from './PlannerQuery.svelte';
import type { SearchContext, QueryRequest } from '../../lib/planner/search/types';

const context: SearchContext = {view:[7.8,47.9,8,48.1],plan:{coordinates:[[7.8,48],[7.9,48]],days:[{number:1,from:0,to:7,rest:false}],points:[]}};
const reply = (request: QueryRequest) => ({ok:true,json:async()=>({type:'places',request,results:[]})}) as Response;
const mounted: ReturnType<typeof mount>[] = [];
afterEach(async()=>{ for(const component of mounted.splice(0))await unmount(component); vi.unstubAllGlobals(); vi.useRealTimers(); document.body.replaceChildren(); });
async function setup() {
    vi.useFakeTimers({toFake:['setTimeout','clearTimeout']});
    const target=document.createElement('div');document.body.append(target);
    const component=mount(Query,{target,props:{context,onSearch:()=>{},onClear:()=>{},onLocation:()=>{},onSample:()=>{},onDate:()=>{},onPointing:()=>{}}});
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
        const select=target.querySelector('select')!;select.value='campsite';select.dispatchEvent(new Event('change',{bubbles:true}));
        await vi.advanceTimersByTimeAsync(0);await tick();
        const input=JSON.parse(String(fetch.mock.lastCall![1].body));
        expect(input.request.what).toEqual(['campsite']);expect(input.q).toBe('hotels nearby');
        expect(target.textContent).toContain('Edited request');
    });
});
