"""The capture of a prepackaged file of the Copernicus Land Monitoring Service (CLMS), as
specs/obc-data.md (Fetch) says: the service key signs a grant for an access token, the token
orders the file, and the finished order links a zip archive. The program takes
one member of that archive, which can also be inside a zip archive in it. A zip of the file from
the archive drive (`--archive`) replaces the order, and needs no key.

Standard library only. No key, grant or token goes to argv, a URL, a log or an error.
"""

import argparse
import base64
import hashlib
import http.client
import json
import os
import sys
import time
import zipfile
from pathlib import Path, PurePosixPath
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import HTTPRedirectHandler, Request, build_opener, urlopen

KEY = Path("~/.config/openbikecomputer/clms-key.json")
API = "https://land.copernicus.eu/api"
GRANT_TYPE = "urn:ietf:params:oauth:grant-type:jwt-bearer"
# A token lasts an hour; the service allows a grant of at most an hour.
TOKEN_S = 1800
TRIES = 6
POLL_S = 30
WAIT_S = 6 * 3600
WAITING = ("Queued", "In_progress")
# The DER DigestInfo of SHA-256 before the digest: RFC 8017, section 9.2, note 1.
SHA256_INFO = bytes.fromhex("3031300d060960864801650304020105000420")


def integers(der):
    """The INTEGERs of a DER SEQUENCE, as of a PKCS #1 RSAPrivateKey."""

    def item(at):
        tag, length, at = der[at], der[at + 1], at + 2
        if length & 0x80:
            count = length & 0x7F
            length, at = int.from_bytes(der[at:at + count], "big"), at + count
        return tag, der[at:at + length], at + length

    tag, body, _ = item(0)
    if tag != 0x30:
        raise ValueError("the key is not a DER SEQUENCE")
    der, at, out = body, 0, []
    while at < len(der):
        tag, value, at = item(at)
        if tag != 0x02:
            raise ValueError("the key holds more than INTEGERs")
        out.append(int.from_bytes(value, "big"))
    return out


def sign(pem, message):
    """The RS256 signature of `message`: RSASSA-PKCS1-v1_5 with SHA-256, RFC 8017, by the PKCS #1
    key `pem`. A local signature with our own key, so the time it takes reveals nothing new."""
    lines = pem.strip().splitlines()
    if lines[0] != "-----BEGIN RSA PRIVATE KEY-----":
        raise ValueError("the private key of the service key is not a PKCS #1 RSA key")
    _, n, _, d = integers(base64.b64decode("".join(lines[1:-1])))[:4]
    size = (n.bit_length() + 7) // 8
    info = SHA256_INFO + hashlib.sha256(message).digest()
    encoded = b"\x00\x01" + b"\xff" * (size - len(info) - 3) + b"\x00" + info
    return pow(int.from_bytes(encoded, "big"), d, n).to_bytes(size, "big")


