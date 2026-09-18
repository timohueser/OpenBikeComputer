"""One-off shadow polygons for the simulator. Uses cached data and the existing packer."""
import argparse, json, math, subprocess
from pathlib import Path
import numpy as np
import rasterio
from rasterio.features import shapes
from rasterio.transform import from_origin
from rasterio.warp import reproject, Resampling
from scipy.ndimage import gaussian_filter, label
from shapely.geometry import shape, box
from shapely.ops import transform, unary_union

p=argparse.ArgumentParser()
p.add_argument('--source',required=True,type=Path)
p.add_argument('--dem',required=True,type=Path)
p.add_argument('--out',required=True,type=Path)
p.add_argument('--name',default='shadows',help='output basename inside --out')
# Terrain lighting. Azimuth is compass degrees (0 = from north, 315 = from northwest).
p.add_argument('--smooth-m',type=float,default=200.0,help='Gaussian sigma in metres')
p.add_argument('--azimuth',type=float,default=315.0)
p.add_argument('--altitude',type=float,default=45.0)
p.add_argument('--threshold',type=float,default=None,help='instead shade where illumination is below this absolute value')
p.add_argument('--shade-fraction',type=float,default=None,help='instead pick the threshold that shades this share of the rock')
p.add_argument('--aspect-drop',type=float,default=.10,help='instead set the threshold this far below flat ground; 0 shades every slope that faces away, so the boundary follows the ridge and valley lines')
p.add_argument('--deep-fraction',type=float,default=None,help='share of the rock that also gets the deep-shade colour')
p.add_argument('--slope-above',type=float,default=None,help='select steep ground instead of ground the light misses: shade slopes over this many degrees')
p.add_argument('--deep-slope-above',type=float,default=None,help='steepness for the deep-shade colour')
p.add_argument('--and-facing-away',action='store_true',help='with --slope-above, keep only the steep ground that also faces away from the light')
p.add_argument('--hollow-radius',type=float,default=None,help='select hollows instead: compare each point with the average height within this radius, in metres')
p.add_argument('--hollow-fraction',type=float,default=.25,help='share of the rock that counts as a hollow')
p.add_argument('--deep-hollow-fraction',type=float,default=None,help='share of the rock that counts as a deep hollow')
p.add_argument('--min-cells',type=int,default=12,help='drop shade blobs smaller than this many grid cells')
p.add_argument('--simplify',type=float,default=15.0,help='boundary simplification in metres')
p.add_argument('--min-area',type=float,default=2500.0,help='drop pieces below this many square metres')
p.add_argument('--min-lod',type=int,default=9,help='0 = every zoom; 9 = 16 m/pixel and closer')
p.add_argument('--density',choices=('sparse','mid','dense'),default='sparse',help='dot density of the main shade level')
p.add_argument('--contour-color',default='0x02AA',help='override the contour colour, e.g. 0xAAAA')
a=p.parse_args(); a.out.mkdir(parents=True,exist_ok=True)
root=Path(__file__).resolve().parents[4]
# Pattern F, generated once. Tile-edge distance prevents a visible seam.
rng=np.random.default_rng(20260918)
points=[rng.integers(0,32,size=2)]
for _ in range(63):
 candidates=rng.integers(0,32,size=(32,2))
 delta=np.abs(candidates[:,None,:]-np.asarray(points)[None,:,:])
 delta=np.minimum(delta,32-delta)
 points.append(candidates[np.argmax(np.min(np.sum(delta*delta,axis=2),axis=1))])
rows=[0]*32
for y,x in points: rows[y]|=1<<int(x)
assert sum(int(r).bit_count() for r in rows)==64
(a.out/'pattern-rows.json').write_text(json.dumps(rows))

lon,lat=8.425,46.8
sx,sy=111320*math.cos(math.radians(lat)),111320
project=lambda x,y,z=None:((np.asarray(x)-lon)*sx,(np.asarray(y)-lat)*sy)
unproject=lambda x,y,z=None:(np.asarray(x)/sx+lon,np.asarray(y)/sy+lat)
geo_box=box(8.20,46.65,8.65,46.95)
region=transform(project,geo_box)
# Parsing the cached extract costs about a minute, so keep the prepared rock beside the outputs.
cache=a.out/'rock.wkb'
if cache.exists():
 from shapely import wkb
 rock=wkb.loads(cache.read_bytes())
 print('Rock loaded from cache',flush=True)
