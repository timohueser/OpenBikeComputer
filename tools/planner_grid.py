"""Merge cell descriptors without reading or changing their immutable page payloads."""
from copy import deepcopy


def ranges(values):
    result = []
    for first, last in sorted(values):
        if first >= last:
            raise ValueError("Invalid grid range")
        if result and first <= result[-1][1]:
            result[-1][1] = max(result[-1][1], last)
        else:
            result.append([first, last])
    return result


def table(target, other):
    if target["len"] != other["len"]:
        raise ValueError("Grid columns use different source lengths")
    pages = {}
    for item in (target, other):
        for number, key in zip(item.get("pages", range(len(item["blocks"]))), item["blocks"]):
            if number in pages and pages[number] != key:
                raise ValueError("Grid columns use different source pages")
            pages[number] = key
    target["pages"] = sorted(pages)
    target["blocks"] = [pages[number] for number in target["pages"]]


def merge(target, other):
    if isinstance(target, dict) and "len" in target and "blocks" in target:
        table(target, other)
    elif isinstance(target, dict):
        if target.keys() != other.keys():
            raise ValueError("Grid manifests use different schemas")
        for key in target:
            merge(target[key], other[key])
    elif isinstance(target, list) and target and isinstance(target[0], dict):
        if len(target) != len(other):
            raise ValueError("Grid manifests use different columns")
        for a, b in zip(target, other):
            merge(a, b)
    elif target != other:
        raise ValueError("Grid manifests use different source metadata")


def routing(selections, adjacency, region, bounds):
    if not selections:
        raise ValueError("There are no published roads in this area.")
    result = deepcopy(selections[0])
    result["data"]["bounds"] = bounds
    result["data"]["region"] = region
    for selection in selections[1:]:
        if selection["format"] != result["format"] or selection["source"] != result["source"]:
            raise ValueError("Grid cells use different routing releases")
        data = deepcopy(selection["data"])
        data["bounds"], data["region"] = bounds, region
        merge(result["data"], data)
        for cell, key in selection["snap"].items():
            if cell in result["snap"] and result["snap"][cell] != key:
                raise ValueError("Grid cells use different snapping data")
            result["snap"][cell] = key
    result["roads"] = ranges(r for selection in selections for r in selection["roads"])
    result["archives"] = sorted({key for selection in selections for key in selection["archives"]})
    result["arcs"] = sum(z - a for a, z in ranges(r for group in adjacency for r in group))
    return result
