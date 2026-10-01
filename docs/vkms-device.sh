#!/usr/bin/env bash
set -euo pipefail

# Install this source as a root-owned helper before adding the sudoers rule:
#   sudo install -o root -g root -m 0755 docs/vkms-device.sh /usr/local/sbin/yserver-vkms-device
# Sudoers entry for the hardware acceptance test:
#   ariel_santangelo ALL=(root) NOPASSWD: /usr/local/sbin/yserver-vkms-device create, /usr/local/sbin/yserver-vkms-device destroy

readonly CONFIGFS_ROOT=/sys/kernel/config/vkms
readonly DEVICE_NAME=yserver-c0-3cii
readonly DEVICE_ROOT="$CONFIGFS_ROOT/$DEVICE_NAME"

if [[ $# -ne 1 || ( "$1" != create && "$1" != destroy ) ]]; then
    printf 'usage: %s {create|destroy}\n' "$0" >&2
    exit 2
fi
readonly operation=$1

if (( EUID != 0 )); then
    printf '%s requires root; install a root-owned copy and invoke it through sudo -n\n' "$0" >&2
    exit 1
fi

ensure_configfs_mount() {
    if ! mountpoint -q /sys/kernel/config; then
        mount -t configfs none /sys/kernel/config
    fi
}

link_once() {
    local target=$1
    local link=$2
    if [[ -L "$link" ]]; then
        if [[ $(readlink -- "$link") != "$target" ]]; then
            printf 'refusing unexpected symlink: %s\n' "$link" >&2
            exit 1
        fi
        return
    fi
    if [[ -e "$link" ]]; then
        printf 'refusing unexpected existing path: %s\n' "$link" >&2
        exit 1
    fi
    ln -s "$target" "$link"
}

destroy_device() {
    if [[ ! -d "$DEVICE_ROOT" ]]; then
        printf 'vkms %s is already absent\n' "$DEVICE_NAME"
        return
    fi

    if [[ -e "$DEVICE_ROOT/enabled" ]] && [[ $(<"$DEVICE_ROOT/enabled") == 1 ]]; then
        printf '0\n' >"$DEVICE_ROOT/enabled"
    fi

    shopt -s nullglob
    local link
    for link in "$DEVICE_ROOT"/planes/*/possible_crtcs/* \
        "$DEVICE_ROOT"/encoders/*/possible_crtcs/* \
        "$DEVICE_ROOT"/connectors/*/possible_encoders/*; do
        if [[ -L "$link" ]]; then
            rm -- "$link"
        fi
    done
    local item
    for item in "$DEVICE_ROOT"/planes/* "$DEVICE_ROOT"/crtcs/* \
        "$DEVICE_ROOT"/encoders/* "$DEVICE_ROOT"/connectors/*; do
        if [[ -d "$item" ]]; then
            rmdir -- "$item"
        fi
    done
    rmdir -- "$DEVICE_ROOT"
}

ensure_configfs_mount

if [[ ! -d "$CONFIGFS_ROOT" ]]; then
    if [[ "$operation" == destroy ]]; then
        printf 'vkms %s is already absent (vkms configfs is unavailable)\n' "$DEVICE_NAME"
        exit 0
    fi
    modprobe vkms
fi

if [[ ! -d "$CONFIGFS_ROOT" ]]; then
    printf 'vkms configfs is unavailable at %s\n' "$CONFIGFS_ROOT" >&2
    exit 1
fi

case "$operation" in
    destroy)
        destroy_device
        ;;
    create)
        if [[ -d "$DEVICE_ROOT" && -e "$DEVICE_ROOT/enabled" ]] \
            && [[ $(<"$DEVICE_ROOT/enabled") == 1 ]]; then
            printf 'vkms %s is already created and enabled\n' "$DEVICE_NAME"
            exit 0
        fi
        if [[ ! -d "$DEVICE_ROOT" ]]; then
            mkdir -- "$DEVICE_ROOT"
        fi
        mkdir -p -- "$DEVICE_ROOT/planes/plane0"
        mkdir -p -- "$DEVICE_ROOT/crtcs/crtc0"
        mkdir -p -- "$DEVICE_ROOT/encoders/encoder0"
        mkdir -p -- "$DEVICE_ROOT/connectors/connector0"
        printf '1\n' >"$DEVICE_ROOT/planes/plane0/type"
        printf '1\n' >"$DEVICE_ROOT/connectors/connector0/status"
        link_once "$DEVICE_ROOT/crtcs/crtc0" \
            "$DEVICE_ROOT/planes/plane0/possible_crtcs/crtc0"
        link_once "$DEVICE_ROOT/crtcs/crtc0" \
            "$DEVICE_ROOT/encoders/encoder0/possible_crtcs/crtc0"
        link_once "$DEVICE_ROOT/encoders/encoder0" \
            "$DEVICE_ROOT/connectors/connector0/possible_encoders/encoder0"
        if [[ $(<"$DEVICE_ROOT/enabled") != 1 ]]; then
            printf '1\n' >"$DEVICE_ROOT/enabled"
        fi
        printf 'vkms %s is created and enabled\n' "$DEVICE_NAME"
        ;;
esac
