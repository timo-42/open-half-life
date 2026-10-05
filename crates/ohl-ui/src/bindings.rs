//! Rebindable gameplay controls: which key or mouse button triggers each
//! [`Action`].
//!
//! This is pure data. The options menu edits a [`Bindings`] table and the
//! host looks every key and mouse press up in it, so a binding the player
//! changes takes effect on the very next press. `Escape` (menus) and the
//! backquote key (console) are reserved by the host and can never be
//! bound to an action.

use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// A gameplay action a key or mouse button can be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    MoveForward,
    MoveBack,
    MoveLeft,
    MoveRight,
    Jump,
    Duck,
    Use,
    Attack,
    Attack2,
    Reload,
    Flashlight,
    Weapon1,
    Weapon2,
    Weapon3,
    Weapon4,
    Weapon5,
    Scoreboard,
    QuickSave,
    QuickLoad,
    PerformanceOverlay,
}

/// Which section of the controls list an [`Action`] is shown under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionGroup {
    Movement,
    Combat,
    Interface,
}

impl ActionGroup {
    /// Every group, in the order the controls list shows them.
    pub const ALL: [Self; 3] = [Self::Movement, Self::Combat, Self::Interface];

    /// The section heading.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Movement => "Movement",
            Self::Combat => "Combat",
            Self::Interface => "Interface",
        }
    }
}

impl Action {
    /// Every action, in the order the controls list shows them.
    pub const ALL: [Self; 20] = [
        Self::MoveForward,
        Self::MoveBack,
        Self::MoveLeft,
        Self::MoveRight,
        Self::Jump,
        Self::Duck,
        Self::Use,
        Self::Attack,
        Self::Attack2,
        Self::Reload,
        Self::Flashlight,
        Self::Weapon1,
        Self::Weapon2,
        Self::Weapon3,
        Self::Weapon4,
        Self::Weapon5,
        Self::Scoreboard,
        Self::QuickSave,
        Self::QuickLoad,
        Self::PerformanceOverlay,
    ];

    /// This action's position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The player-facing name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MoveForward => "Move forward",
            Self::MoveBack => "Move back",
            Self::MoveLeft => "Strafe left",
            Self::MoveRight => "Strafe right",
            Self::Jump => "Jump",
            Self::Duck => "Duck",
            Self::Use => "Use",
            Self::Attack => "Primary attack",
            Self::Attack2 => "Secondary attack",
            Self::Reload => "Reload",
            Self::Flashlight => "Flashlight",
            Self::Weapon1 => "Weapon slot 1",
            Self::Weapon2 => "Weapon slot 2",
            Self::Weapon3 => "Weapon slot 3",
            Self::Weapon4 => "Weapon slot 4",
            Self::Weapon5 => "Weapon slot 5",
            Self::Scoreboard => "Show scores",
            Self::QuickSave => "Quick save",
            Self::QuickLoad => "Quick load",
            Self::PerformanceOverlay => "Performance overlay",
        }
    }

    /// The stable identifier the settings file stores this action under.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::MoveForward => "forward",
            Self::MoveBack => "back",
            Self::MoveLeft => "moveleft",
            Self::MoveRight => "moveright",
            Self::Jump => "jump",
            Self::Duck => "duck",
            Self::Use => "use",
            Self::Attack => "attack",
            Self::Attack2 => "attack2",
            Self::Reload => "reload",
            Self::Flashlight => "flashlight",
            Self::Weapon1 => "slot1",
            Self::Weapon2 => "slot2",
            Self::Weapon3 => "slot3",
            Self::Weapon4 => "slot4",
            Self::Weapon5 => "slot5",
            Self::Scoreboard => "scores",
            Self::QuickSave => "quicksave",
            Self::QuickLoad => "quickload",
            Self::PerformanceOverlay => "perfoverlay",
        }
    }

    /// The action [`Self::id`] names.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }

    /// The section of the controls list this action belongs to.
    #[must_use]
    pub const fn group(self) -> ActionGroup {
        match self {
            Self::MoveForward
            | Self::MoveBack
            | Self::MoveLeft
            | Self::MoveRight
            | Self::Jump
            | Self::Duck
            | Self::Use => ActionGroup::Movement,
            Self::Attack
            | Self::Attack2
            | Self::Reload
            | Self::Flashlight
            | Self::Weapon1
            | Self::Weapon2
            | Self::Weapon3
            | Self::Weapon4
            | Self::Weapon5 => ActionGroup::Combat,
            Self::Scoreboard | Self::QuickSave | Self::QuickLoad | Self::PerformanceOverlay => {
                ActionGroup::Interface
            }
        }
    }

    /// The binding a fresh install starts with: the keys the window has
    /// always used.
    #[must_use]
    pub const fn default_binding(self) -> Binding {
        match self {
            Self::MoveForward => Binding::Key(KeyCode::KeyW),
            Self::MoveBack => Binding::Key(KeyCode::KeyS),
            Self::MoveLeft => Binding::Key(KeyCode::KeyA),
            Self::MoveRight => Binding::Key(KeyCode::KeyD),
            Self::Jump => Binding::Key(KeyCode::Space),
            Self::Duck => Binding::Key(KeyCode::ControlLeft),
            Self::Use => Binding::Key(KeyCode::KeyE),
            Self::Attack => Binding::Mouse(MouseButton::Left),
            Self::Attack2 => Binding::Mouse(MouseButton::Right),
            Self::Reload => Binding::Key(KeyCode::KeyR),
            Self::Flashlight => Binding::Key(KeyCode::KeyF),
            Self::Weapon1 => Binding::Key(KeyCode::Digit1),
            Self::Weapon2 => Binding::Key(KeyCode::Digit2),
            Self::Weapon3 => Binding::Key(KeyCode::Digit3),
            Self::Weapon4 => Binding::Key(KeyCode::Digit4),
            Self::Weapon5 => Binding::Key(KeyCode::Digit5),
            Self::Scoreboard => Binding::Key(KeyCode::Tab),
            Self::QuickSave => Binding::Key(KeyCode::F6),
            Self::QuickLoad => Binding::Key(KeyCode::F7),
            Self::PerformanceOverlay => Binding::Key(KeyCode::KeyP),
        }
    }
}

