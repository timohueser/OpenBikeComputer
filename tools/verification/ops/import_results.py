"""Import native test reports and retain the exact candidate firmware artifacts."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import urllib.request
import uuid
import xml.etree.ElementTree as ET

ARTIFACT = re.compile(r"(?:rust-(?:test|fixtures)|python-.+|web-.+|desktop-tests-.+|ios-tests-coverage)-[0-9]+")
ASSETS = {"UPDATE.BIN", "manifest.json", "SHA256SUMS.txt", "obc-boot.elf", "obc-fw-nrf54l.elf"}


def junit(path: Path):
    root = ET.parse(path).getroot()
    if root.tag not in {"testsuites", "testsuite"}:
        return
    for case in root.iter("testcase"):
        name = case.get("name", "")
        if not name:
            raise ValueError(f"Unnamed test in {path.name}")
        status, detail = "pass", ""
        for tag, outcome in (("skipped", "skip"), ("failure", "fail"), ("error", "error")):
            node = case.find(tag)
            if node is not None:
                status = outcome
                detail = (node.get("message", "") + "\n" + (node.text or "")).strip()[:20000]
        # Retries must not conceal a failed first execution.
        if case.find("flakyFailure") is not None or case.find("flakyError") is not None:
            status, detail = "fail", "This test passed only after a failed attempt."
        yield case.get("classname", ""), name, status, detail


def swift(path: Path):
    document = json.loads(path.read_text())
    if not isinstance(document.get("testNodes"), list):
        raise ValueError("Missing native Swift testNodes")

    def walk(nodes):
        for node in nodes:
            if node.get("nodeType") == "Test Case":
                identity = node.get("nodeIdentifierURL") or node.get("nodeIdentifier")
                if not identity:
                    raise ValueError("Swift test has no stable identity")
                status = {"Passed": "pass", "Failed": "fail", "Skipped": "skip", "Expected Failure": "skip"}.get(node.get("result"), "error")
                yield identity.rsplit("/", 1)[0], identity.rsplit("/", 1)[-1], status, ""
            else:
                yield from walk(node.get("children", []))
    yield from walk(document["testNodes"])


def read_reports(root: Path):
    cases, results, namespaces, seen = [], [], [], set()
    for directory in sorted(root.iterdir()):
        if not directory.is_dir() or not ARTIFACT.fullmatch(directory.name):
            continue
        namespace = directory.name.rsplit("-", 1)[0]
        namespaces.append(namespace)
        count = 0
        for path in sorted(directory.rglob("*")):
            if path.is_symlink() or not path.is_file():
                continue
            rows = junit(path) if path.suffix == ".xml" else swift(path) if path.name == "tests.json" and namespace == "ios-tests-coverage" else ()
            for classname, name, status, detail in rows:
                identity = f"{namespace}::{classname}::{name}"
                if len(identity) > 1000 or identity in seen:
                    raise ValueError(f"Ambiguous or oversized test identity: {identity[:1000]}")
                seen.add(identity)
                cases.append({"id": identity, "suite": namespace, "name": f"{classname} / {name}"})
                results.append({"caseId": identity, "status": status, **({"detail": detail} if detail else {})})
                count += 1
        if not count:
            raise ValueError(f"No native test results in {directory.name}")
    return cases, results, namespaces


class Client:
    def __init__(self):
        self.base = os.environ["OBC_VERIFICATION_URL"].rstrip("/")
        if not self.base.startswith("https://"):
            raise ValueError("Verification imports require HTTPS")
        self.token = os.environ["OBC_VERIFICATION_CI_TOKEN"]

    def request(self, path, body, content_type="application/json"):
        request = urllib.request.Request(self.base + path, data=body, headers={
            "Authorization": f"Bearer {self.token}", "Content-Type": content_type,
        }, method="POST")
        with urllib.request.urlopen(request, timeout=120) as response:
            return json.load(response)

    def post(self, path, body):
        return self.request(path, json.dumps(body).encode())

    def upload(self, path: Path):
        if path.stat().st_size > 64 * 1024 * 1024:
            raise ValueError(f"Artifact exceeds upload limit: {path.name}")
        data = path.read_bytes()
        boundary = uuid.uuid4().hex
        body = (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{path.name}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode() + data + f"\r\n--{boundary}--\r\n".encode())
        attachment = self.request("/api/files", body, f"multipart/form-data; boundary={boundary}")
        if attachment["sha256"] != hashlib.sha256(data).hexdigest() or attachment["size"] != len(data):
            raise ValueError("Retained artifact digest differs from uploaded bytes")
        return attachment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reports", type=Path, required=True)
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--run-attempt", type=int, required=True)
    parser.add_argument("--candidate")
    parser.add_argument("--conclusion", choices=("success", "failure"), default="success")
    parser.add_argument("--catalog-only", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"[a-f0-9]{40}", args.source_sha) or min(args.run_id, args.run_attempt) < 1:
        parser.error("Invalid source/run identity")
    if not args.catalog_only and (not args.candidate or not re.fullmatch(r"[A-Za-z0-9._-]+", args.candidate)):
        parser.error("--candidate is required for result imports")
    client = Client()
    provenance = {"sourceSha": args.source_sha, "runId": args.run_id, "runAttempt": args.run_attempt}
    try:
        cases, results, namespaces = read_reports(args.reports)
        if args.catalog_only:
            if namespaces:
                client.post("/api/ci/catalog", {**provenance, "cases": cases, "namespaces": namespaces})
            print(f"Imported {len(cases)} catalogue entries from {len(namespaces)} report families.")
            return
        assets = []
        if args.assets and args.assets.is_dir():
            for name in sorted(ASSETS):
                paths = list(args.assets.rglob(name))
                if len(paths) > 1:
                    raise ValueError(f"Ambiguous firmware asset: {name}")
                if paths:
                    if not paths[0].is_file() or paths[0].is_symlink():
                        raise ValueError(f"Invalid firmware asset: {name}")
                    assets.append(client.upload(paths[0]))
        client.post(f"/api/ci/candidates/{args.candidate}/results", {
            **provenance, "conclusion": args.conclusion, "results": results, "assets": assets,
            **({"failure": "One or more required CI or firmware jobs failed."} if args.conclusion == "failure" else {}),
        })
        print(f"Imported {len(results)} results and {len(assets)} retained assets.")
    except Exception as exc:
        if not args.catalog_only:
            client.post(f"/api/ci/candidates/{args.candidate}/results", {
                **provenance, "conclusion": "failure", "results": [], "assets": [],
                "failure": f"Evidence import failed: {type(exc).__name__}",
            })
        raise


if __name__ == "__main__":
    main()
