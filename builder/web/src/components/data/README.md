# Operator page

The page of `obc data` on one machine. `obc data ui` builds it and serves it on
`http://127.0.0.1:8700/`. The page acts only through `POST /api/cli`, which runs
`obc data --json ARGS` on that machine, and each action shows its command. The
[data contract](../../../../../specs/obc-data.md#the-page) defines the route and its checks.

## Run

From the repository root:

```sh
obc data ui              # build the page, then serve it until Ctrl-C
obc data open MACHINE    # the page of MACHINE, through an SSH tunnel at http://localhost:8701/
```

`obc data ui` needs Node and `npm ci` in `builder/web`. It refuses a request from another page,
and it runs only the commands that the page uses. A command runs to its end also when the page
stops waiting for it. `data/machines.toml` sets the header colour and the SSH host of each machine.

A systemd unit for `obc data ui` starts `tools/obc data ui`, so that `tools/obc.local` loads, and
sets `KillMode=process`, so that a restart of the page does not stop the runs that it started.

## Develop

From `builder/web`, while `obc data ui` runs:

| Command | Action |
| --- | --- |
| `npm run dev -- --mode data` | Serve the page at `http://localhost:5173/data.html`; `/api` goes to port 8700 |
| `npm run build:data` | Build `dist/data/` |
| `npm run check` | Check the types |
| `npm test` | Run the component tests |
| `npm run test:data --prefix tests/browser` | Check each screen at 390 px; the test answers `/api/cli` |

`test-support/data/` has an example of each JSON that the page reads. `cargo test -p obc-data`
checks each one against the schema of its command; a new example also needs a line in that test.
