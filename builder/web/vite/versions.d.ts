// The versions of the code that the device cache keys by, from bundleVersions of bundle-version.ts.
declare module "virtual:plan-version" {
    const version: string;
    export default version;
}
declare module "virtual:house-version" {
    const version: string;
    export default version;
}

// The model files of the build, from modelFiles of model-files.ts.
declare module "virtual:models" {
    const models: import("../src/lib/trailview/models/model-file").Models;
    export default models;
}
