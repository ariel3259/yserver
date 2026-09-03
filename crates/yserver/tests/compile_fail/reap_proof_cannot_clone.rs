// compile-fail test: ReapProof must not derive or implement Clone
use yserver::kms::executor::ReapProof;

#[allow(dead_code)]
fn test_clone(p: ReapProof) {
    let _ = p.clone();
}
