import { simpleRequest, search, cuisineOf } from './web/engine.mjs';
import { resolve } from './resolver.mjs';
import { validateRequest } from './validation.mjs';

export async function answerQuery(db, input, parser) {
  let canRetry = false;
  let request = input.request || simpleRequest(input.q),
    notice = '',
    elapsed = 0;
  // Exact names and dictionary categories stay usable without loading a model.
  if (
    !input.request &&
    input.submitted &&
    request.type === 'place' &&
    input.q.length <= 80
  ) {
    const exact = search(db, { q: input.q, view: input.view }).results.some(
      (p) => p.why.match >= 96 || p.precision === 'house' || p.why.house,
    );
    if (!exact)
      try {
        const parsed = await parser.parse(input.q);
        request = parsed.request;
        elapsed = parsed.elapsed;
      } catch (error) {
        canRetry = true;
      notice = `${error.message} Showing ordinary place search.`;
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
  validateRequest(request);
  if (request.type === 'none') {
    notice = 'The sentence was not understood. Showing ordinary place search.';
    request = { type: 'place', name: input.q, ignored: request.ignored || [] };
  }
  let answer;
  try {
    answer = resolve(db, request, input);
  } catch (error) {
    answer = { type: 'unresolved', results: [], note: error.message };
  }
  return { ...answer, request, notice, canRetry, parserMs: elapsed };
}
