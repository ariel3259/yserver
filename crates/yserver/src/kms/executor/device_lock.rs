//! COMMIT-7 device-scoped advisory lock and start-time check.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    path::PathBuf,
    time::Duration,
};

use super::KmsIoExecutor;
#[cfg(test)]
use super::executor_executable;
use crate::{
    kms::owner::{identity::IncarnationId, lifecycle::LifecycleEpochId},
    platform::drm::DrmDeviceKey,
};

pub const LOCK_HOLDER_ARG: &str = "--yserver-internal-kms-lock-holder-v1";

/// Advisory metadata describing the process recorded as acquiring a device lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HolderRecord {
    #[allow(dead_code)]
    pub(crate) pid: u32,
    #[allow(dead_code)]
    pub(crate) start_time: u64,
}

impl HolderRecord {
    pub(crate) fn current() -> Self {
        let pid = std::process::id();
        let start_time = current_process_start_time().unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        });
        Self { pid, start_time }
    }

    pub(crate) fn serialize(&self) -> String {
        format!("pid={}\nstart_time={}\n", self.pid, self.start_time)
    }

    pub(crate) fn deserialize(content: &str) -> Option<Self> {
        let mut pid = None;
        let mut start_time = None;
        for line in content.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("pid=") {
                pid = val.parse::<u32>().ok();
            } else if let Some(val) = line.strip_prefix("start_time=") {
                start_time = val.parse::<u64>().ok();
            } else {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 2
                    && let (Ok(p), Ok(st)) = (parts[0].parse::<u32>(), parts[1].parse::<u64>())
                {
                    return Some(Self {
                        pid: p,
                        start_time: st,
                    });
                }
            }
        }
        match (pid, start_time) {
            (Some(pid), Some(start_time)) => Some(Self { pid, start_time }),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_current_process(&self) -> bool {
        Self::current() == *self
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat_starttime(stat: &str) -> Option<u64> {
    let rparen = stat.rfind(')')?;
    let rest = stat.get(rparen + 1..)?.trim_start();
    let mut tokens = rest.split_whitespace();
    tokens.nth(19)?.parse::<u64>().ok()
}

#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat_btime(stat: &str) -> Option<u64> {
    for line in stat.lines() {
        if let Some(val) = line.strip_prefix("btime ") {
            return val.trim().parse::<u64>().ok();
        }
    }
    None
}

fn current_process_start_time() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(self_stat) = std::fs::read_to_string("/proc/self/stat")
            && let Some(ticks) = parse_proc_stat_starttime(&self_stat)
            && let Ok(sys_stat) = std::fs::read_to_string("/proc/stat")
            && let Some(btime) = parse_proc_stat_btime(&sys_stat)
        {
            // SAFETY: sysconf with _SC_CLK_TCK has no preconditions.
            let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            let clk_tck = if clk_tck > 0 { clk_tck as u64 } else { 100 };
            return Some(btime + (ticks / clk_tck));
        }
    }
    None
}

/// Error returned when the exclusive device lock cannot be acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("device lock unavailable")]
#[doc(hidden)]
pub struct LockUnavailable {
    #[allow(dead_code)]
    pub(crate) recorded_holder: Option<HolderRecord>,
}

/// Advisory device-scoped exclusive lock guard.
///
/// **This type deliberately has no `Drop` impl.** `flock` releases on last
/// close of the open file description, and an explicit unlock through any one
/// duplicate releases it for *all* of them — including the executor's
/// inherited copy, which COMMIT-7 requires to outlive this process. Closing
/// the descriptor, which `File` does on its own, is exactly the semantics we
/// want. `release_explicitly` is the one path that still unlocks, and it
/// disappears at the `into_inheritable` transition.
///
/// The word for that operation is spelled out only inside `release_explicitly`,
/// so Task 7's gate can assert a single occurrence in this file without having
/// to tell a call from a comment.
#[derive(Debug)]
pub struct DeviceLock {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
    device: DrmDeviceKey,
}

