# Draws the .dmg window's background, at 1x and 2x (packaging/macos/
# background.png and background@2x.png), in the app's day colours: Finder
# writes the icons' names in black whatever the system's appearance. The
# icons go at x = 160 and 480, y = 190 (dmg-settings.py): the arrow and the
# line of text are drawn around those. Needs Pillow; run from the repo root.
import math

from PIL import Image, ImageDraw, ImageFont

W, H = 640, 400
GROUND, MAIN, MUTED = (244, 245, 247), (31, 95, 191), (100, 107, 120)


def draw(scale):
    s = lambda v: v * scale
    im = Image.new("RGB", (s(W), s(H)), GROUND)
    d = ImageDraw.Draw(im)
    # A shallow arc from one icon to the other, ending in an open head along
    # its last stretch. Round ends: Pillow draws lines with square ones.
    arc = lambda t: (252 + 136 * t, 184 - 104 * t * (1 - t))
    (xe, ye), (xb, yb) = arc(1), arc(0.96)
    a = math.atan2(ye - yb, xe - xb)
    wing = lambda da: (xe - 13 * math.cos(a + da), ye - 13 * math.sin(a + da))
    width = 3
    for line in ([arc(i / 60) for i in range(61)], [wing(0.6), (xe, ye), wing(-0.6)]):
        d.line([(s(x), s(y)) for x, y in line], fill=MAIN, width=s(width), joint="curve")
        for x, y in (line[0], line[-1]):
            r = s(width) / 2
            d.ellipse((s(x) - r, s(y) - r, s(x) + r, s(y) + r), fill=MAIN)
    font = ImageFont.truetype("crates/gui/fonts/SpaceGrotesk.ttf", s(15))
    try:
        font.set_variation_by_axes([500])
    except OSError:
        pass
    text = "Drag Nightpost into Applications"
    d.text(((s(W) - d.textlength(text, font=font)) / 2, s(318)), text, font=font, fill=MUTED)
    return im


# Drawn four times larger and scaled down: Pillow does not smooth lines.
for scale, name in ((1, "background.png"), (2, "background@2x.png")):
    draw(scale * 4).resize((W * scale, H * scale), Image.LANCZOS).save(f"packaging/macos/{name}")
