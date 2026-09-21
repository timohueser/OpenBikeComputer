"""The part of a download only a person can do: `ingest.py wizard <key>`.

Four sources are open data behind an account. No program may click through a login or
read an e-mail, so the wizard does what a runbook would: it names one step at a time,
waits, then checks what landed and hands it to `ingest --input`. It automates nothing
about the login, and it asks for nothing it could work out itself.

The steps live next to the source, in `sources/<key>.py`, because they are facts about
that portal and they go stale with it. This module walks them and checks the delivery;
`cli.py` is what joins the two to `ingest`.
"""

from pathlib import Path

from .lattice import Refuse
from .pool import open_raster
from .sources.base import READABLE


def walk(source, ask, say) -> bool:
    """The portal's steps, one at a time. `False` when the person stopped."""

    say(f"{source.key}: {source.product} — {source.country}")
    say(f"  licence      {source.licence}")
    say(f"  attribution  {source.attribution}")
    say(f"  datum        {source.vertical_datum}")
    if source.credential:
        say(f"  credential   {source.credential.names}")
    for number, step in enumerate(source.steps, 1):
        say(f"\nStep {number} of {len(source.steps)}")
        for line in step.splitlines():
            say(f"  {line}")
        if ask("  Press Enter when this is done, or `q` to stop: ").strip().lower() == "q":
            say("stopped; nothing was ingested")
            return False
    return True


def delivered(directory: Path) -> list[Path]:
    """What the portal left in a directory, as the wizard's check reads it."""

    return sorted(path for path in directory.rglob("*")
                  if path.suffix.lower() in READABLE | {".zip"})


def check(directory: Path, say) -> bool:
    """Whether the files are there and readable, before the ingest is started.

    A zip is not opened here: `ingest --input` opens it, and the check that matters at
    this point is that the download finished and the files are not an error page.
    """

    if not directory.is_dir():
        say(f"  {directory} is not a directory")
        return False
    files = delivered(directory)
    if not files:
        say(f"  {directory} holds no .tif, .asc or .zip")
        return False
    for path in files:
        size = path.stat().st_size
        note = ""
        if path.suffix.lower() != ".zip":
            try:
                with open_raster(path) as src:
                    note = f", {src.width} x {src.height} {src.dtypes[0]}, {src.crs}"
            except Refuse as refusal:
                say(f"  {path.name}: cannot be opened — {refusal}")
                return False
        say(f"  {path.name}: {size / 1e6:.1f} MB{note}")
    return True


def input_directory(source, given, ask, say) -> str | None:
    """Where the delivered files are, once the steps are done and they are readable.

    `None` is "do not ingest": the person stopped, or what is on disk is not what this
    source is delivered as. An empty string is "fetch the service instead", which is what
    a credential that is now set means.
    """

    if source.credential and source.credential.present():
        say(f"\n{source.credential.names} is set, so the service is fetched directly.")
        return ""
    directory = given or ask("\nWhich directory holds the files? ").strip()
    if not directory:
        say("no directory given; nothing was ingested")
        return None
    say(f"\nWhat is in {directory}:")
    if not check(Path(directory), say):
        say("the files are not what this source is delivered as; nothing was ingested")
        return None
    if ask("\nIngest these into the archive? [y/N] ").strip().lower() not in ("y", "yes"):
        say("stopped; nothing was ingested")
        return None
    return directory
