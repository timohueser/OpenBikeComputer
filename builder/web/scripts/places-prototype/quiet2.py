"""python3 quiet2.py stack|closeup|hover|coarse|recommended: the sheets of quiet2.mjs."""
import json, sys
from PIL import Image, ImageDraw, ImageFont
D = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/places-quiet2/'
marks = json.load(open(D + 'marks.json'))
def font(size): return ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf', size)
big, small = font(30), font(22)
W, H, GAP, HEAD = 900, 650, 12, 52
NAMES = {'glyph': 'icons', 'neutral': 'rings', 'hl': 'discs'}

def label(name):
    m = marks[name]
    text = ', '.join(f'{n} {NAMES[k]}' for k, n in m['counts'].items()) or 'no marks'
    if m['longest']: text += f' · longest row {m["longest"]}, {m["over3"]} spots > 3'
    if m['touching']: text += f' · {m["touching"]} icons touch'
    return text

def tag(draw, x, y, text):
    box = draw.textbbox((x, y), text, font=small)
    draw.rectangle((box[0] - 6, box[1] - 5, box[2] + 6, box[3] + 5), fill='#22211c')
    draw.text((x, y), text, font=small, fill='#ffffff')

def sheet(out, title, cols, rows):
    """rows: [[(file, cell label)]]."""
    img = Image.new('RGB', (len(cols) * (W + GAP) + GAP, HEAD + 36 + len(rows) * (H + GAP)), '#22211c')
    draw = ImageDraw.Draw(img)
    draw.text((GAP + 4, 8), title, font=big, fill='#ffffff')
    for c, col in enumerate(cols): draw.text((GAP + c * (W + GAP) + 4, HEAD), col, font=small, fill='#e8d98a')
    for r, row in enumerate(rows):
        for c, (name, text) in enumerate(row):
            x, y = GAP + c * (W + GAP), HEAD + 36 + r * (H + GAP)
            img.paste(Image.open(f'{D}single/{name}.png').convert('RGB').resize((W, H), Image.LANCZOS), (x, y))
            tag(draw, x + 10, y + 8, text)
            tag(draw, x + 10, y + 42, label(name))
    img.save(D + out)
    print(out, img.size)

what = sys.argv[1]
AREA = {'town': 'Lucerne old town', 'village': 'Andermatt (Urseren valley)'}
LOOKS = ('kinds', 'each', 'ring')
if what == 'stack':
    for area in AREA:
        sheet(f'stack-{area}-z15.png', f'{AREA[area]} · zoom 15 · rows: stack=kinds, each, ring', ['first icons at 15', 'first icons at 16'],
              [[(f'stack-{area}-{s}-m{m}-z15', f'stack={s} · from {m} · z15') for m in (15, 16)] for s in LOOKS])
        for z in (16, 17):
            sheet(f'stack-{area}-z{z}.png', f'{AREA[area]} · zoom {z} · same for first icons at 15 and 16', ['stack=kinds, each, ring'],
                  [[(f'stack-{area}-{s}-m15-z{z}', f'stack={s} · z{z}')] for s in LOOKS])
if what == 'closeup':
    # The busiest spot of stack=kinds, the same piece of map in each look, 2x pixels enlarged twice.
    CW, CH, Z = 200, 130, 2
    img = Image.new('RGB', (150 + 2 * (CW * 2 * Z + GAP), 50 + 3 * (CH * 2 * Z + GAP)), '#22211c')
    draw = ImageDraw.Draw(img)
    for c, z in enumerate((15, 16)):
        draw.text((150 + c * (CW * 2 * Z + GAP), 12), f'old town, zoom {z}: busiest spot', font=small, fill='#e8d98a')
        b = marks[f'stack-town-kinds-m15-z{z}']['busiest']
        for r, s in enumerate(LOOKS):
            im = Image.open(f'{D}single/stack-town-{s}-m15-z{z}.png').convert('RGB')
            x, y = round(b['x'] * 2 - CW), round(b['y'] * 2 - CH)
            im = im.crop((x, y, x + CW * 2, y + CH * 2)).resize((CW * 2 * Z, CH * 2 * Z), Image.LANCZOS)
            img.paste(im, (150 + c * (CW * 2 * Z + GAP), 50 + r * (CH * 2 * Z + GAP)))
            if c == 0: draw.text((12, 50 + r * (CH * 2 * Z + GAP) + CH * Z - 12), s, font=big, fill='#ffffff')
    img.save(D + 'stack-closeup.png'); print('stack-closeup.png', img.size)
if what == 'hover':
    STATES, ITEMS = ('rest', 'hover', 'pressed', 'focus'), (('icon', 'quiet icon'), ('row', 'row (hover on last)'), ('disc', 'rust disc'))
    cell = Image.open(f'{D}work/hover-icon-rest-light.png').size
    img = Image.new('RGB', (300 + 4 * (cell[0] + GAP), 50 + 6 * (cell[1] + GAP)), '#22211c')
    draw = ImageDraw.Draw(img)
    for c, s in enumerate(STATES): draw.text((300 + c * (cell[0] + GAP), 12), s + (' (keyboard)' if s == 'focus' else ''), font=small, fill='#e8d98a')
    r = 0
    for scheme in ('light', 'dark'):
        for item, text in ITEMS:
            y = 50 + r * (cell[1] + GAP)
            draw.text((12, y + cell[1] // 2 - 24), text, font=small, fill='#ffffff'); draw.text((12, y + cell[1] // 2 + 4), scheme, font=small, fill='#bbb')
            for c, s in enumerate(STATES): img.paste(Image.open(f'{D}work/hover-{item}-{s}-{scheme}.png').convert('RGB'), (300 + c * (cell[0] + GAP), y))
            r += 1
    img.save(D + 'hover-states.png'); print('hover-states.png', img.size)
if what == 'coarse':
    sheet('highlight-water-town-coarse.png', 'Lucerne · highlight=water, dim=1 · reach of a disc below zoom 14', ['before: 26 px', 'coarse: 64 px below 14, 26 px from 14'],
          [[(f'hl-fine-z{z}', f'26 px · z{z}'), (f'hl-coarse-z{z}', f'64 px · z{z}')] for z in (12, 13, 14)])
if what == 'recommended':
    rec = sys.argv[2]
    for area, title in (('town', 'Lucerne old town'), ('village', 'Andermatt (Urseren valley)')):
        sheet(f'recommended-{"town" if area == "town" else "valley"}.png', f'{title} · {rec}', ['highlight=water, dim=1', 'nothing highlighted'],
              [[(f'rec-{area}-water-z{z}', f'water · z{z}'), (f'rec-{area}-none-z{z}', f'none · z{z}')] for z in (13, 15, 16)])
