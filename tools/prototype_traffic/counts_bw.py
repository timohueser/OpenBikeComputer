"""Writes counted 2022-2024 L and K road points of the Baden-Wuerttemberg census (dl-de/by-2-0) inside a box, located by the count point map table."""
import sys, re
import pandas as pd

census_dir, locations_csv, box, out = sys.argv[1], sys.argv[2], [float(x) for x in sys.argv[3].split(",")], sys.argv[4]
loc = pd.read_csv(locations_csv, dtype=str).set_index("svznr")
rows = []
for cat in "LK":
    d = pd.read_csv(f"{census_dir}/BW_VM_2024_{cat}_Zeilenformat.csv", sep=";", dtype=str)
    for _, r in d.iterrows():
        tk, year = r["TKZST"], int(r["Erhebungsjahr"]) if isinstance(r["Erhebungsjahr"], str) else 0
        m = re.fullmatch(r"([LKB]) ?(\d+) ?([A-Za-z]?)", str(r["Str"]).strip())
        if not m or year < 2022 or tk not in loc.index:  # older years are forwarded estimates, not counts
            continue
        rows.append(dict(id=tk, lat=float(loc.loc[tk].gpsy1), lon=float(loc.loc[tk].gpsx1), aadt=int(r[f"DTV{year}"]), cat=cat,
                         name=cat + m.group(2), year=year))
d = pd.DataFrame(rows)
d = d[(d.aadt > 0) & d.lon.between(box[0], box[2]) & d.lat.between(box[1], box[3])]
d.to_csv(out, index=False)
print(len(d), d.cat.value_counts().to_dict())
