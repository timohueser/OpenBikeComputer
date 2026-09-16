"""Promote immutable, verified candidate bytes without executing candidate source."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.error
import urllib.request


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def release_notes(candidate, repo):
    version, source = candidate["version"], candidate["sourceSha"]
    exceptions = candidate.get("exceptions", [])
    excluded = [r for r in candidate.get("revision", {}).get("requirements", []) if not r["active"]]
    limitations = " and ".join(label for label, present in [("exceptions", exceptions), ("exclusions", excluded)] if present)
    outcome = f"Candidate accepted with requirement {limitations}" if limitations else "Verified candidate"
    notice = ""
    if exceptions:
        ids = ", ".join(f"`{item['requirementId']}`" for item in exceptions)
        notice = (f"**Accepted requirement exceptions: {len(exceptions)}.** {ids}. "
                  "These requirements are not counted as verified. Reasons, approvers, and original test outcomes "
                  "are recorded in the attached `verification-report.html` and `verification-evidence.json`.\n\n")
    if excluded:
        ids = ", ".join(f"`{item['id']}`" for item in excluded)
        notice += (f"**Excluded requirements: {len(excluded)}.** {ids}. "
                   "These requirements are outside verification for this release and are not counted as verified. "
                   "See the attached verification report for their definitions and labels.\n\n")
    return (
        f"{outcome} `{candidate['id']}` at `{source}`.\n\n" + notice +
        f"**Source and licences.** Firmware is licensed under [GPL-3.0](https://github.com/{repo}/blob/{version}/LICENSE). "
        f"[Complete corresponding source](https://github.com/{repo}/tree/{version}) and "
        f"[third-party licences](https://github.com/{repo}/blob/{version}/THIRD-PARTY.md) apply to these files and their copies at updates.openbikecomputer.com.\n"
    )


def main():
    candidate_id = os.environ["CANDIDATE_ID"]
    assert re.fullmatch(r"[A-Za-z0-9-]+", candidate_id), "Invalid candidate ID"
    base = os.environ["OBC_VERIFICATION_URL"].rstrip("/")
    assert base.startswith("https://"), "Verification service requires HTTPS"
    headers = {"Authorization": f"Bearer {os.environ['OBC_VERIFICATION_CI_TOKEN']}"}

    def fetch(path):
        request = urllib.request.Request(base + path, headers=headers)
        with urllib.request.urlopen(request, timeout=120) as response:
            return response.read()

    snapshot = json.loads(fetch(f"/api/ci/candidates/{candidate_id}"))
    candidate = snapshot["candidate"]
    assert snapshot["readiness"]["ready"], "Candidate evidence is incomplete"
    assert candidate["status"] == "publishing", "Publication must be requested by an owner"
    version, source = candidate["version"], candidate["sourceSha"]
    assert re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?", version)
    assert re.fullmatch(r"[0-9a-f]{40}", source)
    repo = os.environ["GH_REPO"]
    required = {"UPDATE.BIN", "manifest.json", "SHA256SUMS.txt", "obc-boot.elf", "obc-fw-nrf54l.elf"}
    assets = {asset["name"]: asset for asset in candidate["assets"]}
    assert len(assets) == len(candidate["assets"]), "Duplicate release filenames"
    assert set(assets) == required, "Unexpected or missing firmware assets"
    # Check distribution configuration before creating any public state.
    for key in ("OBC_R2_ACCOUNT_ID", "OBC_R2_BUCKET", "OBC_R2_ACCESS_KEY_ID", "OBC_R2_SECRET_ACCESS_KEY"):
        assert os.environ.get(key), f"Missing {key}"
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        for name, asset in assets.items():
            assert re.fullmatch(r"[A-Za-z0-9-]+", asset["id"])
            content = fetch(f"/api/files/{asset['id']}")
            assert len(content) == asset["size"], f"Size mismatch: {name}"
            assert hashlib.sha256(content).hexdigest() == asset["sha256"], f"Checksum mismatch: {name}"
            (directory / name).write_bytes(content)
        manifest = json.loads((directory / "manifest.json").read_text())
        assert manifest["version"] == version
        assert manifest["sha256"] == assets["UPDATE.BIN"]["sha256"]
        assert manifest["bytes"] == assets["UPDATE.BIN"]["size"]
        # Check the build's checksum manifest without allowing arbitrary file paths.
        checksums = {}
        for row in (directory / "SHA256SUMS.txt").read_text().splitlines():
            digest, name = row.split(maxsplit=1)
            name = name.lstrip("*")
            assert name in required and name != "SHA256SUMS.txt"
            assert digest == assets[name]["sha256"], f"Build checksum mismatch: {name}"
            checksums[name] = digest
        assert {"UPDATE.BIN", "obc-boot.elf", "obc-fw-nrf54l.elf"} <= checksums.keys()
        (directory / "verification-report.html").write_bytes(fetch(f"/api/candidates/{candidate_id}/report"))
        (directory / "verification-evidence.json").write_bytes(fetch(f"/api/candidates/{candidate_id}/evidence"))
        release_url = f"https://github.com/{repo}/releases/tag/{version}"
        # Existing tags must refer to the exact candidate; never force-move a tag.
        response = subprocess.run(["gh", "api", f"repos/{repo}/git/ref/tags/{version}"], capture_output=True, text=True)
        if response.returncode == 0:
            reference = json.loads(response.stdout)["object"]
            while reference["type"] == "tag":
                reference = json.loads(command("gh", "api", f"repos/{repo}/git/tags/{reference['sha']}"))["object"]
            assert reference["type"] == "commit" and reference["sha"] == source, "Release tag belongs to another commit"
        else:
            # Creation also fails safely if the earlier read failed for reasons other than absence.
            command("gh", "api", "--method", "POST", f"repos/{repo}/git/refs", "-f", f"ref=refs/tags/{version}", "-f", f"sha={source}")
        existing = subprocess.run(["gh", "release", "view", version, "--json", "isDraft,assets"], capture_output=True, text=True)
        prerelease = "-" in version.split("+", 1)[0]
        if existing.returncode != 0:
            notes = directory / "notes.md"
            notes.write_text(release_notes(candidate, repo))
            args = ["gh", "release", "create", version, "--verify-tag", "--draft", "--title", version, "--notes-file", str(notes), "--generate-notes"]
            if prerelease:
                args.append("--prerelease")
            command(*args)
        existing = json.loads(command("gh", "release", "view", version, "--json", "isDraft,assets"))
        existing_assets = {asset["name"]: asset for asset in existing["assets"]}
        for name in sorted(required | {"verification-report.html", "verification-evidence.json"}):
            path = directory / name
            if name in existing_assets:
                old = directory / "existing" / name
                old.parent.mkdir(exist_ok=True)
                command("gh", "release", "download", version, "--pattern", name, "--dir", str(old.parent))
                assert old.read_bytes() == path.read_bytes(), f"Release already contains different {name}"
            else:
                assert existing["isDraft"], f"Published release is missing {name}; refusing mutation"
                command("gh", "release", "upload", version, str(path))
        if existing["isDraft"]:
            command("gh", "release", "edit", version, "--draft=false")
        remote = dict(os.environ,
            RCLONE_CONFIG_OBCR2_TYPE="s3", RCLONE_CONFIG_OBCR2_PROVIDER="Cloudflare",
            RCLONE_CONFIG_OBCR2_REGION="auto", RCLONE_CONFIG_OBCR2_NO_CHECK_BUCKET="true",
            RCLONE_CONFIG_OBCR2_ENDPOINT=f"https://{os.environ['OBC_R2_ACCOUNT_ID']}.r2.cloudflarestorage.com",
            RCLONE_CONFIG_OBCR2_ACCESS_KEY_ID=os.environ["OBC_R2_ACCESS_KEY_ID"],
            RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY=os.environ["OBC_R2_SECRET_ACCESS_KEY"])
        prefix = f"obcr2:{os.environ['OBC_R2_BUCKET']}/fw"
        subprocess.run(["rclone", "copyto", "--checksum", "--immutable", str(directory / "UPDATE.BIN"), f"{prefix}/{version}/UPDATE.BIN"], env=remote, check=True)
        channel = "prerelease/" if prerelease else ""
        subprocess.run(["rclone", "copyto", str(directory / "manifest.json"), f"{prefix}/{channel}manifest.json"], env=remote, check=True)
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"source_sha={source}\nrelease_url={release_url}\n")


if __name__ == "__main__":
    main()
