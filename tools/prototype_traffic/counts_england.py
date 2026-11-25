"""Writes the counted DfT minor-road points inside a box, latest counted year of each point since 2010."""
import sys
import pandas as pd

dft, box, out = sys.argv[1], [float(x) for x in sys.argv[2].split(",")], sys.argv[3]
d = pd.read_csv(dft, low_memory=False)
c = d[(d.road_type == "Minor") & (d.estimation_method == "Counted") & (d.year >= 2010)]
c = c.sort_values("year").groupby("count_point_id").last().reset_index()
c = c[c.longitude.between(box[0], box[2]) & c.latitude.between(box[1], box[3])]
c = c.rename(columns={"count_point_id": "id", "latitude": "lat", "longitude": "lon", "all_motor_vehicles": "aadt",
                      "road_category": "cat", "road_name": "name", "region_name": "region"})
c[["id", "lat", "lon", "aadt", "cat", "name", "year", "region"]].to_csv(out, index=False)
print(len(c), "points;", c.region.value_counts().to_dict())
