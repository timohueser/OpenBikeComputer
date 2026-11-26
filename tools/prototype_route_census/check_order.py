"""Prototype: checks that the nodes of an OSM PBF come in ID order, as the routing import needs.

Usage: uv run --no-project --with osmium python3 -I check_order.py FILE.osm.pbf
"""

import sys

import osmium


class Order(osmium.SimpleHandler):
    def __init__(self):
        super().__init__()
        self.previous = None
        self.unsorted = 0
        self.counts = {"node": 0, "way": 0, "relation": 0}
        self.bicycle_routes = 0

    def node(self, node):
        self.counts["node"] += 1
        if self.previous is not None and node.id < self.previous:
            self.unsorted += 1
        self.previous = node.id

    def way(self, way):
        self.counts["way"] += 1

    def relation(self, relation):
        self.counts["relation"] += 1
        tags = relation.tags
        if tags.get("route") == "bicycle" and tags.get("type") in ("route", "superroute"):
            self.bicycle_routes += 1


order = Order()
order.apply_file(sys.argv[1])
print(order.counts, "bicycle route relations:", order.bicycle_routes, "unsorted node steps:", order.unsorted)
sys.exit(1 if order.unsorted else 0)
