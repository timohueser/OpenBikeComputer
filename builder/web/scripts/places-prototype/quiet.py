"""python3 quiet.py: the sheets of quiet.mjs, each cell labelled with its look, zoom and marks in view; and marks-closeup.png."""
import json
from PIL import Image, ImageDraw, ImageFont
D = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/places-quiet/'
marks = json.load(open(D + 'marks.json'))
def font(size, bold=True): return ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans%s.ttf' % ('-Bold' if bold else ''), size)
big, small = font(32), font(24)
W, H, GAP, HEAD = 900, 650, 12, 56
NAMES = {'icons': 'icons', 'glyph': 'bare icons', 'neutral': 'rings', 'dot': 'dots', 'hl': 'discs', 'badge': 'badges'}

def label(name):
    counts = marks[name]['counts']
    parts = [f'{n} {NAMES[k]}' for k, n in counts.items()]
    return ', '.join(parts) or 'no marks'

def sheet(out, title, cols, rows):
    """cols: [column title], rows: [[(file, cell label)]]."""
    img = Image.new('RGB', (len(cols) * (W + GAP) + GAP, HEAD + 40 + len(rows) * (H + GAP)), '#22211c')
    draw = ImageDraw.Draw(img)
    draw.text((GAP + 4, 8), title, font=big, fill='#ffffff')
    for c, col in enumerate(cols): draw.text((GAP + c * (W + GAP) + 4, HEAD + 4), col, font=small, fill='#e8d98a')
    for r, row in enumerate(rows):
        for c, (name, text) in enumerate(row):
            x, y = GAP + c * (W + GAP), HEAD + 40 + r * (H + GAP)
            img.paste(Image.open(f'{D}single/{name}.png').convert('RGB').resize((W, H), Image.LANCZOS), (x, y))
            text = f'{text} · {label(name)}'
            box = draw.textbbox((x + 10, y + 8), text, font=small)
            draw.rectangle((box[0] - 6, box[1] - 5, box[2] + 6, box[3] + 5), fill='#22211c')
            draw.text((x + 10, y + 8), text, font=small, fill='#ffffff')
    img.save(D + out)
    print(out, img.size)

AREA = {'town': 'Lucerne old town', 'valley': 'Urseren valley', 'mountains': 'Göscheneralp'}
LOOK = {'today': 'today', 'badges': 'badges (tile)', 'glyph': 'quiet glyph, from 14', 'dot': 'quiet dot, from 14', 'none': 'none until 16'}
for area, zooms in (('town', (14, 15)), ('valley', (13, 14))):
    for zoom in zooms:
        for part, looks in (('a', ('today', 'badges')), ('b', ('glyph', 'dot', 'none'))):
            sheet(f'looks-{area}-z{zoom}-{part}.png', f'{AREA[area]} · zoom {zoom} · 7 default kinds', [LOOK[l] for l in looks],
                  [[(f'looks-{area}-{l}-z{zoom}', f'{LOOK[l]} · z{zoom}') for l in looks]])
sheet('glyph-town-zooms.png', 'Lucerne old town · quiet glyph · rows: minzoom 14, minzoom 15', [f'zoom {z}' for z in (13, 14, 15, 16)],
      [[(f'glyph-m{m}-town-z{z}', f'glyph, from {m} · z{z}') for z in (13, 14, 15, 16)] for m in (14, 15)])
for kind, area, zooms in (('water', 'town', (12, 13, 14, 15)), ('camp', 'valley', (12, 13, 14)), ('camp', 'mountains', (12, 13, 14))):
    sheet(f'highlight-{kind}-{area}.png', f'{AREA[area]} · quiet glyph from 14 + highlight={kind}', ['highlight', 'highlight + dim=1'],
          [[(f'hl{kind}-{area}-z{z}', f'highlight {kind} · z{z}'), (f'hl{kind}-dim-{area}-z{z}', f'+ dim · z{z}')] for z in zooms])
sheet('dark-town.png', 'Lucerne old town · zoom 14 · highlight=water · dark (light for reference)', ['quiet glyph + highlight', 'quiet glyph + highlight + dim=1'],
      [[('dark-town-z14', 'dark · glyph + water'), ('dark-dim-town-z14', 'dark · + dim')], [('hlwater-town-z14', 'light · glyph + water'), ('dark-dot-town-z14', 'dark · dot + water')]])

# The close crops: one mark of each type, 2x pixels of the real map, enlarged 3 times more.
C, ZOOM = 44, 3
def crop(name, look, several=None):
    items = [i for i in marks[name]['items'] if i['look'] == look and 60 < i['x'] < 840 and 60 < i['y'] < 590 and (several is None or (i['count'] > 1) == several)]
    item = min(items, key=lambda i: abs(i['x'] - 450) + abs(i['y'] - 325))
    im = Image.open(f'{D}single/{name}.png').convert('RGB')
    x, y = round(item['x'] * 2), round(item['y'] * 2)
    return im.crop((x - C, y - C, x + C, y + C)).resize((2 * C * ZOOM, 2 * C * ZOOM), Image.NEAREST)
TYPES = [('bare glyph', 'glyph', None), ('neutral ring', 'neutral', None), ('dot', 'dot', None), ('highlight disc', 'hl', False), ('disc + second disc', 'hl', True)]
ROWS = [('light', {'glyph': 'looks-town-glyph-z14', 'neutral': 'looks-town-glyph-z14', 'dot': 'looks-town-dot-z14', 'hl': 'hlwater-town-z14'}),
        ('dark', {'glyph': 'dark-town-z14', 'neutral': 'dark-town-z14', 'dot': 'dark-dot-town-z14', 'hl': 'dark-town-z14'})]
S, LEFT = 2 * C * ZOOM, 120
img = Image.new('RGB', (LEFT + len(TYPES) * (S + GAP), 60 + len(ROWS) * (S + GAP)), '#22211c')
draw = ImageDraw.Draw(img)
for c, (text, _, _) in enumerate(TYPES): draw.text((LEFT + c * (S + GAP) + 4, 16), text, font=small, fill='#e8d98a')
for r, (scheme, files) in enumerate(ROWS):
    draw.text((12, 60 + r * (S + GAP) + S // 2 - 12), scheme, font=small, fill='#ffffff')
    for c, (_, look, several) in enumerate(TYPES): img.paste(crop(files[look], look, several), (LEFT + c * (S + GAP), 60 + r * (S + GAP)))
img.save(D + 'marks-closeup.png')
print('marks-closeup.png', img.size)