/// A key or mouse button an [`Action`] can be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
}

/// Every key a binding may use, with its player-facing label. A key
/// outside this table (a media key, say) cannot be bound: the settings
/// file stores a key by name, and only these names are read back.
const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "A"),
    (KeyCode::KeyB, "B"),
    (KeyCode::KeyC, "C"),
    (KeyCode::KeyD, "D"),
    (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"),
    (KeyCode::KeyG, "G"),
    (KeyCode::KeyH, "H"),
    (KeyCode::KeyI, "I"),
    (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"),
    (KeyCode::KeyL, "L"),
    (KeyCode::KeyM, "M"),
    (KeyCode::KeyN, "N"),
    (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"),
    (KeyCode::KeyQ, "Q"),
    (KeyCode::KeyR, "R"),
    (KeyCode::KeyS, "S"),
    (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"),
    (KeyCode::KeyV, "V"),
    (KeyCode::KeyW, "W"),
    (KeyCode::KeyX, "X"),
    (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"),
    (KeyCode::Digit0, "0"),
    (KeyCode::Digit1, "1"),
    (KeyCode::Digit2, "2"),
    (KeyCode::Digit3, "3"),
    (KeyCode::Digit4, "4"),
    (KeyCode::Digit5, "5"),
    (KeyCode::Digit6, "6"),
    (KeyCode::Digit7, "7"),
    (KeyCode::Digit8, "8"),
    (KeyCode::Digit9, "9"),
    (KeyCode::F1, "F1"),
    (KeyCode::F2, "F2"),
    (KeyCode::F3, "F3"),
    (KeyCode::F4, "F4"),
    (KeyCode::F5, "F5"),
    (KeyCode::F6, "F6"),
    (KeyCode::F7, "F7"),
    (KeyCode::F8, "F8"),
    (KeyCode::F9, "F9"),
    (KeyCode::F10, "F10"),
    (KeyCode::F11, "F11"),
    (KeyCode::F12, "F12"),
    (KeyCode::Space, "Space"),
    (KeyCode::Tab, "Tab"),
    (KeyCode::Enter, "Enter"),
    (KeyCode::Backspace, "Backspace"),
    (KeyCode::CapsLock, "Caps Lock"),
    (KeyCode::ShiftLeft, "Left Shift"),
    (KeyCode::ShiftRight, "Right Shift"),
    (KeyCode::ControlLeft, "Left Ctrl"),
    (KeyCode::ControlRight, "Right Ctrl"),
    (KeyCode::AltLeft, "Left Alt"),
    (KeyCode::AltRight, "Right Alt"),
    (KeyCode::ArrowUp, "Up Arrow"),
    (KeyCode::ArrowDown, "Down Arrow"),
    (KeyCode::ArrowLeft, "Left Arrow"),
    (KeyCode::ArrowRight, "Right Arrow"),
    (KeyCode::Insert, "Insert"),
    (KeyCode::Delete, "Delete"),
    (KeyCode::Home, "Home"),
    (KeyCode::End, "End"),
    (KeyCode::PageUp, "Page Up"),
    (KeyCode::PageDown, "Page Down"),
    (KeyCode::Minus, "-"),
    (KeyCode::Equal, "="),
    (KeyCode::BracketLeft, "["),
    (KeyCode::BracketRight, "]"),
    (KeyCode::Semicolon, ";"),
    (KeyCode::Quote, "'"),
    (KeyCode::Comma, ","),
    (KeyCode::Period, "."),
    (KeyCode::Slash, "/"),
    (KeyCode::Backslash, "\\"),
    (KeyCode::Numpad0, "Keypad 0"),
    (KeyCode::Numpad1, "Keypad 1"),
    (KeyCode::Numpad2, "Keypad 2"),
    (KeyCode::Numpad3, "Keypad 3"),
    (KeyCode::Numpad4, "Keypad 4"),
    (KeyCode::Numpad5, "Keypad 5"),
    (KeyCode::Numpad6, "Keypad 6"),
    (KeyCode::Numpad7, "Keypad 7"),
    (KeyCode::Numpad8, "Keypad 8"),
    (KeyCode::Numpad9, "Keypad 9"),
    (KeyCode::NumpadAdd, "Keypad +"),
    (KeyCode::NumpadSubtract, "Keypad -"),
    (KeyCode::NumpadMultiply, "Keypad *"),
    (KeyCode::NumpadDivide, "Keypad /"),
    (KeyCode::NumpadDecimal, "Keypad ."),
    (KeyCode::NumpadEnter, "Keypad Enter"),
];

/// Every mouse button a binding may use: (button, label, settings name).
const MOUSE_BUTTONS: &[(MouseButton, &str, &str)] = &[
    (MouseButton::Left, "Mouse 1", "MouseLeft"),
    (MouseButton::Right, "Mouse 2", "MouseRight"),
    (MouseButton::Middle, "Mouse 3", "MouseMiddle"),
    (MouseButton::Back, "Mouse 4", "MouseBack"),
    (MouseButton::Forward, "Mouse 5", "MouseForward"),
];

impl Binding {
    /// `code` as a binding, or `None` when it is reserved by the host or
    /// not a key a binding may use (see [`KEYS`]).
    #[must_use]
    pub fn from_key(code: KeyCode) -> Option<Self> {
        KEYS.iter()
            .any(|(key, _)| *key == code)
            .then_some(Self::Key(code))
    }

    /// `button` as a binding, or `None` when it is not one a binding may
    /// use.
    #[must_use]
    pub fn from_mouse(button: MouseButton) -> Option<Self> {
        MOUSE_BUTTONS
            .iter()
            .any(|(candidate, _, _)| *candidate == button)
            .then_some(Self::Mouse(button))
    }

    /// The player-facing name, for example `W`, `Left Ctrl` or `Mouse 1`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Key(code) => KEYS
                .iter()
                .find(|(key, _)| *key == code)
                .map_or("?", |(_, label)| label),
            Self::Mouse(button) => MOUSE_BUTTONS
                .iter()
                .find(|(candidate, _, _)| *candidate == button)
                .map_or("?", |(_, label, _)| label),
        }
    }

    /// The stable name the settings file stores this binding under: a key's
    /// `winit` name (`KeyW`, `ControlLeft`) or `MouseLeft`-style for a
    /// button.
    #[must_use]
    pub fn id(self) -> String {
        match self {
            Self::Key(code) => format!("{code:?}"),
            Self::Mouse(button) => MOUSE_BUTTONS
                .iter()
                .find(|(candidate, _, _)| *candidate == button)
                .map_or_else(String::new, |(_, _, id)| (*id).to_owned()),
        }
    }

    /// The binding [`Self::id`] names, or `None` for anything else.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        if let Some((button, _, _)) = MOUSE_BUTTONS
            .iter()
            .find(|(_, _, candidate)| *candidate == id)
        {
            return Some(Self::Mouse(*button));
        }
        KEYS.iter()
            .find(|(code, _)| format!("{code:?}") == id)
            .map(|(code, _)| Self::Key(*code))
    }
}

/// Which binding each [`Action`] has. At most one action owns a binding,
/// and an action may be left unbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    slots: [Option<Binding>; Action::ALL.len()],
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            slots: Action::ALL.map(|action| Some(action.default_binding())),
        }
    }
}

