"""A product indexed by STAC: the items of a collection, or the answer to a box search.

Lower Saxony publishes cloud-optimised GeoTIFFs whose S3 prefix is the delivery batch the
tile came in, not its position, so the URL cannot be built from coordinates: the search is
the index. The files are one square kilometre each, so they are downloaded whole.

`stac_items` is shared with Switzerland, because both servers page and both cap a page.
"""

import json

from ..lattice import Refuse
from .base import http_get
from .bulk import BulkSource


def stac_items(url: str):
    """Every item a STAC answer holds, following `rel: next` to the end.

    Every server caps a page — LGLN's caps at 1000 — so a box that needs more squares than
    the cap comes back short, and silently, unless the pages are followed. The number of
    pages is bounded by the box. A `next` link that points back at a page already read is
    refused rather than followed for ever.
    """

    seen = set()
    while url:
        if url in seen:
            raise Refuse(f"{url}: the STAC `next` link points at a page already read")
        seen.add(url)
        page = json.loads(http_get(url))
        if "features" not in page:
            raise Refuse(f"{url}: not a STAC answer: {str(page)[:200]}")
        yield from page["features"]
        url = next((link["href"] for link in page.get("links", [])
                    if link.get("rel") == "next"), None)


class StacSearchSource(BulkSource):
    """`/search?bbox=` against a STAC API, taking one named asset per item."""

    def __init__(self, *args, search, asset, collections=None, page=500, **kw):
        super().__init__(*args, **kw)
        self.search, self.asset, self.collections, self.page = search, asset, collections, page

    def files(self, bbox):
        query = f"{self.search}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}&limit={self.page}"
        if self.collections:
            query += f"&collections={self.collections}"
        wanted = []
        for feature in stac_items(query):
            asset = feature.get("assets", {}).get(self.asset)
            if asset is None:
                raise Refuse(f"{self.key}: item {feature.get('id')} has no `{self.asset}` asset")
            href = asset["href"]
            wanted.append((href.rsplit("/", 1)[-1], href))
        return sorted(set(wanted))
