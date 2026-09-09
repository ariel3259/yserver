# Sourced by tools/vng-shot.sh INSIDE the guest (DISPLAY=:7, cwd = artifacts).
#
# Issue #135: trace ONLY maim, not the whole session. awesome connects
# directly to :7; x11trace proxies :8 -> :7 and maim runs against :8, so the
# trace holds maim's requests and nothing else.
#
# maim's imports say what to look for (`nm -D /usr/bin/maim`):
#   XCompositeRedirectSubwindows, XRenderCreatePicture / XRenderComposite,
#   XFixesCreateRegionFromWindow / SetPictureClipRegion / Subtract / Union /
#   Translate, XFixesGetCursorImage, and XGetImage LAST.
# So it redirects the root's subwindows, assembles the screen itself with
# RENDER under XFixes clips, and reads back its own pixmap — nothing like
# `import`, which XGetImages the root and is pixel-exact here.
set -u
: "${YS_RC:=/home/jos/.config/awesome/rc.lua}"
[ -r "$YS_RC" ] || YS_RC=/etc/xdg/awesome/rc.lua
cp "$YS_RC" rc.lua
configured=$(sed -n 's/^beautiful.wallpaper *= *"\(.*\)".*/\1/p' rc.lua | head -1)
if [ -z "$configured" ] || [ ! -r "$configured" ]; then
    magick -size 36x25 xc: +noise Random -scale 1280x800 wallpaper.png 2> magick.log
    sed -i "s|^beautiful.wallpaper .*=.*|beautiful.wallpaper = \"$PWD/wallpaper.png\"|" rc.lua
fi
awesome -c "$PWD/rc.lua" > awesome.log 2>&1 &
sleep 8
xterm -geometry 60x20+120+150 > xterm.log 2>&1 &
sleep 4

x11trace -d :7 -D :8 -n -o maim.xtrace > x11trace.log 2>&1 &
trace_pid=$!
sleep 2
DISPLAY=:8 maim maim-traced.png > maim.log 2>&1 || true
sleep 2
kill -TERM $trace_pid 2>/dev/null || true
sleep 1

# Same capture untraced, so a tracer artefact is distinguishable.
maim maim-untraced.png > maim2.log 2>&1 || true
import -window root import-root.png > import.log 2>&1 || true
xwininfo -root -tree > tree.txt 2>&1 || true
