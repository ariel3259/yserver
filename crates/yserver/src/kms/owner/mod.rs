#[doc(hidden)]
pub mod build;
#[doc(hidden)]
pub mod clock;
#[doc(hidden)]
pub mod closure;
#[doc(hidden)]
pub mod device;
#[doc(hidden)]
pub mod identity;
#[doc(hidden)]
pub mod ledger;
#[doc(hidden)]
pub mod lifecycle;
#[doc(hidden)]
pub mod record;
#[doc(hidden)]
pub mod slot;
#[doc(hidden)]
pub mod test_fixtures;

/// Stage 2b-i converts no live call site and therefore owns no KMS resource.
/// Stage 2c substitutes real RAII resource owners at this generic seam.
#[derive(Debug)]
pub enum NeverResource {}
