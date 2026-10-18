// The owner's review of the Europe tune: per baseline region its spots, each round of shots beside
// the round before, and comment threads for the region and each spot. The dev server reads and
// writes the review folder (tune/review.mjs); agents reply with tune/comment.mjs.

type Status = 'no shots' | 'in review' | 'approved';
interface Region { short: string; name: string; countries: string[]; status: Status; round: number }
interface Comment { time: string; round: number; spot?: string; text: string; by: 'owner' | 'agent'; approve?: true }
interface Review { rounds: Record<string, string[]>; comments: Comment[] }
interface Spot { id: string; name: string }
interface Shown { src: string; caption: string }
interface View { short: string; update(review: Review): void }

const SPOTS = import.meta.glob<Spot[]>('../spots/*.json', { eager: true, import: 'default' });
const STATUS: Record<Status, string> = { 'in review': 'In review', approved: 'Approved', 'no shots': 'No shots yet' };
const POLL_MS = 4000;

const nav = document.querySelector('nav')!;
const main = document.querySelector('main')!;
const zoom = document.querySelector('dialog')!;
let regions: Region[] = [];
let view: View | null = null;
let navShown = '';

function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: (Node | string | null | false)[]): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  Object.assign(element, props);
  element.append(...children.filter((c) => c !== null && c !== false));
  return element;
}

async function get<T>(url: string): Promise<T | null> {
  const response = await fetch(url, { cache: 'no-store' }).catch(() => null);
  return response?.ok ? response.json() : null;
}

const when = (time: string) => new Date(time).toLocaleString('en-GB', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });

/** The spot of a shot: the longest spot id its name starts with, as one id can begin another. */
const spotOf = (file: string, ids: string[]) => ids.filter((id) => file.startsWith(`${id}-`)).sort((a, b) => b.length - a.length)[0];

/** Two rounds' shots in rows: equal file names side by side, then the others in order. */
function pairs(before: string[], after: string[]): (string | undefined)[][] {
  const left = before.filter((f) => !after.includes(f));
  const right = after.filter((f) => !before.includes(f));
  const rest = Array.from({ length: Math.max(left.length, right.length) }, (_, i) => [left[i], right[i]]);
  return [...after.filter((f) => before.includes(f)).map((f) => [f, f]), ...rest];
}

let zoomed: Shown[] = [];
let zoomedAt = 0;
function enlarge(shown: Shown[], at: number): void {
  [zoomed, zoomedAt] = [shown, at];
  zoom.querySelector('img')!.src = shown[at].src;
  zoom.querySelector('p')!.textContent = `${shown[at].caption}${shown.length > 1 ? ' · ← → the other round' : ''} · Esc closes`;
  if (!zoom.open) zoom.showModal();
}
zoom.addEventListener('click', () => zoom.close());
zoom.addEventListener('keydown', (event) => {
  if (zoomed.length < 2 || !['ArrowLeft', 'ArrowRight', ' '].includes(event.key)) return;
  event.preventDefault();
  enlarge(zoomed, 1 - zoomedAt);
});

function figure(shown: Shown[], at: number, caption: string): HTMLElement {
  const open = (event: MouseEvent) => {
    event.preventDefault();
    enlarge(shown, at);
  };
  return h('figure', {}, h('a', { href: shown[at].src, onclick: open }, h('img', { src: shown[at].src, alt: shown[at].caption, loading: 'lazy' })), h('figcaption', { textContent: caption }));
}

/** Round n beside round n − 1; prefix is the spot id the file names start with. */
function shots(short: string, n: number, before: string[], after: string[], prefix: string): HTMLElement[] {
  if (!before.length && !after.length) return [h('p', { className: 'muted wide', textContent: n ? `No shots in round ${n}.` : 'No shots yet.' })];
  const label = (file: string) => file.slice(prefix.length, -'.png'.length);
  return [
    h('h4', { textContent: n > 1 ? `Round ${n - 1}` : 'No earlier round' }),
    h('h4', { textContent: `Round ${n}` }),
    ...pairs(before, after).flatMap((row) => {
      const cells = row.map((file, i) => file && { round: n - 1 + i, file });
      const shown = cells.filter((c) => !!c).map((c) => ({ src: `/review/${short}/${c.round}/${encodeURIComponent(c.file)}`, caption: `Round ${c.round} · ${c.file}` }));
      return cells.map((c, i) => (c ? figure(shown, shown.length > 1 ? i : 0, label(c.file)) : h('div', { className: 'empty', textContent: n - 1 + i ? `not in round ${n - 1 + i}` : 'first round' })));
    }),
  ];
}

