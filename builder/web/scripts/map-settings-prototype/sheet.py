# The contact sheet: today, A and B side by side, open with the cycling network and the snow layer on.
import json
from PIL import Image, ImageDraw, ImageFont
OUT = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/mapsettings'
words = json.load(open(f'{OUT}/words.json'))
cells = [
    ('today-on', 'Today', f"{words['today-on']} words in the popup"),
    ('tiles-on', 'A · Picture tiles', f"{words['tiles-on']} words in the popup, {words['tiles-on, legend on the map']} in the legend on the map"),
    ('list-on', 'B · Trimmed list', f"{words['list-on']} words in the popup, the same legend on the map"),
]
W, H, GAP, PAD, HEAD = 1350, 975, 36, 48, 104
bold = ImageFont.truetype('/usr/share/fonts/opentype/inter/Inter-SemiBold.otf', 38)
plain = ImageFont.truetype('/usr/share/fonts/opentype/inter/Inter-Regular.otf', 26)
sheet = Image.new('RGB', (2 * PAD + 3 * W + 2 * GAP, 2 * PAD + HEAD + H), '#f4f2eb')
draw = ImageDraw.Draw(sheet)
for i, (name, title, note) in enumerate(cells):
    x = PAD + i * (W + GAP)
    draw.text((x, PAD), title, font=bold, fill='#1c1b14')
    draw.text((x, PAD + 52), note, font=plain, fill='#5c5a2e')
    sheet.paste(Image.open(f'{OUT}/{name}.png').convert('RGB').resize((W, H), Image.LANCZOS), (x, PAD + HEAD))
sheet.save(f'{OUT}/sheet.png')
print(sheet.size)