else:
 rock=[];cutouts=[]
 for f in json.load(open(a.source))['features']:
  n=f['properties'].get('natural')
  if f['geometry']['type'] not in ('Polygon','MultiPolygon'):continue
  if n not in ('bare_rock','scree','shingle','water','glacier'):continue
  g=shape(f['geometry'])
  if not g.intersects(geo_box):continue
  g=transform(project,g.intersection(geo_box))
  (rock if n in ('bare_rock','scree','shingle') else cutouts).append(g)
 rock=unary_union(rock).difference(unary_union(cutouts)).simplify(10,preserve_topology=True)
 cache.write_bytes(rock.wkb)
 print('Rock prepared',flush=True)
left,bottom,right,top=region.bounds
cell=25;pad=125
nx=math.ceil((right-left+2*pad)/cell);ny=math.ceil((top-bottom+2*pad)/cell)
left-=pad;top+=pad
grid=from_origin(left,top,cell,cell)
heights=np.zeros((ny,nx),np.float32)
with rasterio.open(a.dem) as src:
 reproject(rasterio.band(src,1),heights,src_transform=src.transform,src_crs=src.crs,dst_transform=from_origin(lon+left/sx,lat+top/sy,cell/sx,cell/sy),dst_crs='EPSG:4326',resampling=Resampling.bilinear)
# Rows run north to south, so `dy` is the southward slope and the north component flips sign.
dy,dx=np.gradient(gaussian_filter(heights,a.smooth_m/cell),cell,cell)
az,alt=math.radians(a.azimuth),math.radians(a.altitude)
lx,ly,lz=math.cos(alt)*math.sin(az),math.cos(alt)*math.cos(az),math.sin(alt)
illum=(-lx*dx+ly*dy+lz)/np.sqrt(dx*dx+dy*dy+1)
slope=np.degrees(np.arctan(np.hypot(dx,dy)))
# How far a point sits below its own surroundings. Negative in gullies and basins, positive on
# ridges and spurs. Contours cannot say which of the two you are looking at; this can.
smoothed=gaussian_filter(heights,a.smooth_m/cell)
hollow=smoothed-gaussian_filter(smoothed,a.hollow_radius/cell) if a.hollow_radius else None

# Measure only inside the rock, so a threshold means the same share of shaded rock in each variant.
from rasterio.features import rasterize
rock_mask=rasterize([(rock,1)],out_shape=(ny,nx),transform=grid,dtype='uint8').astype(bool)
inside=illum[rock_mask]
def level(fraction):
 return float(np.quantile(inside,fraction)) if fraction is not None else None
# Selection modes, most specific first: a share of the rock, an absolute level, or a distance below flat ground.
threshold=level(a.shade_fraction) or a.threshold or lz-a.aspect_drop
deep=level(a.deep_fraction)

def sink(fraction):
 return hollow<float(np.quantile(hollow[rock_mask],fraction))
def pick(degrees,illum_below):
 """Steep ground, ground the light misses, or both. `degrees` is None for the light alone."""
 if degrees is None:return illum<illum_below
 chosen=slope>degrees
 return chosen&(illum<lz) if a.and_facing_away else chosen
if hollow is not None:
 main=sink(a.hollow_fraction)
 deeper=sink(a.deep_hollow_fraction) if a.deep_hollow_fraction else None
else:
 main=pick(a.slope_above,threshold)
 deeper=pick(a.deep_slope_above,deep) if (a.deep_slope_above is not None or deep is not None) else None
share=lambda m:100*m[rock_mask].mean()
if hollow is not None:
 print(f'Hollows within {a.hollow_radius:.0f} m, smoothing {a.smooth_m:.0f} m: {share(main):.1f}% of the rock',flush=True)
elif a.slope_above is None:
 print(f'Light: azimuth {a.azimuth:.0f} deg, altitude {a.altitude:.0f} deg, smoothing {a.smooth_m:.0f} m',flush=True)
 print(f'Threshold {threshold:.4f} shades {share(main):.1f}% of the rock',flush=True)
else:
 facing=' and facing away from the light' if a.and_facing_away else ''
 print(f'Steeper than {a.slope_above:.0f} deg{facing}, smoothing {a.smooth_m:.0f} m: {share(main):.1f}% of the rock',flush=True)
if deeper is not None:print(f'Deep level: {share(deeper):.1f}% of the rock',flush=True)

