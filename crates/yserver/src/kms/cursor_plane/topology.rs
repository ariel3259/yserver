/// Driver names whose cursor-plane visibility depends on client hotspot support.
pub(crate) fn is_virtualized_cursor_driver(driver_name: &str) -> bool {
    matches!(driver_name, "virtio_gpu" | "vmwgfx" | "qxl" | "vboxvideo")
}
