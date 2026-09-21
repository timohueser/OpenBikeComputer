"""The source registry: one module per country, one row per product.

`SOURCES` is the whole registry, and adding a country is a module here plus a row in it.
Every row records its licence, its attribution and its vertical datum, whether or not it
can be fetched: a row behind an account states the credential it wants and the steps
`wizard` walks, and `ingest --input` takes the files the portal delivered. `README.md`
records every probe, including the ones that failed.
"""

from .at import AT
from .au import AU
from .base import Credential, ManualSource, Source, http_get
from .ca import CA
from .ch import CH
from .de import DE
from .dk import DK
from .es import ES
from .fi import FI
from .fr import FR
from .it import IT
from .nl import NL
from .no import NO
from .nz import NZ
from .se import SE
from .uk import UK
from .us import US

SOURCES = {source.key: source for source in
           (CH, FR, US, NO, ES, NL, UK, AT, CA, NZ, DK, SE, FI, AU, *DE, *IT)}

__all__ = ["SOURCES", "Credential", "ManualSource", "Source", "http_get"]
