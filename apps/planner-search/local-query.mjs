import { kindOf, norm } from './web/engine.mjs';

function known(db, text, locality = false) {
  return db.all(`SELECT 1 FROM names n JOIN places p ON p.id=n.place_id
    WHERE n.term=? ${locality ? "AND p.kind IN ('city','town','village','hamlet','district','suburb','locality')" : ''}
    LIMIT 1`, [norm(text)]).length > 0;
}

// Only split a literal category or mapped name from a mapped locality. Sentences go to the model.
export function localQuery(db, text) {
  if (db.all('SELECT 1 FROM names WHERE term>=? AND term<? LIMIT 1',
    [norm(text), norm(text) + '\uffff']).length) return null;
  const words = text.trim().split(/\s+/);
  for (let i = 1; i < words.length; i++) {
    const name = words.slice(0, i).join(' ');
    const place = words.slice(i).join(' ').replace(/^(?:in|near|bei|dans|à|a|en|vicino a)\s+/i, '');
    const kind = kindOf(name);
    if ((!kind && !known(db, name)) || !known(db, place, true)) continue;
    return kind
      ? { type: 'places', what: [kind], where: { near: [{ name: place }] } }
      : { type: 'place', name, near: { name: place } };
  }
  return null;
}
