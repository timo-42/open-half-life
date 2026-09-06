//! One frame's worth of player intent.

/// A host-independent input snapshot handed to [`crate::Game::tick`].
///
/// The movement axes are the same tri-state the physics controller expects
/// (`-1`, `0`, `+1`); `mouse_delta` is in raw device pixels and is scaled by
/// [`crate::MOUSE_SENSITIVITY`].
// A frame's buttons are genuinely independent held/pressed flags, exactly as
// the host's key bindings deliver them; folding them into enums would only
// move the same fan-out into the binding layer.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Input {
    /// Forward (`+1`) / back (`-1`).
    pub forward: i8,
    /// Right (`+1`) / left (`-1`).
    pub right: i8,
    /// Up (`+1`) / down (`-1`), used only while noclipping.
    pub up: i8,
    /// Whether jump is held.
    pub jump: bool,
    /// Whether duck is held.
    pub duck: bool,
    /// Set for exactly the frame "use" was pressed, not while it is held.
    /// Drives an instantaneous action (opening a door, pressing a button).
    pub use_pressed: bool,
    /// Whether "use" is currently held down. Held, not an edge: a
    /// use-and-hold charger drains for as long as this stays `true`, unlike
    /// [`Self::use_pressed`], which the host clears every frame regardless
    /// of how long the key stays down.
    pub use_held: bool,
    /// Whether primary fire is held. Held, not an edge: a fully automatic
    /// weapon fires for as long as it is down, and the firing state machine
    /// decides the cadence.
    pub attack: bool,
    /// Whether secondary fire is held.
    pub attack2: bool,
    /// Set for exactly the frame reload was pressed.
    pub reload: bool,
    /// The HUD weapon slot selected this frame, when one was.
    pub select_slot: Option<u8>,
    /// Set for exactly the frame the flashlight was toggled.
    pub flashlight_pressed: bool,
    /// Relative mouse motion since the last tick, in device pixels.
    pub mouse_delta: (f32, f32),
}
