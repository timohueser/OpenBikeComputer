// Every fetch goes through API_BASE so a deployment can relocate the API with
// a build-time env var (VITE_API_BASE) instead of code changes.
export const API_BASE: string = import.meta.env.VITE_API_BASE ?? "/api";

// External links, in one place for the day the docs/landing move off github.io.
export const LINKS = {
    docs: "https://timohueser.github.io/OpenBikeComputer/docs/",
    simulator: "https://timohueser.github.io/OpenBikeComputer/",
    github: "https://github.com/timohueser/OpenBikeComputer",
};