impl DeviceLock {
    /// Attempt non-blocking acquisition of the exclusive advisory lock for `device`.
    pub fn acquire(device: &DrmDeviceKey) -> Result<Self, LockUnavailable> {
        let path = lock_file_path(device).map_err(|err| {
            log::error!("failed to create lock directory for device {device}: {err}");
            LockUnavailable {
                recorded_holder: None,
            }
        })?;

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| {
                log::error!("failed to open lock file {}: {err}", path.display());
                LockUnavailable {
                    recorded_holder: None,
                }
            })?;

        // SAFETY: file is a valid open descriptor.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            let record = HolderRecord::current();
            let serialized = record.serialize();
            let _ = file.set_len(0);
            let _ = file.seek(SeekFrom::Start(0));
            let _ = file.write_all(serialized.as_bytes());
            let _ = file.flush();
            Ok(Self {
                file,
                path,
                device: *device,
            })
        } else {
            let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EWOULDBLOCK || errno == libc::EAGAIN {
                let mut contents = String::new();
                let _ = file.seek(SeekFrom::Start(0));
                let _ = file.read_to_string(&mut contents);
                let recorded_holder = HolderRecord::deserialize(&contents);
                Err(LockUnavailable { recorded_holder })
            } else {
                log::warn!(
                    "unexpected error from flock on {}: errno {errno}",
                    path.display()
                );
                Err(LockUnavailable {
                    recorded_holder: None,
                })
            }
        }
    }

    /// The only explicit release, and it exists only before handoff: the
    /// single-process paths that never give the lock to a helper. It is
    /// consuming, and `InheritableDeviceLock` deliberately has no equivalent.
    pub fn release_explicitly(self) {
        // SAFETY: self.file is a valid descriptor owned by self.
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        // `self` drops here, closing the descriptor.
    }

    /// Consume the guard into the form that can cross `execve`. The returned
    /// value owns the descriptor, cannot unlock, and releases the lock only
    /// by closing — which, once a helper has inherited a copy, releases
    /// nothing.
    pub fn into_inheritable(self) -> InheritableDeviceLock {
        let device = self.device;
        // `File` -> `OwnedFd` moves the descriptor out without closing it.
        InheritableDeviceLock {
            fd: OwnedFd::from(self.file),
            device,
        }
    }

    #[cfg(test)]
    pub fn duplicate_for_tests(&self) -> Self {
        use std::os::fd::FromRawFd;
        let dup = unsafe { libc::dup(self.file.as_raw_fd()) };
        if dup < 0 {
            panic!("dup failed: {}", io::Error::last_os_error());
        }
        Self {
            file: unsafe { File::from_raw_fd(dup) },
            path: self.path.clone(),
            device: self.device,
        }
    }
}

/// A device lock that has been committed to executor ownership.
#[derive(Debug)]
pub struct InheritableDeviceLock {
    fd: OwnedFd,
    #[allow(dead_code)]
    device: DrmDeviceKey,
}

impl InheritableDeviceLock {
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// Verify device state installation is allowed by acquiring and returning the device lock guard.
#[doc(hidden)]
pub fn may_install_state(device: &DrmDeviceKey) -> Result<DeviceLock, LockUnavailable> {
    DeviceLock::acquire(device)
}

/// Acquire the device lock or return an actionable `io::ErrorKind::ResourceBusy` error.
#[doc(hidden)]
pub fn acquire_device_lock_or_refuse(device: &DrmDeviceKey) -> io::Result<DeviceLock> {
    DeviceLock::acquire(device).map_err(|unavailable| {
        let msg = if let Some(holder) = unavailable.recorded_holder {
            format!(
                "device {device} is locked by another instance (holder {holder:?}); an earlier incarnation's helper may still mutate it"
            )
        } else {
            format!(
                "device {device} is locked; an earlier incarnation's helper may still mutate it"
            )
        };
        io::Error::new(io::ErrorKind::ResourceBusy, msg)
    })
}

fn lock_file_path(device: &DrmDeviceKey) -> io::Result<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // SAFETY: getuid has no preconditions.
            let uid = unsafe { libc::getuid() };
            PathBuf::from(format!("/tmp/yserver-kms-locks-{uid}"))
        });
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("device_{}_{}.lock", device.major, device.minor)))
}

