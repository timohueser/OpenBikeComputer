"""The source registry: one module per country, one row per product.

`SOURCES` is the whole registry, and adding a country is a module here plus a row in it.
A country with no keyless access at all is not here; `ingest --input` takes hand-fetched
rasters for those, and `README.md` records every probe, including the ones that failed.
"""

from .at import AT
from .base import ManualSource, Source, http_get
from .ca import CA
from .ch import CH
from .de import DE
from .es import ES
from .fr import FR
from .it import IT
from .nl import NL
from .no import NO
from .nz import NZ
from .uk import UK
from .us import US

SOURCES = {source.key: source for source in (CH, FR, US, NO, ES, NL, UK, AT, CA, NZ, *DE, *IT)}

__all__ = ["SOURCES", "ManualSource", "Source", "http_get"]
