//! The player's settings file: where it lives, and reading and writing it.
//!
//! The format and every bound belong to `ohl_ui::menu::OptionsState`; this
//! module only moves its text to and from disk. The file sits in the
//! platform's per-user configuration directory, beside nothing else. Its
//! path is the user's own and is never logged; a failure is reported as a
//! fixed line.

use std::path::{Path, PathBuf};

use ohl_ui::menu::OptionsState;

/// The settings file's name inside the configuration directory.
const FILE_NAME: &str = "settings.cfg";

/// The per-user settings file, or `None` when the platform publishes no
/// configuration directory. The same application identity `ohl_save` keeps
/// its save slots under.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "open-half-life", "open-half-life")
        .map(|dirs| dirs.config_dir().join(FILE_NAME))
}

/// The settings saved at `path`, or the defaults when there are none yet
/// (or they cannot be read).
#[must_use]
pub fn load(path: &Path) -> OptionsState {
    match std::fs::read_to_string(path) {
        Ok(text) => OptionsState::from_settings_text(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => OptionsState::default(),
        Err(_) => {
            tracing::warn!("The settings file could not be read; using the defaults.");
            OptionsState::default()
        }
    }
}

/// Writes `options` to `path`, creating its directory, by writing a
/// sibling temporary file and renaming it over the old one so a crash
/// mid-write never leaves a torn file. Returns whether it was written.
pub fn save(path: &Path, options: &OptionsState) -> bool {
    let written = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("cfg.tmp");
        std::fs::write(&temporary, options.to_settings_text())?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        tracing::warn!("The settings could not be saved.");
    }
    written.is_ok()
}

#[cfg(test)]
mod tests {
    use ohl_ui::menu::{DisplayMode, OptionsState};

    #[test]
    fn saved_settings_load_back_and_a_missing_file_is_the_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("nested").join("settings.cfg");
        assert_eq!(super::load(&path), OptionsState::default());

        let mut options = OptionsState::default();
        options.video.display.mode = DisplayMode::Borderless;
        options.audio.voice = 0.5;
        options.controls.invert_mouse = true;
        assert!(super::save(&path, &options));
        assert_eq!(super::load(&path), options);
        assert!(
            !path.with_extension("cfg.tmp").exists(),
            "the temporary file was renamed into place"
        );
    }

    #[test]
    fn the_settings_file_lives_in_a_configuration_directory() {
        if let Some(path) = super::default_path() {
            assert_eq!(
                path.file_name().and_then(|name| name.to_str()),
                Some("settings.cfg")
            );
        }
    }
}
