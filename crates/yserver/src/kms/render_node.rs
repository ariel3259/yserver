//! Locate and open a DRM render node sibling of the scanout card fd.
//!
//! Phase 4.2 design §3.2: open at backend init via sysfs walk
//! (`/sys/dev/char/<major>:<minor>/device/drm/renderD*`); fall back to
//! a userspace enumeration of `/dev/dri/renderD*` whose parent device
//! matches the card's parent device. We deliberately do **not**
//! hardcode `/dev/dri/renderD128` — on multi-GPU hosts that selects
//! the wrong device. Parent matching cannot fire at all on split
//! display/render SoCs (Apple Silicon under Asahi), so a lone render
//! node is accepted as the answer; see `select_render_node`.

use std::{
    env,
    fmt::Write,
    fs,
    io::{self, ErrorKind},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::fs::MetadataExt,
    },
    path::{Path, PathBuf},
};

const RENDER_NODE_ENV: &str = "YSERVER_DRI_RENDER_NODE";

/// Resolve the render node sibling of `card_fd`, returning a freshly
/// opened fd and the filesystem path. The path is retained so callers
/// (`Backend::dri3_open`) can re-open a new kernel struct file for
/// each client rather than `dup()`-ing a shared one — libdrm_amdgpu
/// keeps per-struct-file GEM-handle namespaces, and dup'ing across
/// clients makes them collide and crash inside `amdgpu_winsys_create`.
pub fn open_for_card<F: AsFd>(card_fd: F) -> io::Result<(OwnedFd, PathBuf)> {
    if let Some(path) = explicit_render_node_path() {
        let fd = open_cloexec(&path)?;
        return Ok((fd, path));
    }

    let fd = card_fd.as_fd();
    let stat = fstat_rdev(fd)?;
    let major = libc_major(stat);
    let minor = libc_minor(stat);

    if let Some(path) = render_node_path_via_sysfs((major, minor))? {
        let fd = open_cloexec(&path)?;
        return Ok((fd, path));
    }

    if let Some(path) = render_node_path_via_dev_walk((major, minor))? {
        let fd = open_cloexec(&path)?;
        return Ok((fd, path));
    }

    Err(io::Error::other(format!(
        "no DRM render node found for card with rdev {major}:{minor} \
         (sysfs walk and /dev/dri scan both yielded nothing). \
         Override with {RENDER_NODE_ENV}=/dev/dri/renderDN if needed."
    )))
}

/// Resolve a fresh `O_RDWR | O_CLOEXEC` fd for an already-known render
/// node path. Used by `Backend::dri3_open` so each DRI3 client gets
/// its own kernel struct file (see `open_for_card` doc).
pub fn open_fresh(path: &Path) -> io::Result<OwnedFd> {
    open_cloexec(path)
}

/// Resolve `(major, minor)` of a card device to the sibling render
/// node path, by reading `/sys/dev/char/<major>:<minor>/device/drm/`.
pub fn render_node_path_via_sysfs(card_dev: (u32, u32)) -> io::Result<Option<PathBuf>> {
    let dir = PathBuf::from(format!(
        "/sys/dev/char/{}:{}/device/drm",
        card_dev.0, card_dev.1
    ));
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("renderD") {
            let dev_path = PathBuf::from("/dev/dri").join(&*name_str);
            return Ok(Some(dev_path));
        }
    }
    Ok(None)
}

fn explicit_render_node_path() -> Option<PathBuf> {
    let raw = env::var_os(RENDER_NODE_ENV)?;
    let path = PathBuf::from(raw);
    if path.as_os_str().is_empty() {
        return None;
    }
    Some(path)
}

fn render_node_path_via_dev_walk(card_dev: (u32, u32)) -> io::Result<Option<PathBuf>> {
    let entries = match fs::read_dir("/dev/dri") {
        Ok(e) => e,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();
        if name_str.starts_with("renderD") {
            candidates.push(entry.path());
        }
    }
    candidates.sort();

    let card_parent = sysfs_parent_for(card_dev).ok();
    let resolved: Vec<(PathBuf, Option<PathBuf>)> = candidates
        .into_iter()
        .map(|cand| {
            let parent = fs::metadata(&cand).ok().and_then(|meta| {
                let cand_dev = (libc_major(meta.rdev()), libc_minor(meta.rdev()));
                sysfs_parent_for(cand_dev).ok()
            });
            (cand, parent)
        })
        .collect();

    select_render_node(&resolved, card_parent.as_deref(), card_dev)
}

