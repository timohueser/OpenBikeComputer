# Verification server

The console runs at `https://releases.openbikecomputer.com` on Debian with Node 24, Caddy and
systemd. Builds live in `/opt/obc-verification/releases/`; `current` points at the active build.
Data lives in `/var/lib/obc-verification/`. Deployments do not replace data.
The commands below use the copied `ops/` directory.

## Install

1. Copy `tools/verification/ops/` to the server.
2. Run `sudo bash ops/install.sh`. It creates users, directories and the daily backup timer,
   but starts no application and writes no credentials.
3. Create `/etc/obc-verification/service.env`, owned by root with mode 0600:

   | Variable | Value or purpose |
   | --- | --- |
   | `ORIGIN` | Exact browser origin, including port |
   | `VERIFICATION_OWNER_USERNAME` | Local admin fallback name |
   | `VERIFICATION_OWNER_PASSWORD_HASH` | Initial hash from `python3 ops/password_hash.py` |
   | `VERIFICATION_CI_TOKEN` | Random bearer token for CI only |
   | `GITHUB_REPOSITORY` | `timohueser/OpenBikeComputer` |
   | `GITHUB_TOKEN` | Repository token with Actions read/write and contents read |
   | `VERIFICATION_SOURCE_BRANCH` | Allowed candidate branch; default `develop` |
   | `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | GitHub OAuth credentials |

4. Point the subdomain's DNS A record at the VPS. Use Cloudflare DNS-only mode.
   Caddy terminates TLS and forwards to `127.0.0.1:3100`.
5. Start the service.

Remove `VERIFICATION_AGENT_TOKEN` from existing configurations; the service does not accept it.
Caddy replaces incoming forwarded-address headers. The localhost-only service trusts one proxy
hop for login rate limits. Configure trusted addresses explicitly for a second proxy.
Never enter credentials through HTTP.

Back up environment files and SSH keys in a private credential store.
Rotate CI and GitHub credentials independently.

## Logs

Keep the privacy notice's promise of no visitor logs. Put this global block at the top of
`/etc/caddy/Caddyfile` to omit access logs and client and request addresses from errors:

```caddyfile
{
    log {
        level WARN
        format filter {
            wrap json
            fields {
                request delete
            }
        }
    }
}
```

Never add a Caddy `log` directive to a site block.
In `/etc/systemd/journald.conf.d/`, add a `[Journal]` drop-in that sorts last with
`MaxFileSec=1day` and `MaxRetentionSec=7day`. Daily rotation lets Journald remove whole files
within seven days.

## Login and users

Register an OAuth app in the administrator's GitHub account:

- Homepage: `https://releases.openbikecomputer.com`
- Callback: `https://releases.openbikecomputer.com/auth/callback`
- Disable device flow; keep the default token expiry.

Set the client ID and secret in the environment file and restart.
Login requests public identity only. The console stores account ID and username, without
profile email or access token. Workflow dispatch uses separate credentials.

Sign in with the local admin fallback. In **Account → Users**, add your GitHub username as
**Admin**, then add collaborators. Access follows GitHub account IDs across renames.
Removing a user ends sessions and revokes their agent tokens, but retains history.
You cannot remove your current account.

The fallback always has admin access. The environment hash initializes a missing local admin hash.
There is no email password reset. Copy `ops/password_hash.py` to the server and run as the
application user:

```sh
sudo -u obc-verification python3 /path/to/password_hash.py --reset /var/lib/obc-verification
```

It prompts without echo, ends local admin sessions and leaves requirements and evidence intact.
Keep database backups private: they contain password hashes and sessions.

## Deploy and roll back

`deploy-verification.yml` tests, builds and deploys application changes on `develop`, or manual
dispatch on that branch. Packages contain the build, runtime dependencies, metadata and source identity.

1. Add secret `OBC_VERIFICATION_DEPLOY_KEY` to the `verification` GitHub environment.
2. Set repository variables `OBC_VERIFICATION_SSH_HOST`, `OBC_VERIFICATION_KNOWN_HOSTS` and
   `OBC_VERIFICATION_URL`. Pin SSH host keys from a trusted connection; never accept them dynamically.
3. Set this `authorized_keys` entry for `obc-verification-deploy`:

   ```text
   restrict,command="sudo -n /usr/local/sbin/obc-verification-deploy" ssh-ed25519 PUBLIC_KEY
   ```

The key accepts an application archive on standard input, with no shell or forwarding.
The root-owned command validates paths, installs the build, switches `current` and checks `/health`.
A failed check restores the previous build. The application runs as the unprivileged
`obc-verification` user.

To roll back, point `current` at `/opt/obc-verification/previous` and restart `obc-verification`.
Data does not roll back. Verify a backup before deployments that change the database format.
Remove old builds only after checking `current` and `previous`.

## Candidate and publication credentials

Set repository secret `OBC_VERIFICATION_CI_TOKEN` to match the service.
Keep these secrets in the `release` GitHub environment:
`OBCU_SIGNING_SEED`, `OBC_R2_ACCOUNT_ID`, `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID` and
`OBC_R2_SECRET_ACCESS_KEY`.

The console dispatches `verification-candidate.yml` on `develop`. It validates candidate ID,
version, source SHA and ancestry against the service before builds or tests.
`verification-publish.yml` rechecks readiness and hashes, tags and publishes the GitHub release,
then updates the R2 channel. It never overwrites versioned firmware; retries require identical bytes.

After successful `develop` CI, `verification-catalog.yml` imports native test identities.
`ops/import_results.py` reads JUnit and Swift results.
Tag pushes publish nothing; `release.yml` only supplies reusable builds.
Limit repository rules and `release` environment access to maintainers.

## Back up and restore

The daily timer retains seven successful local snapshots. Run
`sudo /usr/local/sbin/obc-verification-backup` for an extra backup.
It archives a SQLite snapshot and its referenced attachments under
`/var/backups/obc-verification/`, without stopping the application.
Copy archives to another machine or service; local backups cannot protect against VPS loss.
Use separate credentials from map and fixture buckets.

Verify an archive into a new directory:

```sh
sudo python3 ops/restore.py /var/backups/obc-verification/ARCHIVE.tar.gz /var/lib/obc-verification-restored
```

It checks SQLite integrity and attachment sizes and SHA-256 hashes, without replacing live data.

1. Stop `obc-verification`.
2. Keep the current data directory.
3. Move the verified directory into its place.
4. Set owner to `obc-verification:obc-verification`.
5. Restart. Check owner login, a saved revision and an attachment download.

## Daily checks

```sh
curl --fail http://127.0.0.1:3100/health
systemctl status obc-verification
systemctl status caddy
journalctl -u obc-verification --since '-30 minutes'
```

Report failed workflows in the console. Missing callbacks remain pending or failed.
Retry publication from the same candidate, with its evidence and firmware.
Create a new candidate when source or required evidence definitions change.
Manual results do not carry between candidates.
