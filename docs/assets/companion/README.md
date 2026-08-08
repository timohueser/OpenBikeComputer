# Companion app captures

These five WebP files are real screens from the iOS companion app. The landing page uses them for
the route-upload and ride-download bookends around the live device simulator.

Regenerate them from the repository root with:

```sh
companion-ios/scripts/capture-website-screenshots.sh
```

The script first derives the app import from `apps/obc-sim/assets/grimsel-climb.gpx` and the
finished partial ride from its `grimsel-climb-demo.gpx` replay. It then runs the focused
`WebsiteScreenshotTests` flow on an iPhone 17 Pro simulator, pins the
locale, appearance, text size, network fallback, and status bar, then exports and compresses the
named XCUITest attachments. CI runs the same script with `--check`, so a companion UI change cannot
silently leave stale landing-page captures behind.
