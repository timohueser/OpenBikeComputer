import json,sys
d=json.load(open(sys.argv[1]));r=d['routes'][0]
c=r['coordinates_udeg'];lon=lat=0;pts=[]
e=r['elevation_dm'];h=0;hs=[]
for i in range(0,len(c),2):
    lon+=c[i];lat+=c[i+1];pts.append((lon/1e6,lat/1e6))
for v in e:
    h+= (v or 0); hs.append(round(h/10))
idx=list(range(0,len(pts),3))
if idx[-1]!=len(pts)-1: idx.append(len(pts)-1)
line=[round(x,5) for i in idx for x in pts[i]]
heights=[hs[i] for i in idx]
t=r['totals']
lons=[p[0] for p in pts];lats=[p[1] for p in pts]
summary=dict(line=line,heights=heights,bounds=[min(lons),min(lats),max(lons),max(lats)],lengthM=t['distance_m'],climbM=t['ascent_m'],highestM=max(hs),seconds=t['seconds'],bike='touring',days=1)
P=json.loads(sys.argv[2])
points=[dict(id=k,kind=k,label=lbl,coordinate=xy) for k,lbl,xy in P]
trip=dict(mode='trip',bike='touring',days=1,daysSet=True,restAfter=[],routeOrder=[p['id'] for p in points[1:-1]],points=points)
print(json.dumps(dict(id='stans',revision=0,name='Hergiswil to Wolfenschiessen',updatedAt=0,trip=trip,summary=summary)))
