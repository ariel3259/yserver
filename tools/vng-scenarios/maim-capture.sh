# Sourced by tools/vng-shot.sh INSIDE the guest (DISPLAY=:7, cwd = artifacts).
#
# Issue #135 (BergmannAtmet): "maim captures nothing but the background in
# Awesome. No panel. No windows, tiled, floating, or fullscreen. Passing
# -o/--noopengl doesn't make a difference."
#
# So: a real awesome session with a wallpaper, a wibar and a client window,
# then capture the screen four ways and compare each against yserver's own
# scanout dump, which is ground truth for what is actually on screen. If maim
# returns only the wallpaper while the scanout has the wibar and the window,
# the issue is reproduced and the difference between the capture routes says
# which read path is wrong.
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

# A client, so there is something on screen besides the wallpaper.
xterm -geometry 60x20+120+150 > xterm.log 2>&1 &
sleep 4

xwininfo -root -tree > tree.txt 2>&1 || true
maim maim-default.png            > maim-default.log 2>&1 || true
maim -o maim-noopengl.png        > maim-noopengl.log 2>&1 || true
import -window root import-root.png > import.log 2>&1 || true
sleep 1
