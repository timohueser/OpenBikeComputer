"""Wikimedia dump inputs: resumable downloads and streams of their records."""
from __future__ import annotations

import bz2
from contextlib import contextmanager
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
from urllib.parse import urlparse
from urllib.request import HTTPRedirectHandler, Request, build_opener

from ..landmark_capture import USER_AGENT, write_json

# Only requests to this host carry `OBC_WIKIMEDIA_ENTERPRISE_TOKEN`.
ENTERPRISE = "api.enterprise.wikimedia.com"
RESERVE = 4 * 1024**3
RECORD = 128 * 1024 * 1024
DECOMPRESSORS = {".bz2": ("lbzip2", bz2.open), ".gz": ("pigz", gzip.open), ".tgz": ("pigz", gzip.open)}
TOKEN = re.compile(r"\s*('(?:[^'\\]|\\.)*'|NULL|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[(),;])")
ESCAPES = {"0": "\0", "n": "\n", "r": "\r", "t": "\t", "b": "\b", "Z": "\x1a"}


class Redirects(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, url):
        if not url.startswith("https://"):
            raise ValueError("snapshot redirect must use HTTPS")
        redirected = super().redirect_request(request, response, code, message, headers, url)
        if redirected and urlparse(request.full_url).hostname != urlparse(url).hostname:
            redirected.remove_header("Authorization")
        return redirected


def open_url(request):
    return build_opener(Redirects()).open(request, timeout=60)


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while data := stream.read(1024 * 1024):
            digest.update(data)
    return digest.hexdigest()


def download(source: dict, work: Path) -> tuple[Path, str]:
    """The local file of one input and its SHA-256: its `path`, or its `url` downloaded into
    `work`. A partial download resumes only while the server reports the same length and ETag."""
    if "path" in source:
        path = Path(source["path"]).resolve()
    else:
        url = source["url"]
        if not url.startswith("https://"):
            raise ValueError("snapshot downloads require HTTPS")
        headers = {"User-Agent": USER_AGENT, "Accept-Encoding": "identity"}
        if urlparse(url).hostname == ENTERPRISE:
            token = os.environ.get("OBC_WIKIMEDIA_ENTERPRISE_TOKEN")
            if not token:
                raise ValueError("set OBC_WIKIMEDIA_ENTERPRISE_TOKEN for snapshot downloads")
            headers["Authorization"] = f"Bearer {token}"
        # Resolve the signed storage URL first; the API token never goes to storage.
        with open_url(Request(url, headers=headers, method="HEAD")) as response:
            target = response.url
            length = int(response.headers["Content-Length"])
            etag = response.headers.get("ETag")
        identity = hashlib.sha256(url.encode()).hexdigest()
        work.mkdir(parents=True, exist_ok=True)
        path = work / (identity + source.get("suffix", ".tar.gz"))
        receipt = work / (identity + ".json")
        current = dict(url=url, bytes=length, etag=etag)
        if (json.loads(receipt.read_bytes()) if receipt.exists() else None) != current:
            path.unlink(missing_ok=True)
            write_json(receipt, current)
        offset = path.stat().st_size if path.exists() else 0
        if offset > length:
            raise ValueError("partial snapshot exceeds declared length")
        if offset < length:
            if shutil.disk_usage(work).free < length - offset + RESERVE:
                raise ValueError("snapshot download would consume the disk reserve")
            transfer = {"User-Agent": USER_AGENT, "Accept-Encoding": "identity"}
            if target == url and "Authorization" in headers:
                transfer["Authorization"] = headers["Authorization"]
            if offset:
                transfer["Range"] = f"bytes={offset}-"
            with open_url(Request(target, headers=transfer)) as response:
                if etag and response.headers.get("ETag", etag) != etag:
                    raise ValueError("upstream changed the pinned snapshot during download")
                if offset and (response.status != 206 or response.headers.get("Content-Range") != f"bytes {offset}-{length-1}/{length}"):
                    raise ValueError("upstream did not resume the pinned snapshot")
                with path.open("ab" if offset else "wb") as output:
                    while data := response.read(1024 * 1024):
                        output.write(data)
                    output.flush()
                    os.fsync(output.fileno())
            if path.stat().st_size != length:
                raise ValueError("incomplete snapshot download; run preparation again to resume")
    if not path.is_file():
        raise ValueError(f"snapshot file missing: {path}")
    if source.get("bytes") is not None and source["bytes"] != path.stat().st_size:
        raise ValueError("snapshot length changed")
    digest = sha(path)
    if source.get("sha256") and source["sha256"] != digest:
        raise ValueError("snapshot checksum changed")
    return path, digest


