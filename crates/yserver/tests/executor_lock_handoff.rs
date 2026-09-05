use std::{
    os::fd::AsFd,
    time::{Duration, Instant},
};
use yserver::{
    kms::{
        executor::{
            KmsIoExecutor, LOCK_HANDOFF_ARG,
            device_lock::{acquire_device_lock_or_refuse, may_install_state},
            test_support::{self, executor_executable},
        },
        owner::{identity::IncarnationId, lifecycle::LifecycleEpochId},
    },
    platform::drm::DrmDeviceKey,
};

#[test]
fn a_failed_spawn_leaves_the_lock_with_the_caller_who_releases_it() {
    let key = DrmDeviceKey {
        major: 226,
        minor: 253,
    };
    let inheritable = may_install_state(&key).expect("holder").into_inheritable();
    let dummy = std::fs::File::open("/dev/null").expect("dev null");
    let err = KmsIoExecutor::spawn_with_device_lock_at(
        std::path::Path::new("/nonexistent/yserver-executor"),
        dummy.as_fd(),
        IncarnationId::first(),
        LifecycleEpochId::first(),
        &inheritable,
    )
    .expect_err("spawning a nonexistent executable must fail");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    assert!(
        may_install_state(&key).is_err(),
        "the caller still holds it after a failed spawn"
    );
    drop(inheritable);
    assert!(
        may_install_state(&key).is_ok(),
        "and releases it by dropping it"
    );
}

#[test]
fn the_helper_holds_the_lock_after_the_parent_drops_its_copy() {
    let key = DrmDeviceKey {
        major: 226,
        minor: 254,
    };
    let inheritable = may_install_state(&key).expect("holder").into_inheritable();
    let dummy = std::fs::File::open("/dev/null").expect("dev null");
    let mut executor = KmsIoExecutor::spawn_with_device_lock(
        dummy.as_fd(),
        IncarnationId::first(),
        LifecycleEpochId::first(),
        &inheritable,
    )
    .expect("spawn");
    // The readiness reply is what proves the helper reached its serve loop
    // with LOCK_FD adopted. Dropping the parent copy before that could
    // release the lock if the exec had failed.
    executor
        .await_helper_ready(Duration::from_secs(30))
        .expect("ready");
    drop(inheritable);
    assert!(
        may_install_state(&key).is_err(),
        "the helper's inherited descriptor must still hold the lock"
    );
    test_support::kill_and_reap(&mut executor);
    assert!(
        may_install_state(&key).is_ok(),
        "released only by the helper's death"
    );
}

#[test]
fn the_lock_survives_the_death_of_the_process_that_acquired_it() {
    let key = DrmDeviceKey {
        major: 226,
        minor: 255,
    };
    let output = std::process::Command::new(executor_executable().expect("exe"))
        .arg(LOCK_HANDOFF_ARG)
        .arg(key.major.to_string())
        .arg(key.minor.to_string())
        .output()
        .expect("run the handoff subprocess");
    assert!(
        output.status.success(),
        "handoff subprocess failed: {output:?}"
    );
    let helper_pid: libc::pid_t = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("the handoff subprocess prints its helper pid");

    // The acquiring process is gone — `output()` waited for it.
    assert!(
        may_install_state(&key).is_err(),
        "an orphaned helper must still hold the lock after its parent died"
    );

    // SAFETY: killing the orphaned helper this test created.
    unsafe { libc::kill(helper_pid, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if may_install_state(&key).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the lock was not released by the orphaned helper's death");
}

#[test]
fn the_lock_step_refuses_while_another_holder_has_it() {
    let key = DrmDeviceKey {
        major: 226,
        minor: 249,
    };
    let held = may_install_state(&key).expect("holder");
    let err = acquire_device_lock_or_refuse(&key).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ResourceBusy);
    assert!(
        format!("{err}").contains("226"),
        "the message must name the device"
    );
    drop(held);
    assert!(
        acquire_device_lock_or_refuse(&key).is_ok(),
        "and it succeeds once free"
    );
}
