"""The source registry: one module per country, one row per product.

`SOURCES` is the whole registry. A country with no adapter at all is not here; `ingest
--input` takes hand-fetched rasters for those, and `README.md` lists them.
"""

from .base import ManualSource, Source, http_get
from .ch import CH
from .de import DE
from .es import ES
from .fr import FR
from .nl import NL
from .no import NO
from .us import US

SOURCES = {source.key: source for source in (CH, FR, US, NO, ES, NL, *DE)}

__all__ = ["SOURCES", "ManualSource", "Source", "http_get"]
