// The one place the app reaches its host. `$host` is a Vite alias resolved at
// build time (see vite.config.ts) to exactly one of web.ts / desktop.ts /
// dev.ts, so this is a static re-export, not a dispatch: the hosts you did not
// build have no path into the module graph. That, rather than any dead-code
// elimination, is what keeps maintainer-only and native-only code out of the
// static web bundle.
//
// Type-checking resolves `$host` to dev.ts (tsconfig `paths`) — the mode
// `npm run dev` and `npm run build` use. All three hosts are still checked as
// files, each against `Platform`.

export { platform } from "$host";
export * from "./types";