def b64url(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=")


def grant(key, now):
    """The signed JWT that asks the `token_uri` of the service key for an access token."""
    claims = {"iss": key["client_id"], "sub": key["user_id"], "aud": key["token_uri"], "iat": now, "exp": now + TOKEN_S}
    signing = b".".join(b64url(json.dumps(part, separators=(",", ":")).encode()) for part in ({"alg": "RS256", "typ": "JWT"}, claims))
    return (signing + b"." + b64url(sign(key["private_key"], signing))).decode()


class Unredirected(HTTPRedirectHandler):
    """Fails on a redirect, which urllib follows with the Authorization header, also to another host,
    and after which it sends a POST as a GET."""

    def redirect_request(self, *_):
        return None


# The opener of the calls to the API and to the token endpoint.
API_OPENER = build_opener(Unredirected)


def said(error):
    """`: TEXT` of the message fields of a JSON error answer. Only these fields: another part of the
    body could hold a grant."""
    try:
        body = json.loads(error.read()) if isinstance(error, HTTPError) else None
    except (OSError, ValueError, http.client.IncompleteRead):
        return ""
    fields = ("msg", "message", "error", "error_description")
    text = [str(body[field]) for field in fields if isinstance(body, dict) and field in body]
    return f": {'; '.join(text)}" if text else ""


def call(request, read=lambda response: response.read(), opener=None):
    """`read` of the answer to `request`, by default its body. A failed or cut transfer and a 5xx or
    429 answer are tried again, TRIES in all; another 4xx answer, or a redirect of `opener`, fails at
    once."""
    request.add_header("User-Agent", "OpenBikeComputer/1.0")
    for attempt in range(TRIES):
        try:
            with (opener.open if opener else urlopen)(request, timeout=300) as response:
                return read(response)
        except (OSError, http.client.HTTPException) as error:
            if isinstance(error, HTTPError) and error.code < 500 and error.code != 429 or attempt == TRIES - 1:
                raise RuntimeError(f"{request.full_url}: {error}{said(error)}") from error
            print(f"{request.full_url}: {error}; again in {2 << attempt} s", file=sys.stderr, flush=True)
            time.sleep(2 << attempt)


class Session:
    """Calls to the API with an access token, which it renews after TOKEN_S."""

    def __init__(self, key):
        self.key, self.token, self.since = key, None, 0.0

    def json(self, path, body=None):
        if self.token is None or time.monotonic() - self.since > TOKEN_S:
            form = urlencode({"grant_type": GRANT_TYPE, "assertion": grant(self.key, int(time.time()))}).encode()
            answer = call(Request(self.key["token_uri"], data=form, headers={
                "Accept": "application/json", "Content-Type": "application/x-www-form-urlencoded"}), opener=API_OPENER)
            self.token, self.since = json.loads(answer)["access_token"], time.monotonic()
        headers = {"Accept": "application/json", "Authorization": f"Bearer {self.token}"}
        if body is not None:
            headers["Content-Type"] = "application/json"
        data = None if body is None else json.dumps(body).encode()
        return json.loads(call(Request(f"{API}/{path}", data=data, headers=headers), opener=API_OPENER))


def order(session, dataset, file):
    """The download link of the prepackaged `file` of `dataset`, once its order is finished."""
    answer = session.json("@datarequest_post", {"Datasets": [{"DatasetID": dataset, "FileID": file}]})
    if answer.get("ErrorTaskIds") or not answer.get("TaskIds"):
        raise RuntimeError(f"CLMS refused the order: {answer}")
    task = answer["TaskIds"][0]["TaskID"]
    deadline = time.monotonic() + WAIT_S
    while True:
        status = session.json(f"@datarequest_status_get?TaskID={task}")
        state = status.get("Status")
        if state == "Finished_ok":
            url = status.get("DownloadURL")
            if not (isinstance(url, str) and url.startswith("https://")):
                raise RuntimeError(f"CLMS order {task} is finished, but links no https URL: {url!r}")
            return url
        if state not in WAITING:
            raise RuntimeError(f"CLMS order {task} is {state}: {status.get('Message')}")
        if time.monotonic() > deadline:
            raise RuntimeError(f"CLMS order {task} is still {state} after {WAIT_S // 3600} h")
        print(f"CLMS order {task}: {state}", file=sys.stderr, flush=True)
        time.sleep(POLL_S)


def length(headers):
    """The `Content-Length` of an answer without `Transfer-Encoding`; None for none, or one that is
    not a number."""
    value = headers.get("Content-Length")
    if "Transfer-Encoding" in headers or value is None or not value.strip().isdigit():
        return None
    return int(value)


def whole(target, stream, size=None):
    """Write `stream` to `target` whole and on disk, so a resumed capture keeps only whole files. A
    stream that ends before `size` bytes raises HTTPException, which `call` tries again."""
    part, written = target.with_name(target.name + ".part"), 0
    with open(part, "wb") as out:
        while chunk := stream.read(1 << 20):
            out.write(chunk)
            written += len(chunk)
        out.flush()
        os.fsync(out.fileno())
    if size is not None and written != size:
        raise http.client.HTTPException(f"{target.name}: {written} of {size} bytes")
    part.replace(target)
    print(f"{target.name}: {target.stat().st_size} bytes", file=sys.stderr, flush=True)


def extract(archive, member, target, work):
    """Write the file named `member` of the zip `archive`, or of a zip archive in it, to `target`.
    False when no archive has it."""
    with zipfile.ZipFile(archive) as z:
        names = z.namelist()
        for name in names:
            if PurePosixPath(name).name == member:
                with z.open(name) as stream:
                    whole(target, stream)
                return True
        for name in names:
            if name.endswith(".zip"):
                inner = work / PurePosixPath(name).name
                with z.open(name) as stream:
                    whole(inner, stream)
                if extract(inner, member, target, work):
                    return True
    return False


def fetch(dataset, file, member, work, out, key=KEY, kept=None):
    """The member of the archive of the prepackaged file into `out`: of the zip `kept`, which the
    caller checked, or else of an order. A downloaded archive stays in `work`, so a run that fails
    after the download resumes without a new order."""
    target, archive = out / member, kept or work / "order.zip"
    if target.exists():
        return
    if not kept and not archive.exists():
        url = order(Session(json.loads(key.expanduser().read_text())), dataset, file)
        # Without the token: the link names the order itself.
        call(Request(url), lambda response: whole(archive, response, length(response.headers)))
    try:
        found = extract(archive, member, target, work)
    except zipfile.BadZipFile:
        # The next run downloads it again.
        if not kept:
            archive.unlink()
        raise
    if not found:
        raise RuntimeError(f"no archive of the order of {file} has {member}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dataset", required=True, help="the UID of the dataset")
    parser.add_argument("--file", required=True, help="the @id of the prepackaged file")
    parser.add_argument("--member", required=True, help="the name of the file to take from the archive")
    parser.add_argument("--work", type=Path, required=True, help="the directory that keeps the archive")
    parser.add_argument("--fetch", type=Path, required=True, help="the directory of the captured file")
    parser.add_argument("--archive", type=Path, help="the zip of the file on the archive drive, instead of an order")
    args = parser.parse_args(argv)
    fetch(args.dataset, args.file, args.member, args.work, args.fetch, kept=args.archive)


if __name__ == "__main__":
    main()
