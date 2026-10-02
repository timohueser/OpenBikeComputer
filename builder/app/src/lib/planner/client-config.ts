export interface PlannerClientConfig {
    region?: string;
    bounds?: [number, number, number, number];
    terrainWorkerUrl?: string;
}

/** The native composition root sets this before the shared planner modules load. */
export const clientConfig: PlannerClientConfig =
    (globalThis as typeof globalThis & { __OBC_PLANNER_CONFIG__?: PlannerClientConfig }).__OBC_PLANNER_CONFIG__ ?? {};
