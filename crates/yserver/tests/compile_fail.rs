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
            s.starts_with("libyserver-")
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
