//! Per-category accounting of every `vkAllocateMemory` this process makes.
//!
//! `vram.rs` reports the driver's per-process heap usage; this ledger says
//! which of our allocations hold it. The difference is driver-internal
//! memory (pipelines, descriptor pools, command buffers).
//!
//! Keyed on the raw `vk::DeviceMemory` handle. [`allocate_memory`] and
//! [`free_memory`] are the only callers of the raw `ash::Device` methods;
//! `clippy.toml` disallows them everywhere else. Allocation is rare relative
//! to the cost of `vkAllocateMemory`, so one global mutex is fine.

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};

use ash::vk::{self, Handle};

/// What an allocation is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemCategory {
    /// Storage backing an X window.
    WindowStorage,
    /// Composite redirect backing pixmap of a redirected window.
    RedirectBacking,
    /// Client pixmap storage (and default for drawable storage).
    Pixmap,
    /// Storage parked in the pixmap pool, not owned by any drawable.
    PoolIdle,
    /// Client dma-buf imported via DRI3 (driver may not count it as ours).
    Dri3Import,
    /// Exportable (dma-buf) storage made for TFP / DRI3 export.
    TfpExport,
    /// A redirect backing promoted to exportable storage for TFP.
    RedirectExport,
    /// Scanout buffers and copied-scanout sources.
    Scanout,
    /// Per-op scratch images (copy, mask, dst readback, engine scratch).
    Scratch,
    /// Host-visible staging / upload / readback buffers.
    Staging,
    /// Glyph atlases and glyph caches.
    Glyph,
    /// Everything else (solid-colour images, gradients, diagnostics).
    Other,
}

impl MemCategory {
    /// Every category, in log order.
    pub const ALL: [Self; 12] = [
        Self::WindowStorage,
        Self::RedirectBacking,
        Self::Pixmap,
        Self::PoolIdle,
        Self::Dri3Import,
        Self::TfpExport,
        Self::RedirectExport,
        Self::Scanout,
        Self::Scratch,
        Self::Staging,
        Self::Glyph,
        Self::Other,
    ];

    /// Short name used in the telemetry line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::WindowStorage => "window",
            Self::RedirectBacking => "redirect",
            Self::Pixmap => "pixmap",
            Self::PoolIdle => "pool_idle",
            Self::Dri3Import => "dri3_import",
            Self::TfpExport => "tfp_export",
            Self::RedirectExport => "redirect_export",
            Self::Scanout => "scanout",
            Self::Scratch => "scratch",
            Self::Staging => "staging",
            Self::Glyph => "glyph",
            Self::Other => "other",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// Bytes and allocation count for one category on one side of the split.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub bytes: u64,
    pub count: u64,
}

/// Point-in-time totals, indexed by [`MemCategory::ALL`] order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MemSnapshot {
    pub device_local: [Tally; MemCategory::ALL.len()],
    pub host: [Tally; MemCategory::ALL.len()],
}

impl MemSnapshot {
    #[must_use]
    pub fn device_local(&self, category: MemCategory) -> Tally {
        self.device_local[category.index()]
    }

    #[must_use]
    pub fn host(&self, category: MemCategory) -> Tally {
        self.host[category.index()]
    }

    #[must_use]
    pub fn total_device_local_bytes(&self) -> u64 {
        self.device_local.iter().map(|t| t.bytes).sum()
    }

    #[must_use]
    pub fn total_host_bytes(&self) -> u64 {
        self.host.iter().map(|t| t.bytes).sum()
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    size: u64,
    category: MemCategory,
    device_local: bool,
}

#[derive(Default)]
struct Ledger {
    entries: HashMap<u64, Entry>,
    totals: MemSnapshot,
}

impl Ledger {
    fn side(&mut self, device_local: bool) -> &mut [Tally; MemCategory::ALL.len()] {
        if device_local {
            &mut self.totals.device_local
        } else {
            &mut self.totals.host
        }
    }

    fn add(&mut self, e: Entry) {
        let t = &mut self.side(e.device_local)[e.category.index()];
        t.bytes += e.size;
        t.count += 1;
    }

    fn sub(&mut self, e: Entry) {
        let t = &mut self.side(e.device_local)[e.category.index()];
        t.bytes = t.bytes.saturating_sub(e.size);
        t.count = t.count.saturating_sub(1);
    }

    fn alloc(&mut self, key: u64, e: Entry) {
        if let Some(old) = self.entries.insert(key, e) {
            self.sub(old);
        }
        self.add(e);
    }

    fn free(&mut self, key: u64) {
        if let Some(old) = self.entries.remove(&key) {
            self.sub(old);
        }
    }

