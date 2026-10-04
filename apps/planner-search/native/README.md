# Native search

The Companion app compiles the Swift files in this directory. Link
JavaScriptCore and SQLite. Use iOS 17 or later. The
[app project](../../../companion-ios/project.yml) lists the files and builds the
shared JavaScript resource before it generates the project. To build it by hand,
run from the repository root:

```sh
npm ci --prefix apps/planner-search
node apps/planner-search/native-build.mjs OUTPUT
```

Keep one `PlannerSearchRuntime` per installed map inside one actor. Pass the
installed search databases, their cell bounds, the scripts directory and the region
identifier. The runtime reads the region time zone from the search metadata. Keep
the script and its generated license notices.

Call `request("query", body: jsonData)` with the web query body, or
`request("reverse", body: jsonData)` with a `coordinate` pair. The return value
is the same JSON response as the server. Errors throw. The owning actor serializes
requests and retains the prepared statements and the regional calendar.
Initialization does file I/O; run it outside the main actor. The native provider
has no network capability and does not commit route changes.

The phone has no language model. Exact names and edited requests run in full.
A sentence that needs inference returns a notice that asks for a structured request.
