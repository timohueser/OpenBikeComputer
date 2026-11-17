"""python3 zoom.py: the sheets of zoom.mjs, each cell labelled with its marks in view."""
import json
from PIL import Image, ImageDraw, ImageFont
D = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/places-zoom/'
marks = json.load(open(D + 'marks.json'))
def font(size): return ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf', size)
big, small = font(34), font(30)
W, H, GAP, HEAD = 900, 650, 12, 96

def sheet(out, title, cols, rows):
    """cols: [(label, run)], rows: zooms. The file of a cell is AREA-RUN-zZOOM."""
    img = Image.new('RGB', (len(cols) * (W + GAP) + GAP, HEAD + len(rows) * (H + GAP)), '#22211c')
    draw = ImageDraw.Draw(img)
    draw.text((GAP + 6, 6), title, font=big, fill='#ffffff')
    for c, (label, _) in enumerate(cols): draw.text((GAP + c * (W + GAP) + 6, 50), label, font=small, fill='#e8d98a')
    for r, zoom in enumerate(rows):
        for c, (label, run) in enumerate(cols):
            name = f'{run}-z{zoom}'
            x, y = GAP + c * (W + GAP), HEAD + r * (H + GAP)
            img.paste(Image.open(f'{D}single/{name}.png').convert('RGB').resize((W, H), Image.LANCZOS), (x, y))
            m = marks[name]
            text = f'map z{zoom} · {m["marks"]} marks · {m["places"]} places'
            box = draw.textbbox((x + 12, y + 10), text, font=small)
            draw.rectangle((box[0] - 8, box[1] - 7, box[2] + 8, box[3] + 7), fill='#22211c')
            draw.text((x + 12, y + 10), text, font=small, fill='#ffffff')
    img.save(D + out)
    print(out, img.size)

names = {'town': 'Lucerne old town', 'valley': 'Urseren valley', 'mountains': 'Göscheneralp mountains'}
for area in ('town', 'valley'):
    first = [(f'first zoom {z}', f'{area}-default-first{z}') for z in (12, 13, 14)]
    pinned = [('water pinned from 12, others from 14', f'{area}-default-first14-pinned-water'), ('nothing pinned, all from 14', f'{area}-default-first14'), ('all from 12', f'{area}-default-first12')]
    for rows in ((12, 13), (14, 15)):
        part = f'z{rows[0]}-{rows[1]}'
        sheet(f'first-zoom-{area}-{part}.png', f'{names[area]} · 7 default kinds · columns: first zoom of the places', first, rows)
        sheet(f'pinned-{area}-{part}.png', f'{names[area]} · 7 default kinds · pinned water', pinned, rows)
for area in ('town', 'mountains'):
    kinds = [('all 13 kinds', 'all'), ('7 default kinds', 'default'), ('A: water, food shop, campsite', 'setA'), ('B: A + bike service, peak', 'setB')]
    for half, part in ((kinds[:2], 'all-default'), (kinds[2:], 'setA-setB')):
        sheet(f'kinds-{area}-{part}.png', f'{names[area]} · first zoom 12 · columns: kinds on', [(l, f'{area}-{k}-first12') for l, k in half], (13, 14))
