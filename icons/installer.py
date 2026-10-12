# Draws the installers' icon: a parcel tied with string, in the app's night
# blue and teal, so the .dmg and the Windows setup do not pass for the app
# itself. Writes installer.png (1024), installer.icns and installer.ico
# next to this file. Needs Pillow.
import os

from PIL import Image, ImageDraw, ImageFilter

S = 4  # drawn 4 times larger, then reduced: Pillow does not antialias
TOP, LEFT, RIGHT = (60, 84, 132), (38, 53, 88), (25, 36, 60)
# The string, and where it runs down the side in shadow.
STRING, SHADED, EDGE = (92, 200, 194), (62, 150, 146), (98, 124, 176)


def mid(a, b, t=0.5):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


def draw():
    im = Image.new("RGBA", (1024 * S, 1024 * S), (0, 0, 0, 0))
    s = lambda p: (p[0] * S, p[1] * S)
    # The box, seen from above a corner.
    t, l, r, f = (512, 190), (172, 380), (852, 380), (512, 570)
    h = 360
    down = lambda p: (p[0], p[1] + h)

    # Its shadow on the ground.
    shadow = Image.new("RGBA", im.size, (0, 0, 0, 0))
    ImageDraw.Draw(shadow).ellipse([s((190, 840)), s((834, 960))], fill=(0, 0, 0, 110))
    im.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(28 * S)))

    d = ImageDraw.Draw(im)
    d.polygon([s(p) for p in (l, f, down(f), down(l))], fill=LEFT)
    d.polygon([s(p) for p in (f, r, down(r), down(f))], fill=RIGHT)
    d.polygon([s(p) for p in (t, r, f, l)], fill=TOP)
    # The lid's edges catch the light.
    d.line([s(p) for p in (l, f, r)], fill=EDGE, width=6 * S, joint="curve")

    # The string: once round each way, crossing on the lid.
    w = 34 * S
    for a, b, c, e, side in ((t, l, r, f, SHADED), (t, r, l, f, STRING)):
        start, end = mid(a, b), mid(c, e)
        d.line([s(start), s(end)], fill=STRING, width=w)
        d.line([s(end), s(down(end))], fill=side, width=w)
    # The knot.
    k = mid(t, f)
    d.ellipse([s((k[0] - 40, k[1] - 26)), s((k[0] + 40, k[1] + 26))], fill=STRING)
    return im.resize((1024, 1024), Image.LANCZOS)


here = os.path.dirname(os.path.abspath(__file__))
icon = draw()
icon.save(os.path.join(here, "installer.png"))
icon.save(os.path.join(here, "installer.icns"))
icon.save(os.path.join(here, "installer.ico"), sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
