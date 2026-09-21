"""A product indexed by a STAC API, searched by box.

Lower Saxony publishes cloud-optimised GeoTIFFs whose S3 prefix is the delivery batch the
tile came in, not its position, so the URL cannot be built from coordinates: the search is
the index. The files are one square kilometre each, so they are downloaded whole.
"""

import json

from ..lattice import Refuse
from .base import http_get
from .bulk import BulkSource


class StacSearchSource(BulkSource):
    """`/search?bbox=` against a STAC API, taking one named asset per item."""

    def __init__(self, *args, search, asset, collections=None, page=500, **kw):
        super().__init__(*args, **kw)
        self.search, self.asset, self.collections, self.page = search, asset, collections, page

    def files(self, bbox):
        query = f"{self.search}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}&limit={self.page}"
        if self.collections:
            query += f"&collections={self.collections}"
        answer = json.loads(http_get(query))
        if "features" not in answer:
            raise Refuse(f"{self.key}: {self.search} did not answer a STAC search: {str(answer)[:200]}")
        wanted = []
        for feature in answer["features"]:
            asset = feature.get("assets", {}).get(self.asset)
            if asset is None:
                raise Refuse(f"{self.key}: item {feature.get('id')} has no `{self.asset}` asset")
            href = asset["href"]
            wanted.append((href.rsplit("/", 1)[-1], href))
        return sorted(set(wanted))
