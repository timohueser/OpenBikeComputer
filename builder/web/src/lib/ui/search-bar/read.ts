/** The reader of the text field of the search bar: it marks the phrases of a text that a page's table knows, finds the
 * words that the rider still types, and suggests values for them. It knows no page and sends no request. */

/** A value that words give: its words in a row of the list, the slot that it goes to, and its icon. */
export interface Value { label: string; slot: string; icon: string }

/** What some words mean: a value, or a note that says why they stay in the field. */
export type Meaning<V extends Value> = V | string;

/** The words of a page. Words are lower case with no accents. */
export interface Table<V extends Value> {
    /** `value` is what the words mean alone, and `not` what they mean after a word of `not`, such as "no". A phrase
     * with no `value` gets no mark alone. */
    phrases: { words: string; value?: Meaning<V>; not?: Meaning<V> }[];
    /** Quantities, such as "under 12 km": an expression that starts with `^`, run on the text from the start of a word.
     * Its first value is what the words mean, and each value is a suggestion. */
    patterns: { match: RegExp; values: (match: RegExpExecArray) => V[] }[];
    /** Words that start a quantity, such as "at least", and the usual values for them before a number. */
    starts: { words: string[]; values: V[] }[];
    /** The units that can follow a number. */
    units: string[];
    /** Words that carry no value, such as "with". */
    glue: string[];
    /** Words that give the `not` of the next phrase. */
    not: string[];
}

/** A part of a text, from `from` to `to`. An unknown part stays in the field at Enter. */
export type Part<V extends Value> = { from: number; to: number } & ({ kind: 'value'; value: V } | { kind: 'glue' } | { kind: 'unknown'; note?: string });

type Token = { word: string; from: number; to: number; stop: boolean };
type Phrase<V extends Value> = { words: string[]; text: string; value?: Meaning<V>; not?: Meaning<V> };
interface Compiled<V extends Value> { phrases: Phrase<V>[]; texts: Set<string>; glue: Set<string>; not: Set<string>; leads: string[]; pending: RegExp }

/** Lower case with no accents. Each character keeps its length, so a position in the folded text is one in the text. */
export const fold = (text: string) => text.replace(/./gsu, char => {
    const plain = char.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
    return plain.length === char.length ? plain : char;
});

/** A number with what follows it, a word, or a stop. A dash between letters splits words: "snow-free" is "snow free". */
const TOKEN = /\d+(?:[.,]\d+)?[^\s,;.!?]*|[^\s,;.!?\-–]+|[,;.!?]/g;
function tokens(folded: string): Token[] {
    return [...folded.matchAll(TOKEN)].map(match => ({ word: match[0], from: match.index, to: match.index + match[0].length, stop: /^[,;.!?]$/.test(match[0]) }));
}

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const longestFirst = (list: string[]) => [...new Set(list)].sort((a, b) => b.length - a.length).map(escape).join('|');

const made = new WeakMap<Table<Value>, Compiled<Value>>();
function compiled<V extends Value>(table: Table<V>): Compiled<V> {
    let done = made.get(table) as Compiled<V> | undefined;
    if (done) return done;
    const phrases = table.phrases.map(phrase => ({ ...phrase, words: phrase.words.split(' '), text: phrase.words }));
    // A number at the end with its range and the letters of a unit that the rider may still type.
    const units = longestFirst(table.units.flatMap(unit => [...unit].map((_, i) => unit.slice(0, i + 1))));
    const number = '\\d+(?:[.,]\\d*)?';
    const pending = new RegExp(`(?<!\\S)${number}(?:\\s*(?:-|–|to|and)\\s*(?:${number})?)?(?:\\s*(?:${units}))?$`);
    done = { phrases, texts: new Set(phrases.map(phrase => phrase.text)), glue: new Set(table.glue), not: new Set(table.not), leads: table.starts.flatMap(start => start.words), pending };
    made.set(table, done);
    return done;
}

/** A word of the text is the word of a phrase; the last word of a phrase can have a plural "s" or "es". */
const same = (word: string, of: string, last: boolean) => word === of || (last && (word === `${of}s` || word === `${of}es`));