/// Pick a render node from `candidates` (each paired with its resolved
/// sysfs parent, sorted by path).
///
/// Preference order:
/// 1. A candidate whose sysfs parent is the card's own parent — the
///    normal desktop case (amdgpu, i915, nouveau), and the only rule
///    that stays correct on multi-GPU hosts.
/// 2. Otherwise, the sole candidate if there is exactly one. Split
///    display/render SoCs put scanout and GPU on *different* devices —
///    on Apple Silicon under Asahi the scanout card hangs off
///    `soc:display-subsystem` while renderD128 hangs off `<addr>.gpu`,
///    so rule 1 can never fire and refusing here would silently drop
///    DRI3 (clients fall back to llvmpipe).
/// 3. Several candidates and none matching: ambiguous, so error out
///    with the env override rather than guess a GPU.
fn select_render_node(
    candidates: &[(PathBuf, Option<PathBuf>)],
    card_parent: Option<&Path>,
    card_dev: (u32, u32),
) -> io::Result<Option<PathBuf>> {
    if candidates.is_empty() {
        return Ok(None);
    }

    if let Some(card_parent) = card_parent
        && let Some((cand, _)) = candidates
            .iter()
            .find(|(_, parent)| parent.as_deref() == Some(card_parent))
    {
        return Ok(Some(cand.clone()));
    }

    match candidates {
        [(only, _)] => Ok(Some(only.clone())),
        _ => {
            let why = if card_parent.is_some() {
                "none is a sibling of"
            } else {
                "no sysfs parent data is available to match them to"
            };
            Err(io::Error::other(format!(
                "multiple DRM render nodes found but {why} card rdev {}:{}: {}. \
                 Set {RENDER_NODE_ENV}=/dev/dri/renderDN.",
                card_dev.0,
                card_dev.1,
                display_paths(candidates)
            )))
        }
    }
}

fn sysfs_parent_for(dev: (u32, u32)) -> io::Result<PathBuf> {
    let link = PathBuf::from(format!("/sys/dev/char/{}:{}/device", dev.0, dev.1));
    fs::canonicalize(&link)
}

fn fstat_rdev(fd: BorrowedFd<'_>) -> io::Result<u64> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    #[allow(clippy::useless_conversion)]
    Ok(u64::from(stat.st_rdev))
}

#[allow(clippy::cast_possible_truncation)]
fn libc_major(rdev: u64) -> u32 {
    libc::major(rdev) as u32
}

#[allow(clippy::cast_possible_truncation)]
fn libc_minor(rdev: u64) -> u32 {
    libc::minor(rdev) as u32
}

