//! Owner-route preparation for copied scanout.
//!
//! This module owns the producer-side half of the copied route.  The legacy
//! copied path remains in `scene.rs` and `PlatformBackend`; this module is
//! entered only for an output that has already joined the Owner route.

use std::{io, os::fd::OwnedFd, sync::Arc};

use ash::vk;

use super::{
    platform::{FenceTicket, PlatformBackend, ScanoutRenderCompletionStage},
    resources::{
        AllocationKey, AllocationLease, CoreRetirementBatch, GpuObligation, ObligationId,
        ResourceService,
    },
    scene::{
        ComposeRenderTarget, ComposeSubmit, PostComposePreparation, Repaint,
        record_and_submit_render,
    },
};
use crate::kms::{
    backend::{OutputInstanceId, OutputKey},
    vk::{
        compositor::{CompositeScene, PresentError},
        pipeline::CompositorPipeline,
        scanout::{BoPhase, CopiedScanoutPool},
    },
};

/// Receipt retained by the Owner generation while B's registered batch is in
/// the resource service.  Task 3 consumes these exact entries; readiness and
/// generic service completion are not promotion authority.
#[allow(dead_code)]
pub(crate) struct CopiedRetirementReceipt {
    pub(crate) destination: (AllocationKey, ObligationId),
    pub(crate) source: (AllocationKey, ObligationId),
}

/// Release the source-side synchronization payload once B's exact read
/// obligation has retired. The destination receipt is deliberately not an
/// input here: CP-10 is a source/read rule, not a consequence of destination
/// readiness or of the Owner event stream.
pub(crate) fn release_source_after_read_retirement(
    pool: &mut CopiedScanoutPool,
    service: &mut ResourceService,
    bo_idx: usize,
    source: (AllocationKey, ObligationId),
) -> bool {
    if service.has_pending_obligation(&source.0, source.1) || service.is_frozen(&source.0) {
        return false;
    }
    let Ok(lease) = service.reserve(source.0, super::resources::UseKind::Read) else {
        return false;
    };
    let released = service
        .with_copied_source(&lease, |source| source.release_completed_source())
        .is_ok();
    drop(lease);
    if released {
        pool.release_completed_source(bo_idx);
    }
    released
}

/// Dispose of a sink submission after its dispatch answer is known.  The
/// service owns the cancel/freeze decision; this helper only performs the
/// sink's established quiescence and repairs the managed source payload whose
/// synchronization state no longer lives in the pool husk.
fn dispose_failed_sink_submission(
    pool: &mut CopiedScanoutPool,
    platform: &mut PlatformBackend,
    service: &mut ResourceService,
    bo_idx: usize,
    source_lease: &AllocationLease,
    entries: &[(AllocationKey, ObligationId)],
    gpu_submitted: bool,
    cause: PresentError,
) -> PresentError {
    let cause = match pool.recover_copy_failure(bo_idx) {
        Ok(()) => match service.with_copied_source(source_lease, |source| {
            source.recover_copy_failure_after_quiescence()
        }) {
            Ok(Ok(())) => cause,
            Ok(Err(error)) => {
                platform.renderer_failed = true;
                PresentError::Io(io::Error::other(format!(
                    "recover managed copied source after sink failure: {error}"
                )))
            }
            Err(error) => {
                platform.renderer_failed = true;
                PresentError::Io(io::Error::other(format!(
                    "borrow managed copied source during sink recovery: {error:?}"
                )))
            }
        },
        Err(error) => {
            platform.renderer_failed = true;
            PresentError::Io(io::Error::other(format!(
                "recover copied sink after submission failure: {error}"
            )))
        }
    };
    super::scene::managed_submit_failure(service, entries, gpu_submitted, cause)
}

struct ManagedCopiedComposeTarget<'a> {
    source: &'a mut crate::kms::render::resources::scanout::CopiedSourceAllocation,
}

impl ComposeRenderTarget for ManagedCopiedComposeTarget<'_> {
    fn image(&self) -> vk::Image {
        self.source.image()
    }

    fn image_view(&self) -> vk::ImageView {
        self.source.image_view()
    }

    fn command_buffer(&self) -> vk::CommandBuffer {
        self.source.command_buffer()
    }

    fn completion_semaphore(&self) -> vk::Semaphore {
        self.source.completion_semaphore()
    }

    fn width(&self) -> u32 {
        self.source.width()
    }

    fn height(&self) -> u32 {
        self.source.height()
    }

    fn timestamp_pool(&self) -> vk::QueryPool {
        self.source.timestamp_pool()
    }

    fn timestamps_written(&self) -> bool {
        self.source.timestamps_written()
    }

    fn mark_timestamps_written(&mut self) {
        self.source.mark_timestamps_written();
    }

    fn set_last_gpu_render_ns(&mut self, value: Option<u64>) {
        self.source.set_last_gpu_render_ns(value);
    }

    fn post_compose_preparation(&self) -> Result<PostComposePreparation, PresentError> {
        self.source
            .transport_preparation()
            .map(PostComposePreparation::Copied)
            .map_err(PresentError::Io)
    }

    fn record_post_compose(
        &self,
        _vk: &crate::kms::vk::device::VkContext,
        command_buffer: vk::CommandBuffer,
        preparation: PostComposePreparation,
    ) {
        let PostComposePreparation::Copied(preparation) = preparation else {
            unreachable!("managed copied target received shared post-compose preparation")
        };
        self.source
            .record_transport_copy(command_buffer, preparation);
    }

    fn renderer_wait_semaphore(&self) -> Option<vk::Semaphore> {
        self.source.renderer_wait_semaphore()
    }

    fn note_submit_succeeded(&mut self) {
        self.source.note_renderer_submit_succeeded();
    }
}

