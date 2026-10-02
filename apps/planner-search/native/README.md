# Native search

Compile the Swift and Objective-C++ files in this directory into the app target.
Link JavaScriptCore, SQLite, the Python framework, native ONNX Runtime, and the
Rust tokenizer library. Use C++17 and iOS 17 or later. The
[phone project](../../../host/route-engine/examples/phone/project.yml) lists the
link and resource settings. Its [setup commands](../../../host/route-engine/examples/README.md)
prepare pinned native dependencies and the unchanged Python decoder.

Build both shared JavaScript resources from the repository root:

```sh
npm ci --prefix apps/planner-search
node apps/planner-search/native-build.mjs OUTPUT
```

Keep one `PlannerSearchSession` actor per installed region. Pass the verified
release's database and model hashes, local paths, region identifier, country
code, and IANA time zone. Keep both scripts and their generated license notices.
The Python bundle contains `python/` and `app/` resources. Its default is the
main application bundle. Initialization verifies all data hashes before use.

Call `await session.request("query", body: jsonData)` with the web query body,
or `await session.request("reverse", body: jsonData)` with a `coordinate` pair.
The return value is the same JSON response as the server. Errors throw. Actor
isolation serializes requests and retains the model, prepared statements, and
regional calendar. Initialization does file I/O and model setup; run it outside
the main actor. The composition root owns the session. The native provider has
no network capability and does not commit route changes.

The model receives sentence text only. SQLite, the shared resolver, and the
regional calendar produce results. Edited requests bypass inference. Native
ONNX Runtime must match the host version used to verify the model. Use the
complete phone smart-search corpus after a runtime or bundle change.
