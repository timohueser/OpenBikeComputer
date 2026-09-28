import { describe, expect, it } from 'vitest';
import { parsePlannerQuery } from './query';

describe('query grammar', () => {
    it('knows every place category by its label, plural and kind names', () => {
        const heard = (text: string) => parsePlannerQuery(text);
        expect(heard('pharmacies end of day 2')).toMatchObject({ value: { category: 'pharmacy', day: 2 }, unsupported: '' });
        expect(heard('bakeries within 5 km').value).toMatchObject({ category: 'shop', within: 5 });
        expect(heard('bike shops').value.category).toBe('bike');
        expect(heard('find me some water').value.category).toBe('water');
        expect(heard('peaks').value.category).toBe('peak');
        for (const word of ['supermarkets', 'campsites', 'restaurants', 'stations', 'shelters', 'toilets', 'viewpoints', 'hotels']) {
            expect(heard(word).unsupported).toBe('');
        }
    });

    it('names only the unknown word', () => {
        expect(parsePlannerQuery('hotels with a sauna day 2').unsupported).toBe('sauna');
    });
});
