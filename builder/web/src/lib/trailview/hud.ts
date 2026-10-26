import type { TrailView } from './engine';
import { dayOfYear } from './ground/rules';
import { LABS, type Labs } from './labs';
import { DEBUG_VIEWS, formatView, type DebugView } from './view';

const REFRESH_MS = 250;
/** A new date builds the ground again, so the date slider takes effect once it rests this long after a change: a step of an arrow key is a change. */
const DATE_REST_MS = 400;

/**
 * The HUD: the GPU time of a frame and its passes, the date and the time of day, the debug view and
 * the lab variants, and a line for what goes wrong. H hides it.
 */
export function hud(element: HTMLElement, engine: TrailView, labs: Labs): void {
    const text = document.createElement('pre');
    const debug = document.createElement('select');
    debug.setAttribute('aria-label', 'Debug view');
    for (const view of DEBUG_VIEWS) debug.add(new Option(`debug: ${view}`, view));
    debug.addEventListener('change', () => (engine.view = { ...engine.view, debug: debug.value as DebugView }));
    // The time follows its slider as it moves.
    const [date, dateLabel] = slider('Date', 0, 364, 1);
    const [time, timeLabel] = slider('Time', 0, 24 * 60 - 15, 15);
    const dateOf = (day: number) => new Date(Date.UTC(Number(engine.view.date.slice(0, 4)), 0, 1 + day)).toISOString().slice(0, 10);
    const timeOf = (minute: number) => `${String(Math.floor(minute / 60)).padStart(2, '0')}:${String(minute % 60).padStart(2, '0')}`;
    let resting: ReturnType<typeof setTimeout> | undefined;
    date.addEventListener('input', () => (dateLabel.textContent = dateOf(date.valueAsNumber)));
    date.addEventListener('change', () => {
        clearTimeout(resting);
        resting = setTimeout(() => ((resting = undefined), (engine.view = { ...engine.view, date: dateOf(date.valueAsNumber) })), DATE_REST_MS);
    });
    time.addEventListener('input', () => (engine.view = { ...engine.view, time: (timeLabel.textContent = timeOf(time.valueAsNumber)) }));
    // A control that the pointer used gives the keys back to the eye (controls.ts).
    for (const range of [date, time]) range.addEventListener('pointerup', () => range.blur());
    debug.addEventListener('change', () => debug.blur());
    // The engine builds its variants at start, so a change of variant loads the page again at the same view.
    const variants = Object.entries(LABS).map(([name, names]) => {
        const select = document.createElement('select');
        select.setAttribute('aria-label', `Lab ${name}`);
        for (const variant of names) select.add(new Option(`${name}: ${variant}`, variant));
        select.value = labs[name as keyof Labs];
        select.addEventListener('change', () => {
            const chosen = { ...labs, [name]: select.value };
            const q = new URLSearchParams(location.search);
            q.delete('lab');
            q.set('lab', Object.entries(chosen).map(([lab, variant]) => `${lab}:${variant}`).join(','));
            location.search = formatView(engine.view, `?${q}`);
        });
        return select;
    });
    element.replaceChildren(text, date.parentElement!, time.parentElement!, debug, ...variants);
    const update = () => {
        const stats = engine.stats(), view = engine.view, passes = Object.entries(stats.gpu);
        debug.value = view.debug;
        if (document.activeElement !== date && resting === undefined) [date.valueAsNumber, dateLabel.textContent] = [dayOfYear(view.date), view.date];
        if (document.activeElement !== time) [time.valueAsNumber, timeLabel.textContent] = [Number(view.time.slice(0, 2)) * 60 + Number(view.time.slice(3)), view.time];
        const frame = passes.reduce((sum, [, ms]) => sum + ms, 0);
        const problems = [
            stats.recoveries && `${stats.recoveries} lost devices recovered`,
            stats.failed && `${stats.failed} requests failed`,
            stats.ground.shortage && `${stats.ground.shortage} ground pages short`,
            stats.instances.dropped && `${stats.instances.dropped} instances dropped`,
            stats.terrain.quality < 0 && `terrain quality ${stats.terrain.quality}`,
        ].filter(Boolean);
        text.textContent = [
            passes.length ? `frame ${frame.toFixed(2)} ms GPU ≈ ${Math.round(1000 / frame)} fps · cpu ${stats.cpu.toFixed(2)} ms` : 'no timestamp queries',
            passes.map(([pass, ms]) => `${pass} ${ms.toFixed(2)}`).join(' · '),
            ...(problems.length ? [`! ${problems.join(' · ')}`] : []),
            'W A S D move · drag to look · F walk or fly · wheel altitude · Ctrl+C / Ctrl+V view · H HUD',
        ].join('\n');
    };
    // A hidden HUD reads no stats: the engine then takes no GPU times.
    let poll: ReturnType<typeof setInterval> | undefined;
    const show = () => {
        clearInterval(poll);
        poll = element.hidden ? undefined : (update(), setInterval(update, REFRESH_MS));
    };
    addEventListener('keydown', event => {
        if (event.key.toLowerCase() !== 'h' || event.ctrlKey || event.metaKey) return;
        element.hidden = !element.hidden;
        show();
    });
    show();
}

/** A labelled range input, and the label's value text. */
function slider(name: string, min: number, max: number, step: number): [HTMLInputElement, HTMLSpanElement] {
    const label = document.createElement('label'), input = document.createElement('input'), value = document.createElement('span');
    Object.assign(input, { type: 'range', min: String(min), max: String(max), step: String(step) });
    input.setAttribute('aria-label', name);
    label.append(`${name} `, input, value);
    return [input, value];
}
