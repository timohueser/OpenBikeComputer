import {answerQuery} from './query.mjs';
import {reverseAddress} from './web/reverse.mjs';
import {validateInput} from './validation.mjs';

/** Database, model and calendar adapters are local capabilities; no transport is required. */
export function searchRuntime({db,parser,hours,clock=Date.now}) {
  if (typeof db?.rows!=='function' || typeof parser?.parse!=='function'
      || typeof hours?.openingState!=='function' || typeof hours?.currentOpening!=='function')
    throw new Error('Search needs database, parser and opening-hours adapters.');
  return {
    async query(input) {
      validateInput(input);
      const answer = await answerQuery(db,{...input,now:input.now ?? new Date(clock()).toISOString(),
        openingState:hours.openingState,timeZone:hours.timeZone},parser,hours);
      return {...answer,attribution:db.metadata.attribution};
    },
    reverse:coordinate=>({label:reverseAddress(db,coordinate)}),
  };
}
