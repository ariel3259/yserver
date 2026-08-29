#!/usr/bin/env bash

set -euo pipefail

gsettings set org.mate.Marco.general compositing-manager false
value="$(gsettings get org.mate.Marco.general compositing-manager)"
if [ "${value}" != "false" ]; then
    echo "ERROR: Marco compositing-manager remained ${value}; expected false."
    exit 1
fi
echo "MATE direct-scanout validation: Marco compositing-manager=false"

exec mate-session
