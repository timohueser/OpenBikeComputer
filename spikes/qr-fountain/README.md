# QR fountain spike

A throwaway prototype. A web page shows a route as an animated QR code, and an iOS app reads it
with the camera. No server and no account are necessary. The frames use a fountain code, so the
phone can start at any frame and miss frames.

| Path | Contents |
| --- | --- |
| `web/` | Sender page. `fountain.js` makes the frames, `route.js` makes the route payload. |
| `ios/` | Receiver app. `Fountain.swift` decodes the frames, `Route.swift` decodes the route. |
| `check/` | Sends JS frames, with frames missed, to the Swift decoder. |

## Run the sender

```sh
python3 -m http.server 8000 -d web
```

Open <http://localhost:8000>. The page loads `qrcode-generator` from jsDelivr, so the computer
must be online. Select a synthetic route size or a GPX file, then push **Start**.

## Run the receiver

1. Install XcodeGen (`brew install xcodegen`).
2. `cd ios && xcodegen generate && open QRFountain.xcodeproj`.
3. In **Signing & Capabilities**, select your team.
4. Run on an iPhone. The simulator has no camera.
5. Hold the phone 30–50 cm from the screen, with the full code in the camera view.

When the transfer is complete, the app shows the route, the time and the frame count.

## Check the codec

```sh
check/run.sh
```

The script needs `swiftc` and `node`. The JS encoder and the Swift decoder must agree bit for bit.

## Settings to try

| Setting | Effect |
| --- | --- |
| Block bytes | Larger blocks make fewer, denser frames. 600 bytes gives a version 17 code at level L. |
| Frames per second | Faster frames are better until the camera starts to miss them. |
| Error correction | L is sufficient on a clean screen. M and Q tolerate glare, but each frame becomes denser. |
