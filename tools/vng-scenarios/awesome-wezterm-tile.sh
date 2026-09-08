# Sourced by tools/vng-shot.sh INSIDE the guest, with DISPLAY=:7 already
# exported and the artifact directory as cwd. Reproduces the #133 border
# scenario: awesome with a fat border, two wezterms, switched from
# floating to tiling.
#
# YS_BORDER_WIDTH (default 16) sets awesome's client border. The stock
# /etc/xdg/awesome/rc.lua bakes beautiful.border_width into its awful.rules
# table at load time, so the override has to be injected right after
# beautiful.init() rather than applied afterwards.
: "${YS_BORDER_WIDTH:=16}"
sed "/^beautiful.init(/a beautiful.border_width = $YS_BORDER_WIDTH" \
    /etc/xdg/awesome/rc.lua > rc.lua
awesome -c "$PWD/rc.lua" > awesome.log 2>&1 &
sleep 4

# --always-new-process on both: a bare second `wezterm start` hands the
# command to the first GUI instance, which makes the two windows one X
# client and muddies which client asked for which size.
wezterm start --always-new-process > wezterm-1.log 2>&1 &
sleep 4
wezterm start --always-new-process > wezterm-2.log 2>&1 &
sleep 4

# The pre-tile geometry, so a painted-region measurement in the dump can
# be checked against the size the client had BEFORE the resize.
xwininfo -root -tree > tree-floating.txt 2>&1 || true

# layouts[1] is floating, layouts[2] is tile; Mod4+space advances one.
xdotool key --clearmodifiers super+space
sleep 3

# YS_REFRESH=1 forces a full unmap/remap cycle of every client by switching
# to an empty tag and back. It separates "the visibility walk never claims
# this region" (survives the round trip) from "damage never repainted it"
# (healed by the round trip).
if [ "${YS_REFRESH:-0}" = 1 ]; then
    xdotool key --clearmodifiers super+2
    sleep 2
    xdotool key --clearmodifiers super+1
    sleep 3
fi

# Geometry the WM actually asked for, so a pixel measurement in the dump
# can be checked against a number instead of eyeballed.
wmctrl -l -G > windows.txt 2>&1 || true
# xwininfo is the one that reports Border width, which is the whole point.
xwininfo -root -tree > tree.txt 2>&1 || true
for id in $(wmctrl -l | awk '{print $1}'); do
    xwininfo -id "$id" >> windows-detail.txt 2>&1 || true
done