impl Bindings {
    /// The binding `action` currently has.
    #[must_use]
    pub fn get(&self, action: Action) -> Option<Binding> {
        self.slots[action.index()]
    }

    /// Binds `action` to `binding`, taking the binding away from whichever
    /// other action had it, which is left unbound.
    pub fn bind(&mut self, action: Action, binding: Binding) {
        for slot in &mut self.slots {
            if *slot == Some(binding) {
                *slot = None;
            }
        }
        self.slots[action.index()] = Some(binding);
    }

    /// Leaves `action` unbound.
    pub fn unbind(&mut self, action: Action) {
        self.slots[action.index()] = None;
    }

    /// The action `binding` triggers, if any.
    #[must_use]
    pub fn action_for(&self, binding: Binding) -> Option<Action> {
        Action::ALL
            .into_iter()
            .find(|action| self.slots[action.index()] == Some(binding))
    }

    /// The action `code` triggers, if any.
    #[must_use]
    pub fn action_for_key(&self, code: KeyCode) -> Option<Action> {
        self.action_for(Binding::Key(code))
    }

    /// The action `button` triggers, if any.
    #[must_use]
    pub fn action_for_mouse(&self, button: MouseButton) -> Option<Action> {
        self.action_for(Binding::Mouse(button))
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, ActionGroup, Binding, Bindings};
    use winit::event::MouseButton;
    use winit::keyboard::KeyCode;