fn open_cloexec(path: &Path) -> io::Result<OwnedFd> {
    use std::os::fd::FromRawFd;
    let cstr = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::other(format!("path contains nul byte: {}", path.display())))?;
    let raw = unsafe { libc::open(cstr.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn display_paths(candidates: &[(PathBuf, Option<PathBuf>)]) -> String {
    let mut out = String::new();
    for (idx, (path, _)) in candidates.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        let _ = write!(&mut out, "{}", path.display());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(path: &str, parent: Option<&str>) -> (PathBuf, Option<PathBuf>) {
        (PathBuf::from(path), parent.map(PathBuf::from))
    }

    #[test]
    fn select_render_node_prefers_the_cards_own_sibling() {
        let cands = [
            cand(
                "/dev/dri/renderD128",
                Some("/sys/devices/pci0000:00/0000:00:02.0"),
            ),
            cand(
                "/dev/dri/renderD129",
                Some("/sys/devices/pci0000:00/0000:03:00.0"),
            ),
        ];
        let picked = select_render_node(
            &cands,
            Some(Path::new("/sys/devices/pci0000:00/0000:03:00.0")),
            (226, 1),
        );
        assert_eq!(picked.unwrap(), Some(PathBuf::from("/dev/dri/renderD129")));
    }

    /// Apple Silicon under Asahi: the scanout card hangs off
    /// `soc:display-subsystem`, the render node off the GPU node, so no
    /// parent match exists. The lone render node must still be used —
    /// refusing here drops DRI3 and pushes every GL client to llvmpipe.
    #[test]
    fn select_render_node_accepts_lone_node_on_split_display_render_soc() {
        let cands = [cand(
            "/dev/dri/renderD128",
            Some("/sys/devices/platform/soc/206400000.gpu"),
        )];
        let picked = select_render_node(
            &cands,
            Some(Path::new("/sys/devices/platform/soc/soc:display-subsystem")),
            (226, 2),
        );
        assert_eq!(picked.unwrap(), Some(PathBuf::from("/dev/dri/renderD128")));
    }

    #[test]
    fn select_render_node_errors_when_several_nodes_and_none_match() {
        let cands = [
            cand("/dev/dri/renderD128", Some("/sys/devices/gpu-a")),
            cand("/dev/dri/renderD129", Some("/sys/devices/gpu-b")),
        ];
        let err = select_render_node(&cands, Some(Path::new("/sys/devices/display")), (226, 2))
            .expect_err("ambiguous selection must not guess");
        assert!(err.to_string().contains(RENDER_NODE_ENV));
    }

    /// FreeBSD has no `/sys`, so `sysfs_parent_for` fails for every node
    /// and `card_parent` is `None`. A lone render node is still the
    /// answer there (this is what bbc9d30 fixed).
    #[test]
    fn select_render_node_accepts_lone_node_without_sysfs() {
        let cands = [cand("/dev/dri/renderD128", None)];
        let picked = select_render_node(&cands, None, (226, 0));
        assert_eq!(picked.unwrap(), Some(PathBuf::from("/dev/dri/renderD128")));
    }

    /// Still no `/sys`, but several render nodes: nothing distinguishes
    /// them, so refuse rather than guess a GPU.
    #[test]
    fn select_render_node_errors_without_sysfs_when_ambiguous() {
        let cands = [
            cand("/dev/dri/renderD128", None),
            cand("/dev/dri/renderD129", None),
        ];
        let err = select_render_node(&cands, None, (226, 0))
            .expect_err("ambiguous selection must not guess");
        let msg = err.to_string();
        assert!(msg.contains("no sysfs parent data"), "{msg}");
        assert!(msg.contains(RENDER_NODE_ENV), "{msg}");
    }

    #[test]
    fn select_render_node_returns_none_without_candidates() {
        assert_eq!(select_render_node(&[], None, (226, 0)).unwrap(), None);
    }

    /// On a host with exactly one render node, *every* card node must
    /// resolve to it — including cards that are not its sysfs sibling.
    /// This is the Asahi shape (`card2` on `soc:display-subsystem`,
    /// `renderD128` on `<addr>.gpu`); returning `None` there costs DRI3
    /// and drops every GL client to llvmpipe.
    #[test]
    fn dev_walk_resolves_every_card_when_host_has_one_render_node() {
        let Ok(entries) = fs::read_dir("/dev/dri") else {
            return;
        };
        let mut cards: Vec<(u32, u32)> = Vec::new();
        let mut render_nodes = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            let rdev = meta.rdev();
            if name.starts_with("renderD") {
                render_nodes += 1;
            } else if name.starts_with("card") {
                cards.push((libc_major(rdev), libc_minor(rdev)));
            }
        }
        if render_nodes != 1 {
            return;
        }
        for card in cards {
            let picked = render_node_path_via_dev_walk(card);
            assert!(
                matches!(&picked, Ok(Some(p)) if p.to_string_lossy().starts_with("/dev/dri/renderD")),
                "card {}:{} resolved to {picked:?}, want the host's only render node",
                card.0,
                card.1,
            );
        }
    }

    #[test]
    fn render_node_path_via_sysfs_returns_none_for_absurd_dev() {
        let res = render_node_path_via_sysfs((9999, 9999));
        assert!(matches!(res, Ok(None)));
    }

    #[test]
    fn render_node_path_via_sysfs_smoke() {
        let Ok(entries) = fs::read_dir("/dev/dri") else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("card")
                && let Ok(meta) = entry.metadata()
            {
                let rdev = meta.rdev();
                let dev = (libc_major(rdev), libc_minor(rdev));
                if let Ok(Some(path)) = render_node_path_via_sysfs(dev) {
                    let s = path.to_string_lossy();
                    assert!(
                        s.starts_with("/dev/dri/renderD"),
                        "expected renderD* path, got {s:?}"
                    );
                    return;
                }
            }
        }
    }

    #[test]
    fn open_cloexec_fails_for_missing_path() {
        let path = std::env::temp_dir().join("yserver-render-node-test-nonexistent");
        let _ = fs::remove_file(&path);
        let res = open_cloexec(&path);
        assert!(res.is_err());
    }

    #[test]
    fn libc_major_minor_round_trip() {
        let dev = libc::makedev(226, 128);
        assert_eq!(libc_major(dev), 226);
        assert_eq!(libc_minor(dev), 128);
    }
}