/** The longest phrase or quantity that starts at token `i`: where it ends, the next token, and what it means. */
function longest<V extends Value>(table: Table<V>, folded: string, list: Token[], i: number) {
    let best: { to: number; next: number; value?: Meaning<V>; not?: Meaning<V> } | undefined;
    for (const phrase of compiled(table).phrases) {
        const count = phrase.words.length;
        if (i + count > list.length || !phrase.words.every((word, j) => !list[i + j].stop && same(list[i + j].word, word, j === count - 1))) continue;
        const to = list[i + count - 1].to;
        if (!best || to > best.to) best = { to, next: i + count, value: phrase.value, not: phrase.not };
    }
    for (const pattern of table.patterns) {
        const match = pattern.match.exec(folded.slice(list[i].from)), value = match && pattern.values(match)[0];
        if (!value) continue;
        const to = list[i].from + match[0].trimEnd().length, end = list.findIndex(token => token.to === to);
        if (end >= i && (!best || to > best.to)) best = { to, next: end + 1, value };
    }
    return best;
}

function part<V extends Value>(from: number, to: number, meaning: Meaning<V> | undefined): Part<V> {
    if (meaning === undefined) return { from, to, kind: 'unknown' };
    return typeof meaning === 'string' ? { from, to, kind: 'unknown', note: meaning } : { from, to, kind: 'value', value: meaning };
}

/**
 * The parts of a text: the values, the glue words and the unknown words. The longest phrase wins. A word of `not` takes
 * the next phrase after glue words: its `not`, else the words stay unknown. The words of `starts` alone are glue. Case
 * and accents do not matter.
 */
export function scan<V extends Value>(text: string, table: Table<V>): Part<V>[] {
    const { glue, not, leads } = compiled(table), folded = fold(text), list = tokens(folded), parts: Part<V>[] = [];
    for (let i = 0; i < list.length;) {
        const token = list[i];
        if (token.stop) {
            parts.push({ from: token.from, to: token.to, kind: 'glue' });
            i++;
            continue;
        }
        const hit = longest(table, folded, list, i);
        if (hit) {
            parts.push(part(token.from, hit.to, hit.value));
            i = hit.next;
            continue;
        }
        if (not.has(token.word)) {
            let next = i + 1;
            while (next < list.length && !list[next].stop && glue.has(list[next].word)) next++;
            const after = next < list.length && !list[next].stop ? longest(table, folded, list, next) : undefined;
            if (after) {
                parts.push(part(token.from, after.to, after.not));
                i = after.next;
                continue;
            }
        }
        // A word that starts a quantity carries no value without its number.
        const lead = leads.map(words => words.split(' ')).find(words => words.every((word, j) => list[i + j]?.word === word));
        const end = lead ? i + lead.length - 1 : i;
        parts.push({ from: token.from, to: list[end].to, kind: lead || glue.has(token.word) ? 'glue' : 'unknown' });
        i = end + 1;
    }
    return parts;
}

/**
 * Where the words start that the rider still types: the last word, or more words when they begin a longer phrase or a
 * quantity, with a word of `not` before them. After a space, a whole phrase and a number with its unit are done.
 * The length of the text when no word is in progress.
 */
export function typing<V extends Value>(text: string, table: Table<V>): number {
    const { phrases, texts, leads, not, pending } = compiled(table);
    const trimmed = text.trimEnd(), spaced = trimmed.length < text.length;
    if (!trimmed || /[,;.!?]$/.test(trimmed)) return text.length;
    const folded = fold(trimmed), all = tokens(folded), words = all.slice(all.map(token => token.stop).lastIndexOf(true) + 1);
    if (!words.length) return text.length;
    const tails = words.slice(-5).map((token, i, last) => ({ at: token.from, count: last.length - i, tail: last.slice(i).map(one => one.word).join(' ') }));
    const gap = spaced ? ' ' : '';
    const begins = (tail: string) => phrases.some(phrase => phrase.text.startsWith(tail + gap));
    const leadsWith = (tail: string) => leads.some(lead => (lead + gap).startsWith(tail + gap));
    const quantity = pending.exec(folded);
    let start: number;
    if (spaced) {
        if (tails.some(({ tail }) => texts.has(tail))) return text.length;
        if (quantity) start = /\p{L}$/u.test(quantity[0]) && !/\b(?:to|and)$/.test(quantity[0]) ? text.length : quantity.index;
        else start = tails.find(({ tail }) => begins(tail) || leadsWith(tail))?.at ?? (not.has(words.at(-1)!.word) ? words.at(-1)!.from : text.length);
    } else {
        start = Math.min(words.at(-1)!.from, quantity?.index ?? Infinity);
        const found = tails.find(({ count, tail }) => begins(tail) || (count > 1 && leadsWith(tail)));
        if (found) start = Math.min(start, found.at);
    }
    if (start >= text.length) return text.length;
    // The words of a quantity or a "no" before the words in progress are part of them.
    const before = words.filter(token => token.to <= start);
    const lead = [3, 2, 1].find(count => count <= before.length && leads.includes(before.slice(-count).map(token => token.word).join(' ')));
    if (lead) start = before[before.length - lead].from;
    const previous = words.filter(token => token.to <= start).at(-1);
    return previous && not.has(previous.word) ? previous.from : start;
}