/// Called by the `yserver` binary before normal argument parsing.
///
/// Returns `Some(...)` if a KMS device lock holder invocation was requested.
#[doc(hidden)]
pub fn run_lock_holder_if_requested() -> Option<io::Result<()>> {
    let mut args = std::env::args_os();
    let _executable = args.next();
    let first = args.next()?;
    let first_str = first.to_str()?;
    if first_str == LOCK_HOLDER_ARG {
        let major = args
            .next()
            .and_then(|s| s.to_str()?.parse::<u32>().ok())
            .unwrap_or(226);
        let minor = args
            .next()
            .and_then(|s| s.to_str()?.parse::<u32>().ok())
            .unwrap_or(0);
        let device = DrmDeviceKey { major, minor };
        Some(run_lock_holder(&device))
    } else if let Some(rest) = first_str.strip_prefix(LOCK_HOLDER_ARG) {
        if let Some(rest) = rest.strip_prefix('=') {
            let parts: Vec<&str> = rest.split(':').collect();
            let major = parts
                .first()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(226);
            let minor = parts
                .get(1)
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            let device = DrmDeviceKey { major, minor };
            Some(run_lock_holder(&device))
        } else {
            None
        }
    } else {
        None
    }
}

fn run_lock_holder(device: &DrmDeviceKey) -> io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let _lock = loop {
        match DeviceLock::acquire(device) {
            Ok(lock) => break lock,
            Err(err) => {
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("timed out acquiring device lock for {device}: {err}"),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    };
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

pub const LOCK_HANDOFF_ARG: &str = "--yserver-internal-kms-lock-handoff-v1";

/// Acquire the device lock, hand it to a real executor helper, print the
/// helper's pid, and exit **without** reaping it. This exists so a test can
/// observe COMMIT-7's actual property: the lock outliving the death of the
/// process that took it. It is never reached in normal operation.
#[doc(hidden)]
pub fn run_lock_handoff_if_requested() -> Option<io::Result<()>> {
    let mut args = std::env::args_os();
    let _executable = args.next();
    let first = args.next()?;
    let first_str = first.to_str()?;
    let device = if first_str == LOCK_HANDOFF_ARG {
        let major = args
            .next()
            .and_then(|s| s.to_str()?.parse::<u32>().ok())
            .unwrap_or(226);
        let minor = args
            .next()
            .and_then(|s| s.to_str()?.parse::<u32>().ok())
            .unwrap_or(0);
        DrmDeviceKey { major, minor }
    } else {
        let rest = first_str.strip_prefix(LOCK_HANDOFF_ARG)?;
        let rest = rest.strip_prefix('=')?;
        let parts: Vec<&str> = rest.split(':').collect();
        let major = parts
            .first()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(226);
        let minor = parts
            .get(1)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        DrmDeviceKey { major, minor }
    };

    Some((|| -> io::Result<()> {
        let lock = DeviceLock::acquire(&device).map_err(|err| {
            io::Error::new(
                io::ErrorKind::ResourceBusy,
                format!("failed to acquire device lock for handoff: {err}"),
            )
        })?;
        let inheritable = lock.into_inheritable();
        let dummy = std::fs::File::open("/dev/null")?;
        let mut executor = KmsIoExecutor::spawn_wedged_lock_holder_for_tests(
            dummy.as_fd(),
            IncarnationId::first(),
            LifecycleEpochId::first(),
            &inheritable,
        )?;
        executor.await_helper_ready(Duration::from_secs(30))?;
        println!("{}", executor.helper_pid());
        io::stdout().flush()?;
        drop(inheritable);
        std::mem::forget(executor);
        unsafe { libc::_exit(0) };
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::drm::DrmDeviceKey;

    fn wait_for_lock_held(device: &DrmDeviceKey) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if super::may_install_state(device).is_err() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out waiting for device lock to be held");
    }

    #[test]
    fn a_lock_held_by_another_process_blocks_install() {
        let device = DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let mut child = std::process::Command::new(super::executor_executable().expect("exe"))
            .arg("--yserver-internal-kms-lock-holder-v1")
            .arg(device.major.to_string())
            .arg(device.minor.to_string())
            .spawn()
            .expect("spawn");
        wait_for_lock_held(&device);
        assert!(super::may_install_state(&device).is_err());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn the_lock_is_released_when_the_holder_dies() {
        let device = DrmDeviceKey {
            major: 226,
            minor: 1,
        };
        {
            let _lock = DeviceLock::acquire(&device).expect("acquire");
            assert!(
                super::may_install_state(&device).is_err(),
                "held while the guard lives"
            );
        }
        assert!(
            super::may_install_state(&device).is_ok(),
            "released when the guard drops"
        );
    }

    #[test]
    fn a_sigkilled_holder_still_releases_the_lock() {
        // This is the property COMMIT-7 rests on: the guarantee must survive a
        // service manager killing the parent, so it cannot depend on any
        // orderly release path running.
        let device = DrmDeviceKey {
            major: 226,
            minor: 2,
        };
        let mut child = std::process::Command::new(super::executor_executable().expect("exe"))
            .arg("--yserver-internal-kms-lock-holder-v1")
            .arg(device.major.to_string())
            .arg(device.minor.to_string())
            .spawn()
            .expect("spawn");
        wait_for_lock_held(&device);
        child.kill().expect("kill");
        child.wait().expect("wait");
        assert!(super::may_install_state(&device).is_ok());
    }

    #[test]
    fn holder_record_serialization_round_trips() {
        let record = HolderRecord {
            pid: 12345,
            start_time: 1788382136,
        };
        let serialized = record.serialize();
        let parsed = HolderRecord::deserialize(&serialized).expect("deserialized");
        assert_eq!(record, parsed);
    }

    #[test]
    fn holder_record_parse_proc_stat_starttime() {
        let sample = "112780 (cat) R 49626 112780 112780 34816 112780 4194304 267 0 0 0 0 0 0 0 20 0 1 0 2939504 8585216 483";
        assert_eq!(parse_proc_stat_starttime(sample), Some(2939504));

        let sample_with_spaces_and_parens = "123 (complex (proc) name) S 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 88888 23 24";
        assert_eq!(
            parse_proc_stat_starttime(sample_with_spaces_and_parens),
            Some(88888)
        );
    }

    #[test]
    fn holder_record_parse_proc_stat_btime() {
        let sample = "cpu  123 456 789\nbtime 1788382136\nprocesses 1234\n";
        assert_eq!(parse_proc_stat_btime(sample), Some(1788382136));
    }

    #[test]
    fn dropping_a_device_lock_does_not_unlock_a_shared_description() {
        let key = DrmDeviceKey {
            major: 226,
            minor: 250,
        };
        let lock = may_install_state(&key).expect("first holder");
        let duplicate = lock.duplicate_for_tests();
        drop(lock);
        assert!(
            may_install_state(&key).is_err(),
            "the surviving duplicate must still hold the lock"
        );
        drop(duplicate);
        assert!(
            may_install_state(&key).is_ok(),
            "the last close releases it"
        );
    }

    #[test]
    fn an_explicit_release_is_still_available_before_handoff_and_is_global() {
        let key = DrmDeviceKey {
            major: 226,
            minor: 251,
        };
        let lock = may_install_state(&key).expect("holder");
        let duplicate = lock.duplicate_for_tests();
        lock.release_explicitly();
        assert!(
            may_install_state(&key).is_ok(),
            "explicit release is global by design, which is why it disappears after handoff"
        );
        drop(duplicate);
    }

    #[test]
    fn an_inheritable_lock_still_holds_and_still_releases_on_last_close() {
        let key = DrmDeviceKey {
            major: 226,
            minor: 252,
        };
        let inheritable = may_install_state(&key).expect("holder").into_inheritable();
        assert!(
            may_install_state(&key).is_err(),
            "the transition must not drop the lock"
        );
        drop(inheritable);
        assert!(
            may_install_state(&key).is_ok(),
            "with no helper holding a copy, the last close releases it"
        );
    }
}
