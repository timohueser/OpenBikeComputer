#!/usr/bin/env python3
"""Generate search discovery files from the finished site artifact."""

import sys
import xml.etree.ElementTree as ET
from pathlib import Path
from urllib.parse import quote

from build_docs import SITE


def main(dist):
    if not (dist / "index.html").is_file():
        sys.exit("sitemap: site root has no index.html")

    urlset = ET.Element("urlset", xmlns="http://www.sitemaps.org/schemas/sitemap/0.9")
    for page in sorted(dist.rglob("index.html")):
        path = page.parent.relative_to(dist)
        url = SITE + "/".join(quote(part) for part in path.parts)
        if path.parts:
            url += "/"
        ET.SubElement(ET.SubElement(urlset, "url"), "loc").text = url

    ET.indent(urlset)
    ET.ElementTree(urlset).write(dist / "sitemap.xml", encoding="utf-8", xml_declaration=True)
    (dist / "robots.txt").write_text(f"Sitemap: {SITE}sitemap.xml\n")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
