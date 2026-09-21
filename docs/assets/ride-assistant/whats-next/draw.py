from pathlib import Path
from html import escape
from PIL import Image, ImageDraw

OUT=Path(__file__).parent
FONTS=Path(__file__).resolve().parents[4]/'firmware/obc-render/fonts/terminus'
PAPER='#fafaf7'; INK='#242424'; GREY='#72726e'; RULE='#deded7'; AMBER='#ffaa00'

class Frame:
    def __init__(self, name):
        self.name=name
        self.svg=['<svg xmlns="http://www.w3.org/2000/svg" width="240" height="320" viewBox="0 0 240 320" role="img" aria-label="'+escape(name)+'">']
        self.image=Image.new('RGB',(240,320),PAPER); self.d=ImageDraw.Draw(self.image)
        self.rect(0,0,240,320,PAPER)
    def rect(self,x,y,w,h,fill,stroke=None,r=0):
        self.svg.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}"'+(f' stroke="{stroke}"' if stroke else '')+'/>')
        self.d.rounded_rectangle((x,y,x+w-1,y+h-1),radius=r,fill=fill,outline=stroke)
    def line(self,points,fill=INK,width=2,dash=False):
        self.svg.append('<polyline points="'+' '.join(f'{x},{y}' for x,y in points)+f'" fill="none" stroke="{fill}" stroke-width="{width}" stroke-linejoin="round" stroke-linecap="round"'+(' stroke-dasharray="3 3"' if dash else '')+'/>')
        self.d.line(points,fill=fill,width=width,joint='curve')
    def polygon(self,points,fill=INK):
        self.svg.append('<polygon points="'+' '.join(f'{x},{y}' for x,y in points)+f'" fill="{fill}"/>')
        self.d.polygon(points,fill=fill)
    def text(self,s,x,y,body=False,fill=INK):
        w,h=(14,28) if body else (12,24)
        raw=(FONTS/f'ter_u{h}b.raw').read_bytes(); stride=16*w//8
        self.svg.append('<g aria-label="'+escape(s)+'">')
        for n,c in enumerate(s):
            idx=ord(c)-32
            assert 0<=idx<96, c
            gx=(idx%16)*w; gy=(idx//16)*h
            for yy in range(h):
                xx=0
                while xx<w:
                    bit=lambda at: raw[(gy+yy)*stride+(gx+at)//8] & (128>>((gx+at)%8))
                    if not bit(xx): xx+=1; continue
                    first=xx
                    while xx<w and bit(xx): xx+=1
                    self.rect(x+n*w+first,y+yy,xx-first,1,fill)
        self.svg.append('</g>')
    def title(self,text,count=None,filtered=False):
        self.rect(0,0,240,40,PAPER)
        self.rect(4,4,232,34,INK,r=5)
        self.text(text,12,6,True,PAPER)
        if count: self.text(count,228-len(count)*12,8,fill=PAPER)
        if filtered:
            self.polygon([(137,13),(153,13),(148,20),(148,28),(143,26),(143,20)],AMBER)
    def icon(self,kind,x,y,selected=False,planned=False,marker=False):
        if marker: self.rect(x-13,y-14,27,28,AMBER if selected else PAPER,INK,4)
        if kind=='water':
            self.polygon([(x,y-9),(x-7,y+2),(x-6,y+7),(x,y+9),(x+6,y+7),(x+7,y+2)])
            self.line([(x+2,y+2),(x+2,y+5)],PAPER,2)
        elif kind=='shop':
            self.line([(x-8,y-3),(x-6,y+7),(x+6,y+7),(x+8,y-3),(x-8,y-3)],INK,2)
            self.line([(x-5,y-4),(x-2,y-9),(x+2,y-9),(x+5,y-4)],INK,2)
            self.line([(x-3,y),(x-2,y+4)],INK,1);self.line([(x+3,y),(x+2,y+4)],INK,1)
        elif kind=='camp':
            self.line([(x,y-9),(x-9,y+8),(x+9,y+8),(x,y-9),(x,y+8)],INK,2)
        else:
            self.polygon([(x,y-9),(x+8,y),(x,y+9),(x-8,y)])
        if planned: self.polygon([(x+12,y-13),(x+16,y-9),(x+12,y-5),(x+8,y-9)],INK)
    def map(self,kinds=('water','shop'),selected=0,planned=(False,False),bottom=184):
        self.rect(0,40,240,bottom-40,'#f0f0e9')
        self.rect(12,70,39,32,'#e5e5df',r=2);self.rect(184,117,44,43,'#e5e5df',r=2)
        for pts in [[(0,98),(70,85),(165,145),(240,129)],[(71,40),(55,77),(94,159),(126,bottom)],[(0,157),(70,164),(170,44)],[(215,40),(181,87),(227,bottom)]]:
            self.line(pts,'#d5d5cd',7)
        self.line([(44,173),(89,146),(112,129),(101,108),(134,75),(193,49)],'#686861',5)
        self.polygon([(44,161),(37,177),(44,173),(51,177)],INK)
        pts=[(149,144),(153,73)]
        self.line([(112,129),(149,144)],GREY,1,True)
        for i,(kind,(x,y)) in enumerate(zip(kinds,pts)):
            self.icon(kind,x,y,i==selected,planned[i],True)
    def row(self,e,y,selected=False,h=62):
        name,kind,distance,climb,offset,planned=e
        self.rect(8,y,224,h,AMBER if selected else PAPER,r=5)
        self.icon(kind,23,y+19,planned=planned)
        self.text(name,40,y+3,True)
        self.text(distance,16,y+33)
        self.polygon([(94,y+49),(103,y+49),(99,y+39)])
        self.text(climb,110,y+33)
        if offset:
            tx=224-len(offset)*12
            self.polygon([(tx-14,y+39),(tx-14,y+49),(tx-7,y+44)])
            self.text(offset,tx,y+33)
    def save(self):
        self.svg.append('</svg>')
        (OUT/(self.name+'.svg')).write_text(''.join(self.svg))
        self.image.save(OUT/(self.name+'.png'))


HILL=[(0,0),(.8,40),(1.3,30),(2,50),(5,260),(7,120),(8.5,190),(10,120)]
DOWNHILL=[(0,0),(.7,10),(3,-110),(4,-100),(5.5,-130),(7,-110),(10,-140)]

def height(points,d):
    for (a,za),(b,zb) in zip(points,points[1:]):
        if a<=d<=b: return za+(zb-za)*(d-a)/(b-a)
    return points[-1][1]

def profile(f,points,span,climb=None,water=None,stop=None):
    # The examples use the same 300 m vertical range. The diagram does not encode road gradient.
    import math
    low=math.floor(min(z for _,z in points)/100)*100
    x=lambda km:round(12+216*km/span)
    y=lambda z:round(178-(z-low)*56/300)
    visible=[p for p in points if p[0]<span]+[(span,height(points,span))]
    coords=[(x(km),y(z)) for km,z in visible]
    f.polygon([(12,178)]+coords+[(228,178)],'#deded7')
    if climb:
        a,b=climb;b=min(b,span)
        if a<b:
            middle=[p for p in visible if a<p[0]<b]
            slope=[(a,height(points,a))]+middle+[(b,height(points,b))]
            f.polygon([(x(a),178)]+[(x(km),y(z)) for km,z in slope]+[(x(b),178)],AMBER)
    f.line(coords,INK,2)
    for km,kind in [(water,'water'),(stop,'planned')]:
        if km is not None and km<=span:
            xx,yy=x(km),y(height(points,km))
            f.line([(xx,yy),(xx,yy-14)],GREY,1)
            f.icon(kind,xx,yy-18,planned=False)
    f.text('Now',12,179,fill=GREY)
    end=f'{span}km';f.text(end,228-len(end)*12,179,fill=GREY)

def totals(f,up,down):
    f.polygon([(13,59),(23,59),(18,48)])
    f.text(f'{up}m',32,43)
    f.polygon([(130,48),(140,48),(135,59)])
    f.text(f'{down}m',149,43)

def brief(name,span,up,down,points,headline,subtitle,stop_name,stop_distance,relation,water_distance,shop_distance,climb=None):
    f=Frame(name);f.title(f'Next {span}km')
    f.polygon([(217,18),(227,18),(222,11)],PAPER)
    f.polygon([(217,25),(227,25),(222,32)],PAPER)
    totals(f,up,down)
    f.text(headline,12,70,True)
    f.text(subtitle,12,99)
    if climb:f.text('+210m',166,99)
    profile(f,points,span,climb,water_distance,stop_distance)
    f.icon('planned',17,218)
    f.text(stop_name,34,204,True)
    distance=f'{stop_distance:g}km'
    f.text(distance,228-len(distance)*12,206)
    f.text(relation,12,232,fill=GREY)
    f.icon('water',18,267);f.text('800m' if water_distance==.8 else f'{water_distance:g}km',34,255)
    f.icon('shop',133,267);f.text(f'{shop_distance:g}km' if shop_distance<=span else 'none',150,255)
    f.rect(12,284,216,32,AMBER,r=5);f.text('Explore ahead',29,286,True)
    f.save()

brief('01-next-10km',10,340,220,HILL,'Climb in 2km','3km at 7%','Lunch',7,'After the descent',.8,6.4,(2,5))
brief('02-next-5km',5,270,10,HILL,'Climb in 2km','3km at 7%','Lunch',7,'Outside this view',.8,6.4,(2,5))
brief('03-downhill',10,40,180,DOWNHILL,'Mostly downhill','With short rises','Camp',4.5,'Water before camp',1.1,2.8)

f=Frame('04-explore');f.title('Ahead 10km','1/16')
f.row(('Village water','water','800m','40m','80m',False),44,True,63)
f.rect(8,111,224,63,PAPER,r=5)
f.polygon([(14,141),(31,141),(23,120)])
f.text('Climb: 210m',40,114,True);f.text('in 2km; 3km at 7%',12,144)
f.row(('Village shop','shop','6.4km','270m','',False),178,False,63)
f.row(('Lunch stop','shop','7km','270m','',True),245,False,63)
f.rect(235,46,2,264,RULE);f.rect(235,46,2,66,GREY);f.save()

f=Frame('05-filters');f.title('Water','1/3',True)
f.row(('Village water','water','800m','40m','80m',False),44,True,63)
f.rect(0,130,240,190,PAPER,INK,r=8)
for y,left,right,active in [(144,'Range','10km',False),(200,'Filter','Water',True),(256,'Sources','Both',False)]:
    if active:f.rect(10,y,220,51,AMBER,r=5)
    f.text(left,18,y+9,True);f.text(right,224-len(right)*12,y+11)
f.save()

frames=[
('01-next-10km','A brief of the next part of the ride','A climb starts in 2 km and rises 210 m over 3 km at 7% average. Mapped water is before it; your lunch waypoint is after the descent. The totals cover the full 10 km window.'),
('02-next-5km','Look closer: the next 5 km','The same ride, with a shorter window. Climbing totals and services change with the range. Lunch is explicitly beyond the window. In this fictional, fully covered example, there is no mapped shop inside 5 km.'),
('03-downhill','A different stretch of riding','The same layout shows a descending stretch, a planned camp, and nearby services. The profile uses the same 300 m vertical range as the climbing example. Mostly downhill does not promise an easy ride.'),
('04-explore','The complete available timeline','Select Explore ahead to browse climbs, planned waypoints, and map POIs in route order. This list has no four-recommendation limit. Only four of the example events are visible at once.'),
('05-filters','Retain detailed filtering','Range scopes the window. The detailed timeline retains category and source filtering. A filtered list says what it shows; it does not remove terrain or planned stops from the overview.'),
]
html='''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Ride Assistant · What's next?</title><style>
*{box-sizing:border-box}body{margin:32px;background:#eeeee8;color:#242424;font:16px/1.5 system-ui,sans-serif}main{max-width:1180px;margin:auto}header{max-width:920px}h1{font-size:30px;margin:0}h2{font-size:17px;margin:0 0 14px}section{display:grid;grid-template-columns:repeat(auto-fit,minmax(272px,1fr));gap:20px;margin-top:26px}figure{margin:0;background:white;padding:16px;border-radius:8px}img{display:block;width:240px;height:320px;image-rendering:pixelated}figcaption{font-size:14px;color:#555;max-width:240px;margin-top:12px}a{color:#795100}button{font:inherit;background:white;border:1px solid #888;border-radius:4px;padding:6px 12px;cursor:pointer}.large img{width:480px;height:640px}.large section{grid-template-columns:repeat(auto-fit,minmax(512px,1fr))}.large figcaption{max-width:480px}footer{max-width:920px;margin-top:30px}
</style><main><header><h1>What does the next part of the ride look like?</h1><p><strong>Proposed direction:</strong> a glanceable route brief with terrain, planned stops and services on one distance window. Explore ahead opens the detailed, filterable timeline. A geographical map belongs in the selected-place details.</p><p>These are simple layout studies at 240 × 320 using the firmware's native Terminus fonts. All values and places are fictional. The profile examples have internally consistent ascent/descent totals. This is not implemented firmware, an access calculation or a validated climb-selection policy.</p><p><a href="../../study.md">Read the proposal and selection rules</a> · <a href="https://github.com/timohueser/OpenBikeComputer/issues/1734">Issue #1734</a></p><button onclick="document.body.classList.toggle('large')">Toggle 1× / 2×</button></header><section>'''
import base64
for file,title,caption in frames:
    data=base64.b64encode((OUT/(file+'.svg')).read_bytes()).decode()
    html+='<figure><h2>'+escape(title)+'</h2><img src="data:image/svg+xml;base64,'+data+'" alt="'+escape(title)+'"><figcaption>'+escape(caption)+'</figcaption></figure>'
html+='''</section><footer><p><strong>Proposed buttons:</strong> Up/Down changes the look-ahead distance on the overview, like zoom. Select opens the detailed timeline, where Up/Down selects entries as usual. Down + Back opens the existing filter interaction. Back returns to the overview with the same range.</p><p>The useful addition is the relationship between facts: water before a climb, a planned stop after it, or several climbs before the next stop. Generate only relationships supported by route positions and available data. Do not claim a service is the last one merely because a bounded query returned no more.</p><p>Font: Terminus, SIL Open Font License. <a href="FONT-LICENSE.txt">License</a>.</p></footer></main></html>'''
(OUT/'index.html').write_text(html)
(OUT/'FONT-LICENSE.txt').write_text((FONTS/'LICENSE').read_text())
print('Five route-brief wireframes created; firmware unchanged.')
