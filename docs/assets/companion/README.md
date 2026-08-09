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

The check compares pixels, so the capture has to be deterministic: the app runs unanimated
(`-OBCDisableAnimations`) with every timed confirmation parked (`-OBCHoldConfirmations` — the
post-sync check, the synced line, and the upload sheet that otherwise closes itself 2.6 s after
"On the device"), every asynchronously arriving element is waited for by accessibility identifier
(profiles and previews, plus the device name and battery, which land after the connect), and a
screenshot is only kept once two consecutive frames come back identical. A frame caught mid-render
— or a beat after a short-lived state expired — would otherwise read as a stale asset on a slow
runner (#1212).
