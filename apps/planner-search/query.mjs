import { simpleRequest, search, cuisineOf } from './web/engine.mjs';
import { currentOpening } from './hours.mjs';
import { resolve } from './resolver.mjs';
import { validateRequest, RequestError } from './validation.mjs';
import { localQuery } from './local-query.mjs';

export async function answerQuery(db, input, parser, hours = {currentOpening}) {
  if (input.source) {
    const now = input.now === undefined ? Date.now() : Date.parse(input.now);
    const results = db.all('SELECT p.* FROM places p WHERE p.source = ? LIMIT 1', [input.source], {bounds: input.view});
    return {type:'places', request:{type:'place',name:input.q}, results:results.map(place=>({
      ...place, precision:'place', distance:0, hoursStatus:hours.currentOpening(place,now),
    }))};
  }
  let canRetry = false;
  const local = !input.request && input.submitted && input.q.length <= 80
    ? localQuery(db, input.q) : null;
  let ordinary;
  let request = input.request || local || simpleRequest(input.q),
    notice = '',
    elapsed = 0;
  // Exact names and dictionary categories stay usable without loading a model.
  if (
    !input.request &&
    !local &&
    input.submitted &&
    request.type === 'place' &&
    input.q.length <= 80
  ) {
    ordinary = search(db, { q: input.q, view: input.view, limit: input.limit });
    const exact = ordinary.results.some(
      (p) => p.why.match >= 88 || p.precision === 'house' || p.why.house,
    );
    if (!exact) {
      let parsed;
      try {
        parsed = await parser.parse(input.q);
        validateRequest(parsed.request);
        request = parsed.request;
        ordinary = null;
        elapsed = parsed.elapsed;
      } catch (error) {
        // Model labels and the request contract can drift; that must not fail the search.
        const unsupported = error instanceof RequestError;
        if (unsupported) console.error('Unsupported query model request:', JSON.stringify(parsed.request));
        canRetry = !unsupported;
        notice = `${unsupported ? 'The query model returned an unsupported request.' : error.message} Showing ordinary place search.`;
      }
    }
  } else if (!input.request && input.submitted && input.q.length > 80)
    notice =
      'Smart requests use at most 80 characters. Showing ordinary place search.';
  if (request.type === 'places') {
    const cuisine =
      request.cuisine ||
      request.what.find((k) => cuisineOf(k)) ||
      (!input.request &&
        cuisineOf(input.q.split(/\s+(?:in|near|bei|dans|à)\s+/i)[0]));
    if (cuisine)
      request = {
        ...request,
        what: [
          ...new Set(request.what.map((k) => (cuisineOf(k) ? 'food' : k))),
        ],
        cuisine,
      };
  }
  if (request.type === 'none') {
    notice = 'The sentence was not understood. Showing ordinary place search.';
    request = { type: 'place', name: input.q, ignored: request.ignored || [] };
  }
  let answer;
  try {
    answer = ordinary ? { type: 'places', ...ordinary } : resolve(db, request, input);
  } catch (error) {
    answer = { type: 'unresolved', results: [], note: error.message };
  }
  const now = input.now === undefined ? Date.now() : Date.parse(input.now);
  if (answer.results) answer.results = answer.results.map(place => ({ ...place, hoursStatus: hours.currentOpening(place, now) }));
  return { ...answer, request, notice, canRetry, parserMs: elapsed };
}
