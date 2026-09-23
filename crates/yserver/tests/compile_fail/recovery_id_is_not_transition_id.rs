// compile-fail test: recovery incidents have their own identity space.
use yserver::kms::owner::lifecycle::{LifecycleTransitionId, RecoveryId};

#[allow(dead_code)]
fn transition_is_not_a_recovery_incident() {
    let transition = LifecycleTransitionId::from_raw(1);
    let _: RecoveryId = transition.into();
}
