export type PlannerQueryValue = {
    text: string;
    category: 'all' | 'hotel' | 'camp' | 'water';
    day: number | null;
    within: number | null;
};

/** The study recognises place categories and explicit day/radius constraints only. */
export function parsePlannerQuery(text: string): { value: PlannerQueryValue; unsupported: string } {
    const lower = text.toLowerCase().trim();
    const category = /\bhotels?\b/.test(lower) ? 'hotel'
        : /\b(?:camps?|campsites?|camping)\b/.test(lower) ? 'camp'
        : /\bwater\b/.test(lower) ? 'water' : 'all';
    const day = lower.match(/\b(?:end\s+of\s+)?(?:day|night)\s*(\d+)\b/);
    const within = lower.match(/\bwithin\s+(\d+(?:[.,]\d+)?)\s*(?:km|kilometres?|kilometers?)\b/);
    const unsupported = lower
        .replace(/\b(?:end\s+of\s+)?(?:day|night)\s*\d+\b/g, '')
        .replace(/\bwithin\s+\d+(?:[.,]\d+)?\s*(?:km|kilometres?|kilometers?)\b/g, '')
        .replace(/\b(?:find|show|me|some|a|the|all|places?|hotels?|camps?|campsites?|camping|drinking|water|along|route|trip|near|at|for|on|please)\b/g, '')
        .replace(/[.,!?]/g, '').trim();
    return { value: { text, category, day: day ? Number(day[1]) : null,
        within: within ? Number(within[1].replace(',', '.')) : null }, unsupported };
}
