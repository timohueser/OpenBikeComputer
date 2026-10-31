"""Write map.json for the explorer prototype from cairn's analysis. Run from the repository root."""
import collections, json, pathlib, sys
root = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / "tools/cairn"))
import cairn  # noqa: E402

config = json.loads((root / ".cairn/config.json").read_text())
docs, files, units, edges, lines = cairn.gather(root, config)
result = cairn.analyse(docs, files, units, edges, lines, config)
found = cairn.owners(files, docs)
size = collections.Counter()
for path, hits in found.items():
    if hits:
        size[hits[0]] += lines.get(path, 0)
problems = collections.defaultdict(list)
for v in result.violations:
    for s in sorted(v.systems):
        problems[s].append({"rule": v.rule, "message": v.message})
out = {
    "systems": [
        {"name": d.system, "kind": d.kind, "area": d.area or "Shells", "purpose": d.purpose, "status": d.status,
         "depends_on": list(d.depends_on), "test_depends_on": list(d.test_depends_on),
         "violations": len(problems[d.system]), "lines": size[d.system], "problems": problems[d.system]}
        for d in sorted(docs, key=lambda d: d.system)
    ],
    "edges": [{"from": a, "to": b, "test": t, "undeclared": (a, b, t) in result.undeclared}
              for a, b, t in sorted(result.edges)],
    "cycles": result.cycles,
}
path = pathlib.Path(__file__).with_name("map.json")
path.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
print(path, len(out["systems"]), "systems", len(out["edges"]), "edges", len(out["cycles"]), "cycles")
