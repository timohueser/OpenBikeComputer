import {nativeCells} from './federation.mjs';
import {searchRuntime} from './runtime.mjs';
import {openingHours} from './hours.mjs';

/** All query algorithms stay in the shared runtime. The phone has no language model, so a
 * sentence that needs inference fails visibly. */
export function nativeSearch(native) {
  const db = nativeCells(native);
  const parser = {parse() { throw new Error('Native search requires a structured request.'); }};
  const {query} = searchRuntime({db, parser, hours: openingHours(db.metadata.time_zone)});
  return {query};
}
