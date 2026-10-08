export interface ContentAttribution { source_url: string; revision: string; license_url: string }
export interface ArticleVariant {
    language: string;
    text_pages: string[];
    attribution: ContentAttribution;
}
export interface PlaceContent {
    default_language: string;
    variants: ArticleVariant[];
    photo?: ContentAttribution & { online_url: string | null;
        file_identity?: { filename: string; page_id: number } | null;
        page_revision?: number | null; file_revision?: { timestamp: string; sha1: string } | null;
        credit: [string, string, string, string] };
}

export function articleVariant(content: PlaceContent, language = 'en'): ArticleVariant | undefined {
    return content.variants.find(variant => variant.language === language)
        ?? content.variants.find(variant => variant.language === content.default_language);
}

/** One current metadata request per opened photo; a changed source keeps only its source link. */
export async function onlinePhoto(photo: NonNullable<PlaceContent['photo']>, signal: AbortSignal): Promise<string | undefined> {
    if (!photo.online_url || !photo.file_identity || !photo.page_revision || !photo.file_revision) return;
    const url = new URL('https://commons.wikimedia.org/w/api.php');
    url.search = new URLSearchParams({ action: 'query', format: 'json', formatversion: '2', origin: '*',
        prop: 'info|imageinfo', pageids: String(photo.file_identity.page_id), iiprop: 'timestamp|sha1|url',
        iiurlwidth: '500', iilimit: '1', maxage: '0', smaxage: '0', maxlag: '5' }).toString();
    const response = await fetch(url, { signal: AbortSignal.any([signal, AbortSignal.timeout(10_000)]), cache: 'no-store',
        headers: { 'Api-User-Agent': 'OpenBikeComputer planner (https://openbikecomputer.com)' } });
    if (!response.ok) return;
    const result = await response.json();
    const page = result.query?.pages?.[0], image = page?.imageinfo?.[0];
    const filename = photo.file_identity.filename.replace(/^File:/, '').replaceAll('_', ' ');
    if (result.error || page?.pageid !== photo.file_identity.page_id || page?.lastrevid !== photo.page_revision
        || page?.title !== `File:${filename}` || image?.timestamp !== photo.file_revision.timestamp
        || image?.sha1 !== photo.file_revision.sha1 || image?.thumburl !== photo.online_url) return;
    const thumbnail = new URL(image.thumburl);
    if (thumbnail.protocol !== 'https:' || thumbnail.hostname !== 'upload.wikimedia.org' || !thumbnail.pathname.includes('/thumb/')) return;
    return thumbnail.href;
}
