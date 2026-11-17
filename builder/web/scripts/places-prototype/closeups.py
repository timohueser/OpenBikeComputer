"""python3 closeups.py SHAPE,SHAPE: <shape>-closeup.png for each shape (light over dark, with captions) and closeups.png with all of them."""
import sys
from PIL import Image, ImageDraw, ImageFont
D = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/badges/'
SCALE, LEFT, HEAD, GAP = 6, 230, 96, 8
CAPTIONS = [(44, '1 kind'), (116, '2 kinds'), (198, '3 kinds'), (290, '4 kinds'), (400, '7 kinds'), (506, '1 kind, 3 places')]
def font(size, bold=True):
    return ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans%s.ttf' % ('-Bold' if bold else ''), size)
def block(shape):
    rows = [Image.open(f'{D}work/closeup-{shape}-{scheme}.png').convert('RGB') for scheme in ('light', 'dark')]
    width, height = rows[0].size
    image = Image.new('RGB', (LEFT + width, HEAD + 2 * height + GAP), '#22211c')
    draw = ImageDraw.Draw(image)
    draw.text((24, 22), shape, font=font(46), fill='#ffffff')
    for x, text in CAPTIONS + ([(584, '7 kinds, ?tilemax=4')] if shape == 'tile' else []):
        box = draw.textbbox((0, 0), text, font=font(28, False))
        draw.text((LEFT + (x - 4) * SCALE - (box[2] - box[0]) / 2, 44), text, font=font(28, False), fill='#d8d4c4')
    for index, (row, scheme) in enumerate(zip(rows, ('light', 'dark'))):
        y = HEAD + index * (height + GAP)
        image.paste(row, (LEFT, y))
        draw.text((24, y + height / 2 - 18), scheme, font=font(30, False), fill='#d8d4c4')
    image.save(f'{D}{shape}-closeup.png')
    return image
blocks = [block(shape) for shape in sys.argv[1].split(',')]
width = max(b.width for b in blocks)
sheet = Image.new('RGB', (width, sum(b.height for b in blocks) + 24 * (len(blocks) - 1)), '#22211c')
y = 0
for b in blocks:
    sheet.paste(b, (0, y)); y += b.height + 24
sheet.save(D + 'closeups.png')
print('closeups', sheet.size)