/** The edits between `a` and `b`. */
function distance(a: string, b: string): number {
    const row = Array.from({ length: b.length + 1 }, (_, i) => i);
    for (let i = 1; i <= a.length; i++) {
        let diagonal = row[0];
        row[0] = i;
        for (let j = 1; j <= b.length; j++) {
            const kept = row[j];
            row[j] = Math.min(row[j] + 1, row[j - 1] + 1, diagonal + (a[i - 1] === b[j - 1] ? 0 : 1));
            diagonal = kept;
        }
    }
    return row[b.length];
}

/** How well typed words match a phrase: by its first letters, a later word, or with one typing error from 4 letters on
 * and two from 7; 0 for no match. */
function score(typed: string, phrase: string): number {
    if (phrase.startsWith(typed)) return 100 - Math.min(20, phrase.length - typed.length);
    if (phrase.split(' ').slice(1).some(word => word.startsWith(typed))) return 70;
    const errors = typed.length >= 4 ? distance(typed, phrase.slice(0, typed.length)) : Infinity;
    return errors <= 1 ? 55 : errors <= 2 && typed.length >= 7 ? 45 : 0;
}

/**
 * The values that the words in progress can become, at most 6, the best first: the values of a quantity, the usual values
 * of a word that starts one, then the phrases by `score`. After a word of `not`, the `not` of the phrases. Words that
 * are already a glue word or a phrase get only the values of those exact words, so Enter does not change them.
 */
export function suggest<V extends Value>(fragment: string, table: Table<V>): V[] {
    const { phrases, glue, not } = compiled(table);
    let typed = fold(fragment).trim().replace(/\s+/g, ' ');
    const first = typed.split(' ')[0], negated = not.has(first);
    if (negated) typed = typed.slice(first.length).trim();
    if (!typed && !negated) return [];
    const words = typed.split(' '), exactly = (text: string) => {
        const own = text.split(' ');
        return own.length === words.length && own.every((word, i) => same(words[i], word, i === own.length - 1));
    };
    const exact = glue.has(typed) || phrases.some(phrase => exactly(phrase.text));
    const found: { score: number; value: V }[] = [];
    if (!negated) {
        for (const pattern of table.patterns) {
            const match = pattern.match.exec(typed);
            if (match?.[0].trimEnd().length === typed.length) found.push(...pattern.values(match).map((value, i) => ({ score: 96 - i, value })));
        }
        if (typed.length >= 2 && !/\d/.test(typed)) {
            for (const start of table.starts) {
                if (start.words.some(lead => exact ? lead === typed : lead.startsWith(typed))) found.push(...start.values.map(value => ({ score: 84, value })));
            }
        }
    }
    for (const phrase of phrases) {
        const meaning = negated ? phrase.not : phrase.value, matched = !typed ? 50 : exact ? exactly(phrase.text) && 100 : score(typed, phrase.text);
        if (meaning && typeof meaning !== 'string' && matched) found.push({ score: matched, value: meaning });
    }
    const best = new Map<string, { score: number; value: V }>();
    for (const one of found) {
        const key = `${one.value.slot}\n${one.value.label}`, had = best.get(key);
        if (!had || one.score > had.score) best.set(key, one);
    }
    // `sort` keeps the order of the table for equal scores.
    return [...best.values()].sort((a, b) => b.score - a.score).slice(0, 6).map(one => one.value);
}