    fn recategorise(&mut self, key: u64, category: MemCategory) {
        let Some(old) = self.entries.get(&key).copied() else {
            return;
        };
        if old.category == category {
            return;
        }
        self.sub(old);
        let new = Entry { category, ..old };
        self.entries.insert(key, new);
        self.add(new);
    }
}

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

fn ledger() -> MutexGuard<'static, Option<Ledger>> {
    LEDGER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn with_ledger<R>(f: impl FnOnce(&mut Ledger) -> R) -> R {
    let mut guard = ledger();
    f(guard.get_or_insert_with(Ledger::default))
}

/// Category for exportable memory that replaces storage currently
/// accounted as `current` (TFP / DRI3 promotion).
#[must_use]
pub fn export_category_for(current: Option<MemCategory>) -> MemCategory {
    match current {
        Some(MemCategory::RedirectBacking | MemCategory::RedirectExport) => {
            MemCategory::RedirectExport
        }
        _ => MemCategory::TfpExport,
    }
}

/// Whether memory type `type_index` carries `DEVICE_LOCAL`.
#[must_use]
pub fn is_device_local(props: &vk::PhysicalDeviceMemoryProperties, type_index: u32) -> bool {
    props
        .memory_types
        .get(type_index as usize)
        .is_some_and(|t| {
            t.property_flags
                .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
}

/// Record a successful allocation. Re-noting a live handle replaces it.
fn note_alloc(mem: vk::DeviceMemory, size: u64, category: MemCategory, device_local: bool) {
    if mem == vk::DeviceMemory::null() {
        return;
    }
    with_ledger(|l| {
        l.alloc(
            mem.as_raw(),
            Entry {
                size,
                category,
                device_local,
            },
        );
    });
}

/// `vkAllocateMemory`, accounted under `category`. `props` must be the
/// properties `info.memory_type_index` was chosen from.
///
/// # Errors
///
/// The `vkAllocateMemory` result.
pub(crate) fn allocate_memory(
    device: &ash::Device,
    info: &vk::MemoryAllocateInfo<'_>,
    category: MemCategory,
    props: &vk::PhysicalDeviceMemoryProperties,
) -> Result<vk::DeviceMemory, vk::Result> {
    // SAFETY: `info` is a valid allocate-info chain built by the caller.
    #[allow(clippy::disallowed_methods)]
    let mem = unsafe { device.allocate_memory(info, None)? };
    note_alloc(
        mem,
        info.allocation_size,
        category,
        is_device_local(props, info.memory_type_index),
    );
    Ok(mem)
}

/// `vkFreeMemory`, removing `mem` from the ledger first.
///
/// # Safety
///
/// Same contract as `ash::Device::free_memory`: `mem` was allocated from
/// `device` and nothing still uses it.
pub(crate) unsafe fn free_memory(device: &ash::Device, mem: vk::DeviceMemory) {
    note_free(mem);
    // SAFETY: forwarded from this function's contract.
    #[allow(clippy::disallowed_methods)]
    unsafe {
        device.free_memory(mem, None);
    }
}

/// Record a `free_memory`. Unknown handles are ignored.
fn note_free(mem: vk::DeviceMemory) {
    if mem == vk::DeviceMemory::null() {
        return;
    }
    with_ledger(|l| l.free(mem.as_raw()));
}

/// Move a live allocation to `category`. Unknown handles are ignored.
pub fn recategorise(mem: vk::DeviceMemory, category: MemCategory) {
    if mem == vk::DeviceMemory::null() {
        return;
    }
    with_ledger(|l| l.recategorise(mem.as_raw(), category));
}

/// The category a live handle is currently accounted under.
#[must_use]
pub fn category_of(mem: vk::DeviceMemory) -> Option<MemCategory> {
    ledger()
        .as_ref()
        .and_then(|l| l.entries.get(&mem.as_raw()).map(|e| e.category))
}

/// Size and category of a live handle.
pub(crate) fn entry_of(mem: vk::DeviceMemory) -> Option<(u64, MemCategory)> {
    ledger()
        .as_ref()
        .and_then(|l| l.entries.get(&mem.as_raw()).map(|e| (e.size, e.category)))
}

/// Current per-category totals.
#[must_use]
pub fn snapshot() -> MemSnapshot {
    ledger().as_ref().map(|l| l.totals).unwrap_or_default()
}

/// The `vram by use [1s]` telemetry line body. `heap_usage` is the
/// device-local `heapUsage` sample, when available.
#[must_use]
pub fn format_line(snap: &MemSnapshot, heap_usage: Option<u64>) -> String {
    use std::fmt::Write as _;
    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    let mut s = String::from("vram by use [1s]:");
    for c in MemCategory::ALL {
        let t = snap.device_local(c);
        let _ = write!(s, " {}={:.1}MiB/{}", c.label(), mib(t.bytes), t.count);
    }
    let tracked = snap.total_device_local_bytes();
    let _ = write!(
        s,
        " tracked_device_local={:.1}MiB host={:.1}MiB",
        mib(tracked),
        mib(snap.total_host_bytes())
    );
    if let Some(usage) = heap_usage {
        let untracked = i128::from(usage) - i128::from(tracked);
        let _ = write!(
            s,
            " untracked={:.1}MiB",
            untracked as f64 / (1024.0 * 1024.0)
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(size: u64, category: MemCategory, device_local: bool) -> Entry {
        Entry {
            size,
            category,
            device_local,
        }
    }

    #[test]
    fn alloc_free_totals() {
        let mut l = Ledger::default();
        l.alloc(1, e(100, MemCategory::Pixmap, true));
        l.alloc(2, e(50, MemCategory::Pixmap, true));
        l.alloc(3, e(7, MemCategory::Staging, false));
        let t = l.totals;
        assert_eq!(
            t.device_local(MemCategory::Pixmap),
            Tally {
                bytes: 150,
                count: 2
            }
        );
        assert_eq!(t.host(MemCategory::Staging), Tally { bytes: 7, count: 1 });
        assert_eq!(t.host(MemCategory::Pixmap), Tally::default());
        l.free(1);
        assert_eq!(
            l.totals.device_local(MemCategory::Pixmap),
            Tally {
                bytes: 50,
                count: 1
            }
        );
        l.free(2);
        l.free(3);
        assert_eq!(l.totals, MemSnapshot::default());
        assert!(l.entries.is_empty());
    }

    #[test]
    fn unknown_free_is_noop() {
        let mut l = Ledger::default();
        l.alloc(1, e(100, MemCategory::Glyph, true));
        l.free(42);
        l.free(1);
        l.free(1);
        assert_eq!(l.totals, MemSnapshot::default());
    }

    #[test]
    fn recategorise_moves_bytes() {
        let mut l = Ledger::default();
        l.alloc(1, e(100, MemCategory::Pixmap, true));
        l.recategorise(1, MemCategory::PoolIdle);
        assert_eq!(l.totals.device_local(MemCategory::Pixmap), Tally::default());
        assert_eq!(
            l.totals.device_local(MemCategory::PoolIdle),
            Tally {
                bytes: 100,
                count: 1
            }
        );
        l.recategorise(1, MemCategory::PoolIdle);
        l.recategorise(9, MemCategory::Other);
        assert_eq!(l.totals.total_device_local_bytes(), 100);
        l.free(1);
        assert_eq!(l.totals, MemSnapshot::default());
    }

    #[test]
    fn double_alloc_replaces() {
        let mut l = Ledger::default();
        l.alloc(1, e(100, MemCategory::Pixmap, true));
        l.alloc(1, e(30, MemCategory::Scratch, false));
        assert_eq!(l.totals.device_local(MemCategory::Pixmap), Tally::default());
        assert_eq!(
            l.totals.host(MemCategory::Scratch),
            Tally {
                bytes: 30,
                count: 1
            }
        );
        assert_eq!(l.entries.len(), 1);
    }

    #[test]
    fn format_line_reports_untracked() {
        let mut l = Ledger::default();
        l.alloc(1, e(3 << 20, MemCategory::WindowStorage, true));
        l.alloc(2, e(1 << 20, MemCategory::Staging, false));
        let line = format_line(&l.totals, Some(10 << 20));
        assert!(line.starts_with("vram by use [1s]: window=3.0MiB/1 redirect=0.0MiB/0"));
        assert!(line.contains(" tracked_device_local=3.0MiB host=1.0MiB untracked=7.0MiB"));
        assert!(!format_line(&l.totals, None).contains("untracked"));
    }

    #[test]
    fn promotion_keeps_redirect_backings_distinct() {
        assert_eq!(
            export_category_for(Some(MemCategory::RedirectBacking)),
            MemCategory::RedirectExport
        );
        assert_eq!(
            export_category_for(Some(MemCategory::RedirectExport)),
            MemCategory::RedirectExport
        );
        for c in [
            MemCategory::Pixmap,
            MemCategory::WindowStorage,
            MemCategory::PoolIdle,
            MemCategory::TfpExport,
        ] {
            assert_eq!(export_category_for(Some(c)), MemCategory::TfpExport);
        }
        assert_eq!(export_category_for(None), MemCategory::TfpExport);
        assert_eq!(MemCategory::RedirectExport.label(), "redirect_export");
    }

    #[test]
    fn global_api_roundtrip() {
        // A handle value no driver will hand out in this test process.
        let mem = vk::DeviceMemory::from_raw(0xdead_beef_0000_0001);
        note_alloc(mem, 4096, MemCategory::Scanout, true);
        assert_eq!(category_of(mem), Some(MemCategory::Scanout));
        recategorise(mem, MemCategory::Other);
        assert_eq!(category_of(mem), Some(MemCategory::Other));
        note_free(mem);
        assert_eq!(category_of(mem), None);
        note_free(mem);
    }
}
