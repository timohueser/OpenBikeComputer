import { categoryIds, placeCategories, type PlaceCategory } from './poi-kinds';

export type PlannerQueryValue = {
    text: string;
    category: 'all' | PlaceCategory;
    day: number | null;
    within: number | null;
};

const extraTerms: Partial<Record<PlaceCategory, string[]>> = {
    hotel: ['room', 'inn', 'sleep'],
    camp: ['camp', 'camping'],
    water: ['tap'],
    shop: ['shop', 'grocery', 'groceries'],
    food: ['eat', 'eating', 'cafe'],
    bike: ['bike shop', 'bike', 'repair'],
    peak: ['pass', 'summit'],
};
const filler = /(?<![\p{L}])(?:find|show|me|some|a|an|the|all|places?|along|route|trip|near|at|for|on|please|of|to|in|by|with|my|any|where|can|i|is|are|there|spots?|stops?|options|drinking)(?![\p{L}])/gu;
const dayPattern = /\b(?:end\s+of\s+)?(?:day|night)\s*(\d+)\b/;
const withinPattern = /\bwithin\s+(\d+(?:[.,]\d+)?)\s*(?:km|kilometres?|kilometers?)\b/;

function forms(word: string): string[] {
    const lower = word.toLowerCase();
    return [lower, `${lower}s`, lower.replace(/y$/, 'ies'), lower.replace(/([sx])$/, '$1es')];
}

// Every category label, plural and kind name in its forms; longest first, so "bike shops" wins over "shops".
const terms = categoryIds
    .flatMap(category => {
        const info = placeCategories[category];
        return [info.label, info.plural, ...Object.values(info.kinds), ...(extraTerms[category] ?? [])]
            .flatMap(forms)
            .map(term => ({ category, pattern: new RegExp(`(?<![\\p{L}])${term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?![\\p{L}])`, 'u'), length: term.length }));
    })
    .sort((a, b) => b.length - a.length);

/** The parser knows place categories, a day and a radius; `unsupported` is what is left over. */
export function parsePlannerQuery(text: string): { value: PlannerQueryValue; unsupported: string } {
    let rest = text.toLowerCase().trim();
    const day = rest.match(dayPattern);
    const within = rest.match(withinPattern);
    rest = rest.replace(new RegExp(dayPattern, 'g'), ' ').replace(new RegExp(withinPattern, 'g'), ' ');
    let category: PlannerQueryValue['category'] = 'all';
    const term = terms.find(candidate => candidate.pattern.test(rest));
    if (term) {
        category = term.category;
        rest = rest.replace(term.pattern, ' ');
    }
    const unsupported = rest.replace(filler, ' ').replace(/[.,!?]/g, '').replace(/\s+/g, ' ').trim();
    return { value: { text, category, day: day ? Number(day[1]) : null, within: within ? Number(within[1].replace(',', '.')) : null }, unsupported };
}
