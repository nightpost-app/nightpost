# dmgbuild settings for Nightpost-macos.dmg:
#   dmgbuild -s packaging/macos/dmg-settings.py -D app=<Nightpost.app> \
#     -D background=<background.tiff> Nightpost Nightpost-macos.dmg
# The background (background.py, then tiffutil -cathidpicheck for both
# sizes) has the arrow and the line of text drawn around the two icons
# placed here. Nothing goes through Finder, so it builds over SSH and in CI.
import os.path

app = defines["app"]
name = os.path.basename(app)

format = "ULFO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = os.path.join(app, "Contents", "Resources", "Nightpost.icns")

background = defines["background"]
window_rect = ((200, 140), (640, 400))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
icon_size = 128
text_size = 13
icon_locations = {name: (160, 190), "Applications": (480, 190)}