    #[test]
    fn every_action_has_its_own_index_and_id() {
        for (index, action) in Action::ALL.into_iter().enumerate() {
            assert_eq!(action.index(), index);
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("nonsense"), None);
    }

    #[test]
    fn every_group_has_an_action() {
        for group in ActionGroup::ALL {
            assert!(Action::ALL.iter().any(|action| action.group() == group));
        }
    }

    #[test]
    fn the_defaults_are_the_keys_the_window_always_used_and_all_distinct() {
        let bindings = Bindings::default();
        assert_eq!(
            bindings.action_for_key(KeyCode::KeyW),
            Some(Action::MoveForward)
        );
        assert_eq!(
            bindings.action_for_key(KeyCode::ControlLeft),
            Some(Action::Duck)
        );
        assert_eq!(
            bindings.action_for_mouse(MouseButton::Left),
            Some(Action::Attack)
        );
        for action in Action::ALL {
            let binding = bindings.get(action).expect("every action starts bound");
            assert_eq!(bindings.action_for(binding), Some(action), "{action:?}");
            assert_ne!(binding.label(), "?", "{action:?}");
        }
    }

    #[test]
    fn binding_a_taken_key_moves_it_and_leaves_the_old_action_unbound() {
        let mut bindings = Bindings::default();
        bindings.bind(Action::Jump, Binding::Key(KeyCode::KeyW));
        assert_eq!(
            bindings.get(Action::Jump),
            Some(Binding::Key(KeyCode::KeyW))
        );
        assert_eq!(bindings.get(Action::MoveForward), None);
        assert_eq!(bindings.action_for_key(KeyCode::Space), None);
        assert_eq!(bindings.action_for_key(KeyCode::KeyW), Some(Action::Jump));
        bindings.unbind(Action::Jump);
        assert_eq!(bindings.action_for_key(KeyCode::KeyW), None);
    }

    #[test]
    fn the_hosts_reserved_keys_and_unknown_keys_cannot_be_bound() {
        assert_eq!(Binding::from_key(KeyCode::Escape), None);
        assert_eq!(Binding::from_key(KeyCode::Backquote), None);
        assert_eq!(Binding::from_key(KeyCode::MediaPlayPause), None);
        assert_eq!(Binding::from_mouse(MouseButton::Other(9)), None);
        assert_eq!(
            Binding::from_key(KeyCode::KeyQ),
            Some(Binding::Key(KeyCode::KeyQ))
        );
    }

    #[test]
    fn every_bindable_input_round_trips_through_its_settings_name() {
        for (code, _) in super::KEYS {
            let binding = Binding::Key(*code);
            assert_eq!(Binding::from_id(&binding.id()), Some(binding));
        }
        for (button, _, _) in super::MOUSE_BUTTONS {
            let binding = Binding::Mouse(*button);
            assert_eq!(Binding::from_id(&binding.id()), Some(binding));
        }
        assert_eq!(Binding::from_id("Escape"), None);
        assert_eq!(Binding::from_id(""), None);
    }

    #[test]
    fn labels_are_player_facing() {
        assert_eq!(Binding::Key(KeyCode::ControlLeft).label(), "Left Ctrl");
        assert_eq!(Binding::Key(KeyCode::Digit3).label(), "3");
        assert_eq!(Binding::Mouse(MouseButton::Right).label(), "Mouse 2");
    }
}
