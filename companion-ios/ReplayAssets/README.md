# Replay renderer assets

Requires Node.js 22 or later. From the repository root:

```sh
npm ci --prefix companion-ios/ReplayAssets
npm test --prefix companion-ios/ReplayAssets
```

XcodeGen runs the install before it generates the app project. The install copies the pinned
Cesium runtime, workers, assets, licence and third-party notices into the OBCKit resource bundle.
Generated files stay out of git. Run the install before standalone OBCKit package tests.

The native player serves only bundled files on a loopback HTTP origin. It fetches terrain and
imagery from the providers in `../Packages/OBCKit/Sources/OBCUI/Resources/Replay/providers.mjs`.
Those endpoints are for owner development. Distribution needs a separate provider access review.
Provider credits remain visible in the map.
