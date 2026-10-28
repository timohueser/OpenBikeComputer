"""The source registry: one module per country, one row per product.

`SOURCES` is the whole registry, and adding a country is a module here plus a row in it.
Every row records its licence, its attribution and its vertical datum, whether or not it
can be fetched: a row behind an account states the credential it wants, and a row ordered
by hand reads the delivery that the environment names.
"""

from .at import AT
from .au import AU
from .base import Credential, ManualSource, Source, http_get
from .ca import CA
from .ch import CH
from .cz import CZ
from .de import DE
from .dk import DK
from .es import ES
from .fi import FI
from .fr import FR
from .gb import GB
from .is_ import IS
from .it import IT
from .nl import NL
from .no import NO
from .nz import NZ
from .pl import PL
from .se import SE
from .sk import SK
from .us import US

SOURCES = {source.key: source for source in
           (CH, FR, US, NO, ES, NL, *GB, AT, CA, NZ, DK, SE, FI, AU, PL, CZ, SK, IS, *DE, *IT)}

__all__ = ["SOURCES", "Credential", "ManualSource", "Source", "http_get"]
