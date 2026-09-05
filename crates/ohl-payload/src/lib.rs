//! Payload path policy, layout planning, component selection, and staging.
//!
//! This crate owns everything between "a reader can enumerate an archive" and
//! "a payload directory exists on disk", and it is deliberately the only place
//! where those decisions are made. The pipeline runs strictly in this order,
//! because each stage narrows what the next one may act on:
//!
//! 1. [`selection`] applies a **runtime-only local recipe** to the entries a
//!    reader offers, deciding which components are imported and to which
//!    destination roots. Selection precedes layout: layout must never see an
//!    entry the user did not select.
//! 2. [`path`] validates each destination as a portable, non-traversing,
//!    reserved-name-free [`path::PayloadPath`].
//! 3. [`layout`] plans the whole set at once against bounded limits, refusing
//!    duplicates, case-only aliases, and file/directory aliases, and fixing a
//!    deterministic order.
//! 4. [`stream`] moves one entry's bytes from a source to a sink, bounded by
//!    the plan and cooperating with [`cancel`].
//! 5. [`store`] provides the transactional filesystem layer over
//!    `ohl_platform`'s create-new staging and no-replace publication.
//! 6. [`stage`] runs the commit protocol: validate, probe, stage, seal,
//!    reverify the pinned source, and publish once.
//!
//! # What this crate does not do
//!
//! It never executes an installer, never names a component of any particular
//! product, and never ships a recipe. `docs/MILESTONES.md` keeps runtime
//! extraction out of scope; this is the staging boundary that a future import
//! session will call, not the import itself.
//!
//! # Feature `std`
//!
//! On by default. The policy core — [`path`], [`layout`], and [`cancel`] —
//! needs only `alloc` and stays available with `default-features = false`, so
//! path and layout rules can be reused anywhere. Everything that touches a
//! filesystem, a pinned source, or a recipe file requires `std`.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod cancel;
pub mod layout;
pub mod path;

#[cfg(feature = "std")]
pub mod selection;
#[cfg(feature = "std")]
pub mod stage;
#[cfg(feature = "std")]
pub mod store;
#[cfg(feature = "std")]
pub mod stream;

#[cfg(all(test, feature = "std"))]
mod test_support;

pub use cancel::{CancellationSource, CancellationToken};
pub use layout::{
    PayloadEntryMetadata, PayloadImportLimits, PayloadLayout, PayloadLayoutError,
    PayloadLayoutRejection, PlannedPayloadEntry, plan_payload_layout,
};
pub use path::{PayloadPath, PayloadPathError};

#[cfg(feature = "std")]
pub use selection::{
    SelectableEntry, SelectionDecision, SelectionError, SelectionPlan, SelectionRecipe,
    SelectionRecipeError, select,
};
#[cfg(feature = "std")]
pub use stage::{
    PayloadStageError, PayloadStagePhase, PayloadStageReport, PayloadStageRequest,
    PayloadStageStatus, stage_payload,
};
#[cfg(feature = "std")]
pub use store::{
    DirectoryPayloadStore, PayloadStore, PayloadStoreError, PayloadTransaction, ProbeState,
    PublishState, StagingEntry, StagingPlan,
};
#[cfg(feature = "std")]
pub use stream::{
    PayloadByteSink, PayloadSource, PayloadStreamError, PayloadStreamOutcome, stream_payload_entry,
};
