// Posts an agent's reply to a region's review thread; the review page shows it within seconds.
//
//   node tune/comment.mjs <short> [--spot <id>] [--round <n>] <text…>
//
// Without --round the comment belongs to the newest round. Prints the line it appended.

import { parseArgs } from 'node:util';
import { comment, known } from './review.mjs';

const { values, positionals: [short, ...words] } = parseArgs({
  allowPositionals: true,
  options: { spot: { type: 'string' }, round: { type: 'string' } },
});
const text = words.join(' ').trim();
if (!short || !text || (values.round && !/^[1-9]\d*$/.test(values.round))) {
  console.error('usage: node tune/comment.mjs <short> [--spot <id>] [--round <n>] <text…>');
  process.exit(2);
}
if (!known(short)) {
  console.error(`${short} is not a region short name: see tune/ecoregions.json`);
  process.exit(2);
}
const round = values.round && Number(values.round);
console.log(JSON.stringify(comment(short, { round, spot: values.spot, text, by: 'agent' })));
