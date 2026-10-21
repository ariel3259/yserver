use crate::kms::vk::{
    scanout::{ScanoutIoContext, ScanoutVkOperationError},
    target::DrawableImageError,
};
use ash::vk;
use std::io;

pub(super) fn scanout_vk_error(operation: &'static str, result: vk::Result) -> io::Error {
    io::Error::other(ScanoutVkOperationError { operation, result })
}

#[cfg(test)]
pub(crate) fn device_lost_scanout_error_for_tests() -> io::Error {
    scanout_vk_error("test scanout operation", vk::Result::ERROR_DEVICE_LOST)
}

pub(super) fn scanout_io_context(context: impl Into<String>, source: io::Error) -> io::Error {
    io::Error::new(
        source.kind(),
        ScanoutIoContext {
            context: context.into(),
            source,
        },
    )
}

pub(super) fn copied_drawable_error(
    operation: &'static str,
    error: DrawableImageError,
) -> io::Error {
    match error {
        DrawableImageError::Vk(result) => scanout_vk_error(operation, result),
        error => io::Error::other(format!("{operation}: {error}")),
    }
}

pub(super) fn copied_quiescence_result(
    operation: &'static str,
    result: Result<(), vk::Result>,
) -> io::Result<()> {
    result.map_err(|result| scanout_vk_error(operation, result))
}

/// Whether an error from exact scanout allocation or a disposable rendering
/// probe contains `VK_ERROR_DEVICE_LOST` in its preserved source chain.
#[must_use]
pub(crate) fn scanout_error_is_device_lost(error: &io::Error) -> bool {
    fn contains_device_lost(error: &(dyn std::error::Error + 'static)) -> bool {
        if error
            .downcast_ref::<ScanoutVkOperationError>()
            .is_some_and(|error| error.result == vk::Result::ERROR_DEVICE_LOST)
        {
            return true;
        }

        // `io::Error` exposes its custom payload through `get_ref`; relying
        // only on `Error::source` loses that payload on some std versions.
        if let Some(io_error) = error.downcast_ref::<io::Error>()
            && let Some(inner) = io_error.get_ref()
        {
            return contains_device_lost(inner);
        }

        error.source().is_some_and(contains_device_lost)
    }

    contains_device_lost(error)
}

/// Whether a typed scanout Vulkan result represents a missing test prerequisite.
#[cfg(test)]
pub(crate) fn scanout_error_is_unavailable_for_tests(error: &io::Error) -> bool {
    fn scanout_vk_result(error: &(dyn std::error::Error + 'static)) -> Option<vk::Result> {
        if let Some(error) = error.downcast_ref::<ScanoutVkOperationError>() {
            return Some(error.result);
        }

        if let Some(io_error) = error.downcast_ref::<io::Error>()
            && let Some(inner) = io_error.get_ref()
        {
            return scanout_vk_result(inner);
        }

        error.source().and_then(scanout_vk_result)
    }

    scanout_vk_result(error).is_some_and(|result| {
        crate::kms::vk::device::VkInitError::Vk(result).is_unavailable_for_tests()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mutation: omit either missing-capability VkResult from the shared test classifier.
    #[test]
    fn typed_scanout_missing_capabilities_are_unavailable_for_tests() {
        for result in [
            vk::Result::ERROR_EXTENSION_NOT_PRESENT,
            vk::Result::ERROR_FEATURE_NOT_PRESENT,
        ] {
            let error = scanout_vk_error("allocate exact copied source", result);
            assert!(
                scanout_error_is_unavailable_for_tests(&error),
                "typed scanout prerequisite error was not classified: {error}"
            );
        }

        let unexpected = scanout_vk_error(
            "allocate exact copied source",
            vk::Result::ERROR_OUT_OF_HOST_MEMORY,
        );
        assert!(!scanout_error_is_unavailable_for_tests(&unexpected));
    }
}
