// Proves the widened surface is genuinely reachable from an EXTERNAL test
// crate, which is the whole point of the visibility table: tasks 3, 4 and 6
// place their tests here, and `send` takes a HostCallRequest.
use yserver::{
    kms::executor::{
        HostCallClass,
        protocol::{HostCallRequest, decode_request, encode_request},
        test_support,
    },
    platform::drm::DrmDeviceKey,
};

#[test]
fn the_test_seam_is_reachable_from_an_external_crate() {
    let request = test_support::small_atomic_request_for_tests();
    assert_eq!(request.class(), HostCallClass::SeatActiveNonblock);
    let frame = encode_request(&request);
    assert_eq!(decode_request(&frame).expect("decode"), request);

    let probe: HostCallRequest = test_support::probe_request_for_tests();
    assert_ne!(
        probe.kind(),
        request.kind(),
        "the two families are distinct"
    );

    let key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    assert_eq!(key.major, 226);
}
