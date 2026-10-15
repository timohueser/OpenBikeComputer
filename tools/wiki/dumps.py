"""Streams of the records of the local Wikimedia dump files."""
from __future__ import annotations

import bz2
from contextlib import contextmanager
import gzip
import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import tarfile

RESERVE = 4 * 1024**3
RECORD = 128 * 1024 * 1024
# Inputs are store objects without a suffix, so the first bytes name their format.
COMPRESSIONS = ((b"BZh", "lbzip2", bz2.open), (b"\x1f\x8b", "pigz", gzip.open))
TOKEN = re.compile(r"\s*('(?:[^'\\]|\\.)*'|NULL|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[(),;])")
ESCAPES = {"0": "\0", "n": "\n", "r": "\r", "t": "\t", "b": "\b", "Z": "\x1a"}


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while data := stream.read(1024 * 1024):
            digest.update(data)
    return digest.hexdigest()


def tar(head: bytes) -> bool:
    return head[257:262] == b"ustar"


def compression(path: Path):
    """The parallel tool and the Python module that decompress a bz2, gzip or plain tar file."""
    with path.open("rb") as stream:
        head = stream.read(262)
    for magic, tool, module in COMPRESSIONS:
        if head.startswith(magic):
            return tool, module
    if tar(head):
        return None, open
    raise ValueError(f"{path.name} is not a bz2, gzip or tar file")


@contextmanager
def opened(path: Path):
    """The decompressed bytes of `path`. lbzip2 and pigz decompress in parallel where present."""
    tool, module = compression(path)
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
    with compression(path)[1](path, "rb") as stream:
        archive = tar(stream.read(262))
    with opened(path) as stream:
        if archive:
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
        # MediaWiki cuts binary title columns at 255 bytes, also inside a UTF-8 character.
        line = raw.decode(errors="replace").rstrip()
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