/// Render A into the managed copied source and export A's completion.  The
/// source Write lease and its GPU obligation are established before any
/// renderer command is submitted, matching the managed shared arm.
#[allow(clippy::too_many_arguments)]
pub(crate) fn submit_owner_copied_scanout_frame(
    vk: &Arc<crate::kms::vk::device::VkContext>,
    pool: &mut CopiedScanoutPool,
    bo_idx: usize,
    pipeline: &CompositorPipeline,
    descriptor_pool: vk::DescriptorPool,
    scene: &CompositeScene,
    repaint: Repaint,
    scissors: &[vk::Rect2D],
    compose_ticket: &FenceTicket,
    gpu_submitted: &mut bool,
    overlay_ops: &[(u32, vk::Rect2D)],
    xor_pipeline: vk::Pipeline,
    xor_layout: vk::PipelineLayout,
    service: &mut ResourceService,
) -> Result<
    (
        ComposeSubmit,
        Option<OwnedFd>,
        CoreRetirementBatch,
        Option<u64>,
    ),
    PresentError,
> {
    let source = pool
        .sources
        .get(bo_idx)
        .ok_or_else(|| PresentError::Io(io::Error::other("copied source index out of range")))?;
    let source_key = source.managed_key().ok_or_else(|| {
        PresentError::Io(io::Error::other(
            "Owner copied compose requires a managed source",
        ))
    })?;
    let destination_state = pool
        .destinations
        .bos
        .get(bo_idx)
        .ok_or_else(|| PresentError::Io(io::Error::other("copied destination index out of range")))?
        .state
        .phase;
    if destination_state != BoPhase::Recording {
        return Err(PresentError::WrongPhase(destination_state));
    }

    let (mut batch, entries) = crate::kms::render::resources::gpu::prepare_retirement_batch(
        service,
        &[source_key],
        Vec::new(),
    )
    .map_err(|error| {
        PresentError::Io(io::Error::other(format!(
            "prepare Owner copied source retirement batch: {error:?}"
        )))
    })?;
    let write_lease = &batch.leases()[0];
    let render_result = service.with_copied_source(write_lease, |source| {
        source
            .prepare_renderer_acquire()
            .map_err(PresentError::Io)?;
        let mut target = ManagedCopiedComposeTarget { source };
        record_and_submit_render(
            vk,
            &mut target,
            pipeline,
            descriptor_pool,
            scene,
            repaint,
            scissors,
            compose_ticket.fence(),
            gpu_submitted,
            overlay_ops,
            xor_pipeline,
            xor_layout,
        )
    });
    let (submitted, previous_gpu_ns) = match render_result {
        Ok(Ok(submitted)) => {
            let previous_gpu_ns = service
                .with_copied_source(write_lease, |source| source.last_gpu_render_ns.take())
                .map_err(|error| {
                    PresentError::Io(io::Error::other(format!(
                        "read managed copied source telemetry: {error:?}"
                    )))
                })?;
            (submitted, previous_gpu_ns)
        }
        Ok(Err(error)) => {
            return Err(super::scene::managed_submit_failure(
                service,
                &entries,
                *gpu_submitted,
                error,
            ));
        }
        Err(error) => {
            return Err(super::scene::managed_submit_failure(
                service,
                &entries,
                *gpu_submitted,
                PresentError::Io(io::Error::other(format!(
                    "with managed copied source: {error:?}"
                ))),
            ));
        }
    };

    let completion = service
        .with_copied_source(write_lease, |source| source.export_render_completion())
        .map_err(|error| {
            PresentError::Io(io::Error::other(format!(
                "export managed copied render completion: {error:?}"
            )))
        })?
        .map_err(PresentError::Vk)?;
    batch.bind_ticket(GpuObligation::new(
        entries,
        compose_ticket.clone(),
        Arc::clone(vk),
    ));
    Ok((submitted, completion, batch, previous_gpu_ns))
}

