# Verification console — server runbook

The console is `apps/obc-verification`. It runs at `https://releases.openbikecomputer.com` on
Debian with Node 24, Caddy and systemd. Builds live in `/opt/obc-verification/releases/`, with
`current` pointing at the active one. Data lives in `/var/lib/obc-verification/` and a deployment
never replaces it. The scripts named below are in `apps/obc-verification/ops/`.

## Install the service

1. Copy `apps/obc-verification/ops/` to the server.
2. Run `sudo bash ops/install.sh`. It creates the users, the directories and the backup timer. It
   does not start the application and it writes no credentials.
3. Create `/etc/obc-verification/service.env`, mode 0600, owned by root, with these values:

   | Variable | Purpose |
   | --- | --- |
   | `ORIGIN` | The exact browser origin, including the port |
   | `VERIFICATION_OWNER_USERNAME` | Local admin fallback account name |
   | `VERIFICATION_OWNER_PASSWORD_HASH` | Initial local admin hash, from `python3 ops/password_hash.py` |
   | `VERIFICATION_CI_TOKEN` | Random bearer token, used only by CI |
   | `GITHUB_REPOSITORY` | `timohueser/OpenBikeComputer` |
   | `GITHUB_TOKEN` | Repository-scoped token: Actions read/write, contents read |
   | `VERIFICATION_SOURCE_BRANCH` | Allowed candidate branch; default `develop` |
   | `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | GitHub OAuth login credentials |

4. Point the subdomain's DNS A record at the VPS and use Cloudflare DNS-only mode. Caddy
   terminates TLS and forwards to `127.0.0.1:3100`.
5. Start the service.

The service does not accept `VERIFICATION_AGENT_TOKEN`. Remove that variable from an existing
configuration.

Caddy replaces incoming forwarded-address headers, and the localhost-only service trusts one proxy
hop for login rate limits. A second proxy needs an explicit trusted-address configuration. Never
enter credentials through an HTTP origin.

The environment file and the SSH private keys are not application assets. Back them up separately
in a private credential store, and rotate the CI and GitHub credentials independently.

## Register the GitHub OAuth app

GitHub is the primary sign-in method. Register an OAuth app in the administrator's GitHub account:

- Homepage: `https://releases.openbikecomputer.com`
- Callback: `https://releases.openbikecomputer.com/auth/callback`
- Device flow: disabled. Keep the default token expiry.

Put the client ID and secret in the environment file and restart the service. The login requests
public identity only. The application stores the GitHub account ID and username, and no profile
email address or access token. The workflow dispatch credential is separate from login.

## Add the first users

1. Sign in with the local admin fallback.
2. Open **Account → Users** and add your own GitHub username with **Admin** selected.
3. Add collaborators by their GitHub usernames.

Approval is tied to GitHub's stable account ID, so a renamed account keeps access and a reused
username does not inherit it. Removing an account ends its sessions and revokes the agent tokens
it issued; it does not remove its historical records. You cannot remove your current account.

The local fallback always has admin access. Its initial hash comes from the environment only when
the database has no local admin hash.

## Recover the local admin password

There is no email reset service. Copy `ops/password_hash.py` to the server and run it as the
application user:

```sh
sudo -u obc-verification python3 /path/to/password_hash.py --reset /var/lib/obc-verification
```

It prompts without echoing and ends local admin sessions. It changes no requirements or evidence.
Database backups contain password hashes and session records; keep them private.

## Deploy

`deploy-verification.yml` tests, builds and deploys on pushes to `develop` that change the
application, and on manual dispatch on `develop`. It packages the build, runtime dependencies,
package metadata and source identity only.

1. Configure the `verification` GitHub environment with secret `OBC_VERIFICATION_DEPLOY_KEY`.
2. Configure repository variables `OBC_VERIFICATION_SSH_HOST`, `OBC_VERIFICATION_KNOWN_HOSTS` and
   `OBC_VERIFICATION_URL`. Copy pinned SSH host keys from a trusted connection; never accept host
   keys dynamically in CI.
3. Give the `obc-verification-deploy` account this `authorized_keys` entry:

   ```text
   restrict,command="sudo -n /usr/local/sbin/obc-verification-deploy" ssh-ed25519 PUBLIC_KEY
   ```

The key accepts an application archive on standard input and can open no interactive shell and
forward no connection. The root-owned command validates archive paths, installs the build,
switches `current` and checks `/health`; a failed health check restores the previous build. The
application itself runs as the unprivileged `obc-verification` user.

**To roll back**, switch `current` to `/opt/obc-verification/previous` and restart
`obc-verification`. A rollback does not roll back data. Take a verified backup before any
deployment that changes the database format. Remove older build directories only after checking
what `current` and `previous` point at.

## Candidate and publication credentials

Configure repository secret `OBC_VERIFICATION_CI_TOKEN` to match the service. Keep firmware
signing and R2 distribution credentials in the `release` GitHub environment: `OBCU_SIGNING_SEED`,
`OBC_R2_ACCOUNT_ID`, `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID` and `OBC_R2_SECRET_ACCESS_KEY`.

The application dispatches `verification-candidate.yml` on `develop`, which verifies the candidate
ID, version, source SHA and source ancestry against the service before it calls the build or test
workflows. `verification-publish.yml` rechecks readiness and file hashes, tags, creates and
publishes the GitHub release, then updates the R2 distribution channel. It never overwrites an
existing version's firmware; a retry accepts existing assets only if their bytes match.

`verification-catalog.yml` imports observed native test identities after a successful `develop`
CI run. `ops/import_results.py` reads native JUnit and Swift result files.

A tag push builds and publishes nothing. `release.yml` is a reusable build workflow only. Keep
repository rules and `release` environment access limited to maintainers.

## Back up and restore

The installer enables a daily backup timer that retains seven successful local snapshots. Run
`sudo /usr/local/sbin/obc-verification-backup` for an extra one. It takes a SQLite snapshot plus
every attachment that snapshot references, into one archive under
`/var/backups/obc-verification/`, without stopping the application.

**Copy these archives to a separate machine or service.** A backup on the same VPS does not
protect against loss of that VPS. Do not use map or fixture bucket credentials for them.

Verify a backup into a new directory before you restore it:

```sh
sudo python3 ops/restore.py /var/backups/obc-verification/ARCHIVE.tar.gz /var/lib/obc-verification-restored
```

The command checks SQLite integrity and each attachment's size and SHA-256. It never replaces live
data. To restore service:

1. Stop `obc-verification`.
2. Keep the current data directory; do not delete it.
3. Move the verified directory into its place.
4. Set its owner to `obc-verification:obc-verification`.
5. Restart the service, then confirm owner login, a saved revision and an attachment download.

## Daily checks

```sh
curl --fail http://127.0.0.1:3100/health
systemctl status obc-verification
systemctl status caddy
journalctl -u obc-verification --since '-30 minutes'
```

Report failed workflow runs in the application. A missing callback stays pending or failed and
must not become a successful candidate. Retry a failed publication from the existing candidate and
keep its evidence and firmware. Prepare a new candidate when the source or the required evidence
definitions change; manual results do not carry to another candidate.
