"""The source registry: one module per country, one row per product.

`SOURCES` is the whole registry. A country with no adapter is not here; `ingest --input`
takes hand-fetched rasters for those, and `README.md` lists them.
"""

from .base import Source, http_get
from .ch import CH

SOURCES = {source.key: source for source in (CH,)}

__all__ = ["SOURCES", "Source", "http_get"]