/// Finish the copied producer after A's completion has been serviced.  This
/// reserves B's destination Write and source Read before submitting the sink
/// copy, registers the sink fence in the same completion ledger, and returns
/// the receipt Task 3 will later consume.
pub(crate) fn prepare_owner_copy_after_render_completion(
    pool: &mut CopiedScanoutPool,
    platform: &mut PlatformBackend,
    output_key: OutputKey,
    output_instance_id: OutputInstanceId,
    bo_idx: usize,
    render_completion: Option<OwnedFd>,
    service: &mut ResourceService,
) -> Result<(CopiedRetirementReceipt, u64), PresentError> {
    let destination_key = pool
        .destinations
        .bos
        .get(bo_idx)
        .and_then(|bo| bo.managed_key())
        .ok_or_else(|| PresentError::Io(io::Error::other("copied destination is unmanaged")))?;
    let source_key = pool
        .sources
        .get(bo_idx)
        .and_then(|source| source.managed_key())
        .ok_or_else(|| PresentError::Io(io::Error::other("copied source is unmanaged")))?;
    let (mut batch, destination_entry, source_entry) =
        crate::kms::render::resources::gpu::prepare_copied_batch(
            service,
            destination_key,
            source_key,
        )
        .map_err(|error| {
            PresentError::Io(io::Error::other(format!(
                "prepare Owner copied sink batch: {error:?}"
            )))
        })?;

    if pool
        .destinations
        .bos
        .get(bo_idx)
        .is_none_or(|bo| bo.state.phase != crate::kms::vk::scanout::BoPhase::Owner)
    {
        let entries = [destination_entry, source_entry];
        return Err(super::scene::managed_submit_failure(
            service,
            &entries,
            false,
            PresentError::Io(io::Error::other(
                "copied Owner destination was released before its paired copy",
            )),
        ));
    }
    let copy_ticket = match pool.acquire_copy_fence() {
        Ok(ticket) => ticket,
        Err(error) => {
            let entries = [destination_entry, source_entry];
            return Err(super::scene::managed_submit_failure(
                service,
                &entries,
                false,
                PresentError::Vk(error),
            ));
        }
    };
    let destination_lease = &batch.leases()[0];
    let source_lease = &batch
        .read_obligation
        .as_ref()
        .expect("copied preparation binds a source read obligation")
        .source_lease;
    #[cfg(test)]
    {
        assert!(
            service.has_pending_obligation(&destination_entry.0, destination_entry.1),
            "destination Write obligation must precede the sink submission"
        );
        assert!(
            service.has_pending_obligation(&source_entry.0, source_entry.1),
            "source Read obligation must precede the sink submission"
        );
    }
    let submit_result = service.with_copied_source_and_scanout_write(
        source_lease,
        destination_lease,
        |source, destination| {
            pool.submit_managed_copy_with_fence(
                bo_idx,
                render_completion,
                copy_ticket.fence(),
                source,
                destination,
            )
        },
    );
    let completion = match submit_result {
        Ok(Ok(completion)) => completion,
        Ok(Err(error)) => {
            let entries = [destination_entry, source_entry];
            return Err(dispose_failed_sink_submission(
                pool,
                platform,
                service,
                bo_idx,
                source_lease,
                &entries,
                error.gpu_submitted(),
                PresentError::Io(error.into_io_error()),
            ));
        }
        Err(error) => {
            let entries = [destination_entry, source_entry];
            return Err(super::scene::managed_submit_failure(
                service,
                &entries,
                false,
                PresentError::Io(io::Error::other(format!(
                    "borrow copied sink payloads: {error:?}"
                ))),
            ));
        }
    };
    #[cfg(test)]
    crate::kms::render::platform::record_copied_copy_fence_for_tests(
        crate::kms::render::platform::CopiedRouteTransport::Owner,
        completion.as_ref(),
    );

    batch.bind_ticket(GpuObligation::new(
        vec![destination_entry],
        copy_ticket,
        pool.sink_context(),
    ));
    service.register_batch(batch);
    let job_id = match platform.register_scanout_render_completion(
        output_key,
        output_instance_id,
        bo_idx,
        ScanoutRenderCompletionStage::CopiedOwnerCopy,
        completion,
    ) {
        Ok(job_id) => job_id,
        Err(error) => {
            // B has already been submitted and its batch is deliberately
            // still rooted in the service.  register_scanout... either
            // installs the whole waiter or leaves no partial queue entry;
            // the generation is displaced by the caller and cannot offer on
            // generic availability.
            return Err(PresentError::Io(error));
        }
    };
    log::debug!("copied Owner B submission registered as completion job {job_id} for BO {bo_idx}");
    Ok((
        CopiedRetirementReceipt {
            destination: destination_entry,
            source: source_entry,
        },
        job_id,
    ))
}
