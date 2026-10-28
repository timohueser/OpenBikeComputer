"""A product indexed by STAC: the items of a collection, or the answer to a box search.

Lower Saxony publishes cloud-optimised GeoTIFFs whose S3 prefix is the delivery batch the
tile came in, not its position, so the URL cannot be built from coordinates: the search is
the index. The files are one square kilometre each, so they are downloaded whole.

`stac_items` is shared with Switzerland, because both servers page and both cap a page.
"""

import json

from ..lattice import Refuse
from .base import http_get, redact
from .bulk import BulkSource


def stac_items(url: str, body: dict | None = None):
    """Every item a STAC answer holds, following `rel: next` to the end.

    Every server caps a page — LGLN's caps at 1000 — so a box that needs more squares than
    the cap comes back short, and silently, unless the pages are followed. The number of
    pages is bounded by the box. A `next` link that points back at a page already read is
    refused rather than followed for ever.

    A search sent as a POST of `body` pages by a `next` link with `method: POST`, whose own
    `body` carries the token of the next page.
    """

    seen = set()
    while url:
        sent = None if body is None else json.dumps(body, sort_keys=True)
        if (url, sent) in seen:
            raise Refuse(f"{url}: the STAC `next` link points at a page already read")
        seen.add((url, sent))
        posted = {"data": sent.encode(), "headers": {"Content-Type": "application/json"}} if sent else {}
        body_bytes = http_get(url, **posted)
        try:
            page = json.loads(body_bytes)
        except ValueError:
            page = body_bytes[:200].decode("utf-8", "replace")
        if not isinstance(page, dict) or "features" not in page:
            raise Refuse(redact(f"{url}: not a STAC answer: {str(page)[:200]}"))
        yield from page["features"]
        link = next((link for link in page.get("links", []) if link.get("rel") == "next"), {})
        url, body = link.get("href"), (link.get("body") if link.get("method") == "POST" else None)


class StacSearchSource(BulkSource):
    """`/search?bbox=` against a STAC API, taking one named asset per item."""

    def __init__(self, *args, search, asset, collections=None, page=500, **kw):
        super().__init__(*args, **kw)
        self.search, self.asset, self.collections, self.page = search, asset, collections, page

    def items(self, bbox):
        """The items of the search for `bbox`."""

        query = f"{self.search}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}&limit={self.page}"
        if self.collections:
            query += f"&collections={self.collections}"
        return stac_items(query)

    def name_of(self, feature, href: str) -> str:
        """What a download is called in the work directory: the file the `href` names."""

        return href.rsplit("/", 1)[-1]

    def files(self, bbox):
        wanted = []
        for feature in self.items(bbox):
            asset = feature.get("assets", {}).get(self.asset)
            if asset is None:
                raise Refuse(f"{self.key}: item {feature.get('id')} has no `{self.asset}` asset")
            href = asset["href"]
            wanted.append((self.name_of(feature, href), href))
        return sorted(set(wanted))