function thread(comments: Comment[], ids: Set<string>): HTMLElement[] {
  return comments.map((c) => {
    const about = [c.by === 'agent' ? 'Agent' : 'Owner', when(c.time), `round ${c.round}`, c.spot && !ids.has(c.spot) && c.spot, c.approve && 'approved'];
    return h('div', { className: `comment ${c.by}${c.approve ? ' approval' : ''}` }, h('small', { textContent: about.filter(Boolean).join(' · ') }), h('p', { textContent: c.text }));
  });
}

/** A comment box, and its post: Send, ⌘/Ctrl + Enter, or an approval that sends the box's text. */
function composer(short: string, placeholder: string, spot?: string): [HTMLElement, (approve?: boolean) => Promise<void>] {
  const text = h('textarea', { placeholder, ariaLabel: placeholder, rows: 2 });
  const error = h('p', { className: 'error', hidden: true });
  const send = h('button', { textContent: 'Send' });
  const post = async (approve = false) => {
    const body = text.value.trim() || (approve ? 'Approved.' : '');
    if (!body) return;
    send.disabled = true;
    const response = await fetch(`/__comment/${short}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text: body, spot, approve }),
    }).catch(() => null);
    send.disabled = false;
    error.hidden = !!response?.ok;
    error.textContent = `Not saved: ${response ? `the dev server answered ${response.status}` : 'the dev server does not answer'}.`;
    if (!response?.ok) return;
    text.value = '';
    await refresh();
  };
  send.onclick = () => post();
  text.addEventListener('keydown', (event) => {
    if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return;
    event.preventDefault();
    void post();
  });
  return [h('div', { className: 'composer' }, text, send, error), post];
}

function spotArticle(short: string, spot: Spot) {
  const [box] = composer(short, `Comment on ${spot.name}…`, spot.id);
  const grid = h('div', { className: 'pair' });
  const comments = h('div', { className: 'thread' });
  const region = h('span', { className: 'mono muted' });
  const reference = figure([{ src: `/${spot.id}/photo.jpg`, caption: `Reference photo of ${spot.name}` }], 0, '');
  reference.querySelector('img')!.onerror = () => reference.replaceWith(h('div', { className: 'empty', textContent: 'No reference photo in the bake' }));
  // An old bake has no meta.region. A spot without a bake answers with the page, which is no JSON.
  get<{ region?: string; photo?: { credit?: string } }>(`/${spot.id}/meta.json`)
    .then((meta) => {
      region.textContent = meta ? (meta.region ?? '') : 'not baked';
      reference.querySelector('figcaption')!.textContent = meta?.photo?.credit ?? '';
    })
    .catch(() => (region.textContent = 'not baked'));
  const article = h('article', {},
    h('header', {}, h('h3', { textContent: spot.name }), h('span', { className: 'mono muted', textContent: spot.id }), region,
      h('a', { href: `/index.html#spot=${encodeURIComponent(spot.id)}`, target: '_blank', rel: 'noopener', textContent: 'Live view ↗' })),
    h('div', { className: 'pair' }, reference, h('div', {}, comments, box)),
    grid,
  );
  return { article, grid, comments };
}

function open(short: string): void {
  const region = regions.find((r) => r.short === short)!;
  const spots = SPOTS[`../spots/${short}.json`] ?? [];
  const ids = spots.map((s) => s.id);
  const known = new Set(ids);
  const pill = h('span', { className: 'pill' });
  const chips = h('div', { className: 'rounds' });
  const [box, post] = composer(short, `Comment on ${region.name}…`);
  const approve = h('button', { className: 'approve', onclick: () => post(true) });
  const comments = h('div', { className: 'thread' });
  const loose = h('div', { className: 'pair' });
  const looseSection = h('section', { hidden: true }, h('h2', { textContent: 'Shots of no listed spot' }), loose);
  const articles = spots.map((spot) => spotArticle(short, spot));
  main.replaceChildren(
    h('div', { className: 'head' },
      h('div', {}, h('h1', {}, region.name, pill), h('p', { className: 'muted', textContent: [short, ...region.countries].join(' · ') })),
      chips, approve),
    h('section', {}, h('h2', { textContent: 'Region' }), comments, box),
    looseSection,
    h('section', {}, h('h2', { textContent: `Spots · ${spots.length}` }),
      !spots.length && h('p', { className: 'muted', textContent: `spots/${short}.json lists no spots yet.` }), ...articles.map((a) => a.article)),
  );
  main.scrollTop = 0;

  let chosen: number | null = null;
  let review: Review = { rounds: {}, comments: [] };
  let shownKey = '';
  const update = (next: Review) => {
    review = next;
    const status = regions.find((r) => r.short === short)!.status;
    const key = JSON.stringify([review, status, chosen]);
    if (key === shownKey) return;
    shownKey = key;
    const numbers = Object.keys(review.rounds).map(Number).sort((a, b) => a - b);
    const newest = numbers.at(-1) ?? 0;
    const n = chosen !== null && numbers.includes(chosen) ? chosen : newest;
    pill.className = `pill ${status.replace(' ', '-')}`;
    pill.textContent = STATUS[status];
    chips.replaceChildren(...(numbers.length ? [h('span', { className: 'muted', textContent: 'Round' })] : []), ...numbers.map((r) =>
      h('button', { textContent: String(r), ariaPressed: String(r === n), onclick: () => ((chosen = r === newest ? null : r), update(review)) })));
    approve.textContent = newest ? `Approve round ${newest}` : 'Approve';
    approve.disabled = status !== 'in review';
    comments.replaceChildren(...thread(review.comments.filter((c) => !c.spot || !known.has(c.spot)), known));
    const of = (round: number, id?: string) => (review.rounds[round] ?? []).filter((f) => spotOf(f, ids) === id);
    for (const [i, spot] of spots.entries()) {
      articles[i].grid.replaceChildren(...shots(short, n, of(n - 1, spot.id), of(n, spot.id), `${spot.id}-`));
      articles[i].comments.replaceChildren(...thread(review.comments.filter((c) => c.spot === spot.id), known));
    }
    looseSection.hidden = !of(n - 1).length && !of(n).length;
    loose.replaceChildren(...shots(short, n, of(n - 1), of(n), ''));
  };
  view = { short, update };
}

function renderNav(): void {
  const count = (status: Status) => regions.filter((r) => r.status === status).length;
  nav.replaceChildren(
    h('header', {}, h('h1', { textContent: 'Trail View review' }),
      h('p', { className: 'muted', textContent: `${count('in review')} in review · ${count('approved')} approved · ${count('no shots')} without shots` })),
    ...(Object.keys(STATUS) as Status[]).flatMap((status) => {
      const members = regions.filter((r) => r.status === status);
      return !members.length ? [] : [h('h2', { textContent: STATUS[status] }), ...members.map((r) =>
        h('button', { title: `${r.name} (${r.short})`, ariaCurrent: String(r.short === view?.short), onclick: () => (location.hash = r.short) },
          h('span', { className: `dot ${r.status.replace(' ', '-')}` }), h('span', { textContent: r.name }),
          h('span', { className: 'mono muted', textContent: r.round ? `${r.short} · r${r.round}` : r.short })))];
    }),
  );
}

async function refresh(): Promise<void> {
  const list = await get<Region[]>('/__review');
  if (!list?.length) return;
  regions = list;
  const asked = location.hash.slice(1);
  const region = regions.find((r) => r.short === asked) ?? regions.find((r) => r.status === 'in review') ?? regions[0];
  if (region.short !== asked) history.replaceState(null, '', `#${region.short}`);
  if (view?.short !== region.short) open(region.short);
  const navKey = JSON.stringify([regions, region.short]);
  if (navKey !== navShown) {
    navShown = navKey;
    renderNav();
  }
  const review = await get<Review>(`/__review/${region.short}`);
  if (review && view?.short === region.short) view.update(review);
}

addEventListener('hashchange', () => void refresh());
document.addEventListener('visibilitychange', () => document.visibilityState === 'visible' && void refresh());
setInterval(() => document.visibilityState === 'visible' && void refresh(), POLL_MS);
void refresh();
