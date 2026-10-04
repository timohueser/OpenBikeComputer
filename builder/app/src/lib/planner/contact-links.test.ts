import { describe, expect, it, vi } from 'vitest';
import { phoneLink, phoneNumbers, websiteLink } from './contact-links';
import { asPlace } from './search/presentation';
import { placeDetails } from './search/types';

describe('place contacts', () => {
    it('opens web addresses and rejects executable or non-web schemes', () => {
        expect(websiteLink('camp.example/path')).toBe('https://camp.example/path');
        expect(websiteLink('//camp.example')).toBe('https://camp.example/');
        expect(websiteLink('http://camp.example')).toBe('http://camp.example/');
        for (const value of ['javascript:alert(1)','data:text/html,test','file:///tmp/place','https://',undefined]) expect(websiteLink(value)).toBeUndefined();
    });
    it('keeps separate phone numbers and leaves ambiguous text undialled', () => {
        expect(phoneNumbers('+49 (123) 45-67; +33 123')).toEqual(['+49 (123) 45-67','+33 123']);
        expect(phoneLink('+49 (123) 45-67')).toBe('tel:+491234567');
        expect(phoneLink('call reception')).toBeUndefined();
    });
    it('retains the source description and contact fields in search results', () => {
        const place = asPlace({source:'n1',name:'Camp',kind:'campsite',city:'',region:'',lon:8,lat:48,distance:0,precision:'place',
            website:'camp.example',phone:'+49 123',description:'Small tents only.\nAsk at reception.'});
        expect(place.website).toBe('camp.example');
        expect(place.phone).toBe('+49 123');
        expect(place.description).toBe('Small tents only.\nAsk at reception.');
    });
    it('only accepts details for the selected OSM identity', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ok:true,json:async () => ({results:[{source:'n2'}]})}));
        try {
            expect(await placeDetails('n1',[8,48],'test',new AbortController().signal)).toBeUndefined();
        } finally { vi.unstubAllGlobals(); }
    });
});