def regions(mask):
 labs,_=label(mask);sizes=np.bincount(labs.ravel());small=sizes<a.min_cells;small[0]=False
 mask=mask&~small[labs]
 out=[shape(g).simplify(a.simplify,preserve_topology=True) for g,v in shapes(mask.astype('uint8'),mask=mask,transform=grid) if v]
 return unary_union(out).intersection(rock) if out else None
shade=regions(main)
deep_shade=regions(deeper) if deeper is not None else None
# The deep level replaces the ordinary one where they overlap, so no pixel is filled twice.
if deep_shade is not None:shade=shade.difference(deep_shade)

def polys(g):
 if g is None or g.is_empty:return
 if g.geom_type=='Polygon':yield g
 elif hasattr(g,'geoms'):
  for child in g.geoms:yield from polys(child)

# Bound individual source features; normal OBCM tiling and simplification follow.
def cut(g):
 pieces=[]
 for x in np.arange(region.bounds[0],region.bounds[2],1500):
  for y in np.arange(region.bounds[1],region.bounds[3],1500):
   for poly in polys(g.intersection(box(x,y,x+1500,y+1500)) if g is not None else None):
    if poly.area>=a.min_area:pieces.append(transform(unproject,poly))
 return pieces
layers=[('obc_relief_shadow',cut(shade))]
if deep_shade is not None:layers.append(('obc_relief_deep',cut(deep_shade)))
print('Shadow polygons: '+', '.join(f'{k}={len(v)}' for k,v in layers),flush=True)
node_id=500_000_000_000;way_id=500_000_000_000;relation_id=500_000_000_000
nodes=[];ways=[];relations=[]
for tag,pieces in layers:
 for poly in pieces:
  members=[]
  for role,ring in [('outer',poly.exterior)]+[('inner',r) for r in poly.interiors]:
   ids=[]
   for x,y in list(ring.coords)[:-1]:
    node_id+=1;ids.append(node_id)
    nodes.append(f'<node id="{node_id}" lat="{y:.7f}" lon="{x:.7f}" version="1"/>')
   ids.append(ids[0]);way_id+=1
   ways.append(f'<way id="{way_id}" version="1">'+''.join(f'<nd ref="{i}"/>' for i in ids)+'</way>')
   members.append(f'<member type="way" ref="{way_id}" role="{role}"/>')
  relation_id+=1
  relations.append(f'<relation id="{relation_id}" version="1">'+''.join(members)+f'<tag k="type" v="multipolygon"/><tag k="natural" v="{tag}"/></relation>')
xml=a.out/f'{a.name}.osm'
xml.write_text('<?xml version="1.0"?><osm version="0.6" generator="OBC relief visual prototype">'+''.join(nodes+ways+relations)+'</osm>')
subprocess.run(['osmium','cat',str(xml),'-o',str(a.out/f'{a.name}.osm.pbf'),'--overwrite'],check=True)
config=json.load(open(root/'builder/presets/schema.json'))
MARKERS={'sparse':'0xFABF','mid':'0xFAB5','dense':'0xA815'}
config['features']['natural']['obc_relief_shadow']={'z_index':8,'color':MARKERS[a.density],'weight':1,'priority':4,'min_lod':a.min_lod,'terrain_layer':True}
if deep_shade is not None:
 config['features']['natural']['obc_relief_deep']={'z_index':8,'color':'0xA815','weight':1,'priority':4,'min_lod':a.min_lod,'terrain_layer':True}
config['features']['contour']['major']['z_index']=9
config['features']['contour']['index']['z_index']=10
if a.contour_color:
 config['features']['contour']['major']['color']=a.contour_color
 config['features']['contour']['index']['color']=a.contour_color
(a.out/f'{a.name}-style.json').write_text(json.dumps(config,indent=2))
(a.out/f'{a.name}-summary.json').write_text(json.dumps({'layers':{k:len(v) for k,v in layers},'shadow_nodes':len(nodes),'bbox':[8.2,46.65,8.65,46.95],'grid':[nx,ny],'pattern_density':.0625,'min_lod':a.min_lod,'azimuth':a.azimuth,'altitude':a.altitude,'smooth_m':a.smooth_m,'threshold':threshold,'deep_threshold':deep,'slope_above':a.slope_above,'deep_slope_above':a.deep_slope_above,'shaded_share':share(main)/100},indent=2))
print('Prototype input ready',flush=True)
