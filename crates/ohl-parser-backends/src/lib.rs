//! The parser worker's container back ends.
//!
//! `ohl-parser-worker-service` runs one OWP/1 lifetime over a
//! [`Dispatcher`](ohl_parser_worker_service::Dispatcher); this crate is the
//! dispatcher the shipped worker hosts. It joins the three clean-room
//! container decoders — [`ohl_wise`], [`ohl_mscab`] and [`ohl_isz`] — to the
//! wire protocol's pull model, in which the worker may only *ask* for bytes
//! and must return one bounded action per step.
//!
//! It lives outside the freestanding image on purpose: the image cannot host
//! a test harness, and everything interesting here — the window adapter, the
//! spelling policy, the Wise walk, the enumeration and the streaming — is
//! exercised on the host through the real service, over a scripted transport,
//! against synthetic packages built by the decoders' own writers.
//!
//! # What it is not
//!
//! It opens nothing, executes nothing, and holds no capability: the only
//! bytes it ever sees are the ones the parent chose to answer inside one
//! window. It never logs, and no error variant carries a media-derived byte;
//! recorded names live in caller-owned storage and are `Debug`-redacted at
//! every layer they pass through.
//!
//! ```no_run
//! use ohl_parser_backends::{BackendLimits, ContainerDispatcher};
//!
//! let mut arena = vec![0u8; 4 * 1024 * 1024];
//! let dispatcher = ContainerDispatcher::new(&mut arena, BackendLimits::default());
//! # let _ = dispatcher;
//! ```

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod buffered;
pub mod dispatcher;
pub mod spelling;
pub mod window;
pub mod wise;

pub use buffered::{BufferError, BufferedEntry, ContainerBuffer, MAXIMUM_BUFFERED_BYTES};
pub use dispatcher::{
    BATCH_ENTRIES, BackendLimits, CHUNK_BYTES, ContainerDispatcher, ContainerKind, SpellingArena,
};
pub use spelling::{SpellingRejection, SpellingSet, UNNAMED_DIRECTORY, unnamed_spelling};
pub use window::{DEFAULT_WINDOW_BYTES, PendingRead, WindowSource};
pub use wise::{UNNAMED_TOKEN_BASE, WiseBackend, WiseEntry};
