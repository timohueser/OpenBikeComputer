"""Fetches Norwegian county and municipal road counts (NLOD) and writes the latest yearly daily volume of each point."""
import sys, json, re, time, urllib.request
import pandas as pd
from concurrent.futures import ThreadPoolExecutor

URL, out_dir = "https://trafikkdata-api.atlas.vegvesen.no/", sys.argv[1]


def q(query):
    req = urllib.request.Request(URL, data=json.dumps({"query": query}).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


pts = q("""{ trafficRegistrationPoints(searchQuery:{roadCategoryIds:[F,K]}) { id name trafficRegistrationType
  location { coordinates { latLon { lat lon } } roadReference { shortForm roadCategory { id } } } } }""")["data"]["trafficRegistrationPoints"]
pts = [p for p in pts if p["trafficRegistrationType"] == "VEHICLE" and re.fullmatch(r"[0-9A-Za-z]+", p["id"])]
SEL = "volume { average { daily { byYear { year dayType total { volume { average } coverage { percentage } } } } } }"


def batch(ids):
    for _ in range(3):
        try:
            d = q("{" + " ".join(f'p{i}: trafficData(trafficRegistrationPointId:"{p}") {{ {SEL} }}' for i, p in enumerate(ids)) + "}").get("data") or {}
            return {p: d.get(f"p{i}") for i, p in enumerate(ids)}
        except Exception:
            time.sleep(2)
    return {}


vol = {}
ids = [p["id"] for p in pts]
with ThreadPoolExecutor(4) as ex:
    for r in ex.map(batch, [ids[i:i + 50] for i in range(0, len(ids), 50)]):
        vol.update(r)
rows = []
for p in pts:
    ys = [(y["year"], y["total"]["volume"]["average"], (y["total"].get("coverage") or {}).get("percentage"))
          for y in ((vol.get(p["id"]) or {}).get("volume", {}).get("average", {}).get("daily", {}).get("byYear", []))
          if y.get("dayType") == "ALL" and y.get("total") and y["total"].get("volume") and isinstance(y["total"]["volume"].get("average"), int)]
    if not ys:
        continue
    ok = sorted(y for y in ys if y[2] is not None and y[2] >= 50)
    year, aadt, cov = ok[-1] if ok else sorted(ys)[-1]
    ll, ref = p["location"]["coordinates"]["latLon"], p["location"]["roadReference"]
    rows.append(dict(id=p["id"], lat=ll["lat"], lon=ll["lon"], aadt=aadt, cat=ref["roadCategory"]["id"], name=ref["shortForm"].split()[0],
                     year=year, continuous=bool(ok)))
d = pd.DataFrame(rows)
d = d[(d.aadt > 0) & (d.year >= 2010)]
# Continuous points (coverage of 50 % or more) and periodic points (a short count, expanded by the road authority) are two test sets.
d[d.continuous].to_csv(f"{out_dir}/counts_std.csv", index=False)
d[~d.continuous].to_csv(f"{out_dir}/counts_periodic_std.csv", index=False)
print("continuous", d.continuous.sum(), "periodic", (~d.continuous).sum())
