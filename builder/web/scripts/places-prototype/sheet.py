"""python3 sheet.py OUT.png "ROW LABEL|file,file,file" ... --cols "today,groups,badges" [--width 900]"""
import sys
from PIL import Image, ImageDraw, ImageFont
D = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/places/'
args = sys.argv[1:]
width = int(args[args.index('--width') + 1]) if '--width' in args else 900
cols = args[args.index('--cols') + 1].split(',')
rows = [a for a in args[1:] if '|' in a]
def font(size):
    for path in ('/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf', '/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf'):
        try: return ImageFont.truetype(path, size)
        except OSError: pass
    return ImageFont.load_default()
big, small = font(26), font(20)
first = Image.open(D + rows[0].split('|')[1].split(',')[0])
height = round(first.height * width / first.width)
gap, head = 10, 44
sheet = Image.new('RGB', (len(cols) * (width + gap) + gap, head + len(rows) * (height + gap)), '#22211c')
draw = ImageDraw.Draw(sheet)
for c, col in enumerate(cols): draw.text((gap + c * (width + gap) + 8, 8), col, font=big, fill='#ffffff')
for r, row in enumerate(rows):
    label, files = row.split('|')
    for c, name in enumerate(files.split(',')):
        x, y = gap + c * (width + gap), head + r * (height + gap)
        if not name: continue
        sheet.paste(Image.open(D + name).convert('RGB').resize((width, height), Image.LANCZOS), (x, y))
        text = f'{label} · {cols[c]}'
        box = draw.textbbox((x + 10, y + 8), text, font=small)
        draw.rectangle((box[0] - 6, box[1] - 5, box[2] + 6, box[3] + 5), fill='#22211c')
        draw.text((x + 10, y + 8), text, font=small, fill='#ffffff')
sheet.save(args[0] if args[0].startswith('/') else D + args[0])
print(args[0], sheet.size)
