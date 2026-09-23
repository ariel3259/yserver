use std::path::PathBuf;

#[test]
fn reap_proof_cannot_be_cloned() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let current_exe = std::env::current_exe().expect("current test executable");
    let deps_dir = current_exe
        .parent()
        .expect("deps directory containing yserver artifacts");

    let mut entries: Vec<_> = std::fs::read_dir(deps_dir)
        .expect("read deps directory")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let s = name.to_str().unwrap_or("");
            let non_empty = e.metadata().map(|m| m.len() > 0).unwrap_or(false);
            non_empty
                && s.starts_with("libyserver-")
                && (s.ends_with(".rlib") || s.ends_with(".rmeta"))
                && !s.starts_with("libyserver_")
        })
        .collect();

    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let yserver_lib = entries
        .last()
        .expect("found compiled libyserver in deps")
        .path();

    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| "crates/yserver".into()),
    );
    let test_file = manifest_dir.join("tests/compile_fail/reap_proof_cannot_clone.rs");

    let output = std::process::Command::new(rustc)
        .arg("--edition=2024")
        .arg("--crate-type=lib")
        .arg("--emit=metadata")
        .arg("-L")
        .arg(format!("dependency={}", deps_dir.display()))
        .arg("--extern")
        .arg(format!("yserver={}", yserver_lib.display()))
        .arg(&test_file)
        .output()
        .expect("execute rustc");

    assert!(
        !output.status.success(),
        "expected compilation failure for ReapProof::clone, but compilation succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no method named `clone`"),
        "expected error about missing `clone` method, got:\n{stderr}"
    );
}

#[test]
fn recovery_id_cannot_be_derived_from_transition_id() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let current_exe = std::env::current_exe().expect("current test executable");
    let deps_dir = current_exe
        .parent()
        .expect("deps directory containing yserver artifacts");

    let mut entries: Vec<_> = std::fs::read_dir(deps_dir)
        .expect("read deps directory")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let s = name.to_str().unwrap_or("");
            let non_empty = e.metadata().map(|m| m.len() > 0).unwrap_or(false);
            non_empty
                && s.starts_with("libyserver-")
                && (s.ends_with(".rlib") || s.ends_with(".rmeta"))
                && !s.starts_with("libyserver_")
        })
        .collect();

    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let yserver_lib = entries
        .last()
        .expect("found compiled libyserver in deps")
        .path();

    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| "crates/yserver".into()),
    );
    let test_file = manifest_dir.join("tests/compile_fail/recovery_id_is_not_transition_id.rs");

    let output = std::process::Command::new(rustc)
        .arg("--edition=2024")
        .arg("--crate-type=lib")
        .arg("--emit=metadata")
        .arg("-L")
        .arg(format!("dependency={}", deps_dir.display()))
        .arg("--extern")
        .arg(format!("yserver={}", yserver_lib.display()))
        .arg(&test_file)
        .current_dir(deps_dir)
        .output()
        .expect("execute rustc");

    assert!(
        !output.status.success(),
        "expected conversion from LifecycleTransitionId to RecoveryId to fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("From<LifecycleTransitionId>"),
        "expected the conversion trait to be unavailable, got:\n{stderr}"
    );
}

#[test]
fn c0_3aii_topology_payload_is_the_tag() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let current_exe = std::env::current_exe().expect("current test executable");
    let deps_dir = current_exe
        .parent()
        .expect("deps directory containing yserver artifacts");

    let mut entries: Vec<_> = std::fs::read_dir(deps_dir)
        .expect("read deps directory")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let s = name.to_str().unwrap_or("");
            let non_empty = e.metadata().map(|m| m.len() > 0).unwrap_or(false);
            non_empty
                && s.starts_with("libyserver-")
                && (s.ends_with(".rlib") || s.ends_with(".rmeta"))
                && !s.starts_with("libyserver_")
        })
        .collect();
    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let yserver_lib = entries
        .last()
        .expect("found compiled libyserver in deps")
        .path();
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| "crates/yserver".into()),
    );
    let test_file = manifest_dir.join("tests/compile_fail/topology_requires_transition_tag.rs");
    let output = std::process::Command::new(rustc)
        .arg("--edition=2024")
        .arg("--crate-type=lib")
        .arg("--emit=metadata")
        .arg("-L")
        .arg(format!("dependency={}", deps_dir.display()))
        .arg("--extern")
        .arg(format!("yserver={}", yserver_lib.display()))
        .arg(&test_file)
        .current_dir(deps_dir)
        .output()
        .expect("execute rustc");

    assert!(
        !output.status.success(),
        "expected a u64 topology payload to fail compilation"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("TransitionTag") && stderr.contains("mismatched types"),
        "expected a typed TransitionTag mismatch, got:\n{stderr}"
    );
}
