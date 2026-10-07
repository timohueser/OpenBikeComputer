export function websiteLink(value?: string): string | undefined {
    const first = value?.split(';').map(v => v.trim()).find(Boolean);
    if (!first || (/^[a-z][a-z\d+.-]*:/i.test(first) && !/^https?:/i.test(first))) return undefined;
    try {
        const url = new URL(first.startsWith('//') ? `https:${first}` : /^https?:/i.test(first) ? first : `https://${first}`);
        return ['http:', 'https:'].includes(url.protocol) && url.hostname ? url.href : undefined;
    } catch { return undefined; }
}

export const phoneNumbers = (value?: string): string[] => value?.split(';').map(v => v.trim()).filter(Boolean) ?? [];
export function phoneLink(value: string): string | undefined {
    const number = value.replace(/[\s()./\-]/g, '');
    return /^\+?\d+$/.test(number) ? `tel:${number}` : undefined;
}
