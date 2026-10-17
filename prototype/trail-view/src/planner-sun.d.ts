// builder/web/src/lib/planner/layers/sun.ts, which vite.config.ts aliases as `planner-sun`. The type
// check reads only this, because the planner's own imports need packages installed in builder/web.
declare module 'planner-sun' {
  export function solarTerms(instant: number): { declination: number; equation: number; minutes: number };
  export function solarPosition(
    coordinate: [number, number],
    terms: ReturnType<typeof solarTerms>,
  ): { bearing: number; altitude: number };
  export function instantAt(date: string, minute: number, timezone: string): number;
  export function clock(minute: number): string;
}