@contextmanager
def opened(path: Path):
    """The decompressed bytes of `path`. lbzip2 and pigz decompress in parallel where present."""
    tool, module = DECOMPRESSORS.get(path.suffix, (None, open))
    if tool and shutil.which(tool):
        process = subprocess.Popen([tool, "-dc", str(path)], stdout=subprocess.PIPE)
        try:
            yield process.stdout
        finally:
            process.stdout.close()
            code = process.wait()
        if code:
            raise ValueError(f"{tool} could not decompress {path.name}")
    else:
        with module(path, "rb") as stream:
            yield stream


def lines(path: Path):
    """Each record line of a JSON array dump, an NDJSON file or the NDJSON members of a tar."""
    with opened(path) as stream:
        if ".tar" in path.suffixes or path.suffix == ".tgz":
            with tarfile.open(fileobj=stream, mode="r|") as archive:
                for member in archive:
                    if not member.isfile() or not member.name.endswith((".ndjson", ".json")):
                        raise ValueError("snapshot archive member is not NDJSON")
                    yield from records(archive.extractfile(member))
        else:
            yield from records(stream)


def records(stream):
    while line := stream.readline(RECORD + 1):
        if len(line) > RECORD:
            raise ValueError("snapshot record exceeds input limit")
        line = line.strip().removesuffix(b",")
        if line not in (b"[", b"]", b""):
            yield line


def tuples(path: Path, table: str, columns: tuple[str, ...]):
    """The named `columns` of each row of one table of a compressed MySQL dump. Columns are found
    by the names that the dump declares, because their order changes between MediaWiki versions."""
    with opened(path) as stream:
        yield from rows(stream, table, columns)


def rows(stream, table: str, columns: tuple[str, ...]):
    create, prefix = f"CREATE TABLE `{table}`".encode(), f"INSERT INTO `{table}` VALUES".encode()
    active, names, positions = False, None, None
    for raw in stream:
        if raw.startswith(create):
            names = []
        elif names is not None and positions is None and (column := re.match(rb"\s+`(\w+)` ", raw)):
            names.append(column[1].decode())
        if raw.startswith(prefix):
            if positions is None:
                if missing := set(columns) - set(names or ()):
                    raise ValueError(f"{table} dump declares no column {', '.join(sorted(missing))}")
                positions = [names.index(column) for column in columns]
            active = True
            raw = raw[len(prefix):]
        elif not active:
            continue
        line = raw.decode().rstrip()
        position, row, end = 0, None, len(line)
        while position < end:
            token = TOKEN.match(line, position)
            if not token:
                raise ValueError(f"invalid {table} dump tuple")
            position = token.end()
            value = token[1]
            if value == "(":
                if row is not None:
                    raise ValueError("nested dump tuple")
                row = []
            elif value == ")":
                if row is None:
                    raise ValueError("unmatched dump tuple")
                yield [row[index] for index in positions]
                row = None
            elif value == ";":
                active = False
            elif value != ",":
                if row is None:
                    raise ValueError("dump value outside tuple")
                if value.startswith("'"):
                    value = re.sub(r"\\(.)", lambda match: ESCAPES.get(match[1], match[1]), value[1:-1])
                elif value == "NULL":
                    value = None
                else:
                    value = float(value) if any(char in value for char in ".eE") else int(value)
                row.append(value)
        if row is not None:
            raise ValueError("incomplete dump tuple")
    if active or names is None:
        raise ValueError(f"incomplete or absent {table} SQL table")
