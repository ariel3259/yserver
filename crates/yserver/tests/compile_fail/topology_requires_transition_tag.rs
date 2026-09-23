use yserver::kms::owner::admission::Admission;

pub fn topology_payload_must_be_typed() {
    let mut admission = Admission::new();
    let _ = admission.request_topology(7_u64);
}
