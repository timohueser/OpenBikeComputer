"""Validate dispatch data before candidate code can reach privileged workflows."""
import json
import os
import re
import subprocess
import urllib.request

candidate_id = os.environ["CANDIDATE_ID"]
source_sha = os.environ["SOURCE_SHA"]
assert re.fullmatch(r"[A-Za-z0-9-]+", candidate_id), "Invalid candidate ID"
assert re.fullmatch(r"[0-9a-f]{40}", source_sha), "Expected full source SHA"
url = os.environ["OBC_VERIFICATION_URL"].rstrip("/")
assert url.startswith("https://"), "Verification service requires HTTPS"
request = urllib.request.Request(
    f"{url}/api/ci/candidates/{candidate_id}",
    headers={"Authorization": f"Bearer {os.environ['OBC_VERIFICATION_CI_TOKEN']}"},
)
with urllib.request.urlopen(request, timeout=30) as response:
    candidate = json.load(response)["candidate"]
assert candidate["sourceSha"] == source_sha, "Candidate source differs from dispatch"
assert candidate["version"] == os.environ["RELEASE_VERSION"], "Candidate version differs from dispatch"
assert candidate["status"] in ("queued", "running", "failed"), "Candidate no longer accepts CI"
subprocess.run(["git", "merge-base", "--is-ancestor", source_sha, "origin/develop"], check=True)
print(f"Validated candidate {candidate_id} at {source_sha}")
