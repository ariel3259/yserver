//! Stage 2c-i debt, session 2, item 4.3: the server reset's forced teardown
//! erases a managed drawable's identity but not its proof-gated backing, and
//! a proof arriving after the numeric XIDs are reused reaches only the old
//! incarnation's entry. Spec:
//! docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md §4.3.
//!
//! **What this does NOT prove (F8 stop, round-1 B-3).** Spec 4.3 asks the
//! test to continue across the reset's own generation replacement.
//! `reset_generation` is `pub(crate)` in `yserver-core` and needs a live
//! poller, a setup registry and an input inventory, none of which this crate
//! can supply, and spec 4.3 says that an undrivable half is an F8 stop to
//! report rather than a reason to add wiring. So the second half below
//! reuses the numeric XIDs over the same backend without crossing the real
//! boundary: it proves the ledger keys proofs by allocation and not by XID,
//! and it does not prove the reset's invariant 6. The crossing itself stays
//! open, for whoever owns the boundary next.

use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    os::unix::net::UnixStream,
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU16},
};

use yserver_core::{
    backend::PixmapHandle,
    core_loop::reset::force_destroy_all_clients,
    resources::ROOT_WINDOW,
    server::{ClientState, ServerState},
};
use yserver_protocol::x11::{ClientByteOrder, ClientId, CreatePixmapRequest, ResourceId};

use super::{
    AllocationKey, AllocationPayload, ObligationKind, ResourceService,
    storage::{PixelIdentity, StorageBacking, StorageLease},
    tests::SpyAllocation,
};
use crate::kms::{
    owner::identity::IncarnationId,
    render::{
        backend::KmsBackend,
        store::{DrawableId, DrawableKind, Storage},
        target::PaintTarget,
    },
};

const CLIENT: u32 = 7;
/// The protocol XID both generations use, deliberately identical.
const PIXMAP: ResourceId = ResourceId(0x0070_0002);
/// The backend's host XID both generations use, deliberately identical.
const HOST_XID: u32 = 0x0400_0002;

fn install_client(state: &mut ServerState, id: u32) {
    let (a, _b) = UnixStream::pair().unwrap();
    state.clients.insert(
        id,
        ClientState {
            writer: Arc::new(Mutex::new(yserver_core::transport::Transport::Unix(a))),
            byte_order: ClientByteOrder::LittleEndian,
            last_sequence: Arc::new(AtomicU16::new(0)),
            resource_id_base: 0,
            resource_id_mask: u32::MAX,
            event_masks: HashMap::new(),
            save_set: HashSet::new(),
            big_requests_enabled: false,
            xi2_masks: HashMap::new(),
            xi1_event_classes: HashSet::new(),
            xi1_window_event_classes: HashMap::new(),
            outbound: VecDeque::new(),
            watching_writable: false,
            focused_window: ROOT_WINDOW,
            reader_control: None,
            is_local: true,
            fd_passing: true,
        },
    );
}

/// One client owning `PIXMAP`, backed by a managed drawable at `HOST_XID`
/// whose allocation is a fresh spy entry. Returns the drawable, its key and
/// the spy's drop counter.
fn seed_managed_pixmap(
    state: &mut ServerState,
    backend: &mut KmsBackend,
) -> (DrawableId, AllocationKey, Rc<Cell<usize>>) {
    install_client(state, CLIENT);
    let drops = Rc::new(Cell::new(0));
    let service = backend.resource_service_mut().expect("service installed");
    let lease = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops),
        }))
        .expect("adopt");
    let key = lease.key();
    let storage = Storage::from_backing(StorageBacking::Managed(StorageLease {
        allocation: lease,
        pixels: PixelIdentity {
            target: PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24),
            allocation: key,
            content_offset: (0, 0),
            extent: ash::vk::Extent2D {
                width: 16,
                height: 16,
            },
            format: ash::vk::Format::B8G8R8A8_UNORM,
            image_view: ash::vk::ImageView::null(),
            sample_view: ash::vk::ImageView::null(),
            image: ash::vk::Image::null(),
        },
    }));
    let id = backend
        .store
        .allocate(HOST_XID, DrawableKind::Pixmap, 24, false, storage)
        .expect("the host XID is free");
    state.resources.create_pixmap(
        ClientId(CLIENT),
        CreatePixmapRequest {
            pixmap: PIXMAP,
            drawable: ROOT_WINDOW,
            width: 16,
            height: 16,
            depth: 24,
        },
    );
    assert!(
        state
            .resources
            .set_pixmap_host_xid(PIXMAP, PixmapHandle::from_raw(HOST_XID).unwrap())
    );
    (id, key, drops)
}

#[test]
fn c0_2ci_reset_forced_teardown_keeps_gated_backing_and_late_proof_skips_reused_xid() {
    let mut backend = KmsBackend::for_tests();
    let device = backend
        .platform
        .primary_device()
        .expect("primary device")
        .key;
    backend.install_resource_service(ResourceService::new(device, IncarnationId::first()));

    // Generation 1: a managed drawable with GPU work still pending.
    let mut state = ServerState::new();
    let (old_id, old_key, old_drops) = seed_managed_pixmap(&mut state, &mut backend);
    let pending = backend
        .resource_service_mut()
        .unwrap()
        .register(old_key, ObligationKind::Gpu)
        .expect("register");

    // The reset's own forced teardown.
    force_destroy_all_clients(&mut state, &mut backend);

    // Identity is erased...
    assert!(state.resources.pixmap(PIXMAP).is_none());
    assert_eq!(
        backend.store.lookup(HOST_XID),
        None,
        "the old host XID still resolves after the forced teardown"
    );
    // ...but the backing is still rooted, its release gated on the proof.
    let service = backend.resource_service_mut().unwrap();
    service.service_ready();
    assert!(
        service.contains(&old_key),
        "the forced teardown released a backing whose GPU obligation is pending"
    );
    assert_eq!(old_drops.get(), 0, "the pending backing was destroyed");

    // A second session reuses both numeric XIDs over the same backend. This
    // is not the reset's own generation replacement -- see the module's F8
    // stop -- it is the numeric reuse that replacement would produce.
    let mut state = ServerState::new();
    let (new_id, new_key, new_drops) = seed_managed_pixmap(&mut state, &mut backend);
    assert_ne!(new_id, old_id);
    assert_ne!(new_key, old_key);

    // The old generation's proof arrives late.
    let service = backend.resource_service_mut().unwrap();
    service
        .apply_validated_proof(old_key, pending)
        .expect("the late proof names the old entry");
    service.service_ready();
    assert!(
        !service.contains(&old_key),
        "the old backing was never released"
    );
    assert_eq!(old_drops.get(), 1);
    assert!(
        service.contains(&new_key),
        "the late proof released the new generation's backing"
    );
    assert_eq!(new_drops.get(), 0);

    // The new drawable still resolves, to its own backing.
    assert_eq!(backend.store.lookup(HOST_XID), Some(new_id));
    let lease_key = backend
        .store
        .get(new_id)
        .and_then(|d| d.managed_lease())
        .map(|lease| lease.allocation.key());
    assert_eq!(lease_key, Some(new_key));
    drop(state);
}
