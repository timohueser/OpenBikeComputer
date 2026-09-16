import { json } from '@sveltejs/kit';
import { store } from '../../lib/server/store.ts';
export const GET = () => {
  try {
    const revision = store().latestRevision();
    if (!revision) return json({ ok: false }, { status: 503 });
    return json({ ok: true });
  } catch { return json({ ok: false }, { status: 503 }); }
};
