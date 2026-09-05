//! Console variables ("cvars"): typed, bounded, named settings with optional
//! change callbacks, in the tradition of the Quake-style developer console.

use std::collections::BTreeMap;
use std::fmt;

/// A variable's current value.
#[derive(Debug, Clone, PartialEq)]
pub enum VarValue {
    /// A boolean flag, parsed from `0`/`1`/`true`/`false`.
    Bool(bool),
    /// A bounded integer.
    Int(i64),
    /// A bounded floating point number.
    Float(f64),
    /// An unbounded string.
    Str(String),
}

impl fmt::Display for VarValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(f, "{value}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{value}"),
            Self::Str(value) => f.write_str(value),
        }
    }
}

/// Numeric bounds applied to [`VarValue::Int`] and [`VarValue::Float`]
/// variables. Both ends are inclusive; either end may be left open.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VarBounds {
    /// The smallest value the variable accepts, if bounded below.
    pub min: Option<f64>,
    /// The largest value the variable accepts, if bounded above.
    pub max: Option<f64>,
}

impl VarBounds {
    /// No lower or upper bound.
    pub const UNBOUNDED: Self = Self {
        min: None,
        max: None,
    };

    /// Bounds a value to `[min, max]`.
    #[must_use]
    pub const fn new(min: f64, max: f64) -> Self {
        Self {
            min: Some(min),
            max: Some(max),
        }
    }

    fn contains(&self, value: f64) -> bool {
        self.min.is_none_or(|min| value >= min) && self.max.is_none_or(|max| value <= max)
    }
}

/// A callback invoked after a variable's value successfully changes.
pub type ChangeCallback = Box<dyn FnMut(&VarValue) + Send>;

struct Variable {
    value: VarValue,
    bounds: VarBounds,
    on_change: Option<ChangeCallback>,
}

/// A variable lookup or assignment failure.
#[derive(Debug)]
pub enum VariableError {
    /// No variable is registered under that name.
    NotFound(String),
    /// The variable already exists; `register_*` does not overwrite.
    AlreadyRegistered(String),
    /// The supplied text could not be parsed as the variable's type.
    InvalidValue { name: String, text: String },
    /// The parsed numeric value fell outside the variable's bounds.
    OutOfBounds {
        name: String,
        value: f64,
        bounds: VarBounds,
    },
}

impl fmt::Display for VariableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "unknown variable \"{name}\""),
            Self::AlreadyRegistered(name) => write!(f, "variable \"{name}\" is already registered"),
            Self::InvalidValue { name, text } => {
                write!(f, "\"{text}\" is not a valid value for \"{name}\"")
            }
            Self::OutOfBounds {
                name,
                value,
                bounds,
            } => write!(
                f,
                "{value} is out of bounds for \"{name}\" (min={:?}, max={:?})",
                bounds.min, bounds.max
            ),
        }
    }
}

impl std::error::Error for VariableError {}

/// A named collection of typed, bounded settings.
#[derive(Default)]
pub struct Variables {
    entries: BTreeMap<String, Variable>,
}

impl Variables {
    /// Creates an empty set of variables.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn register(
        &mut self,
        name: &str,
        value: VarValue,
        bounds: VarBounds,
    ) -> Result<(), VariableError> {
        if self.entries.contains_key(name) {
            return Err(VariableError::AlreadyRegistered(name.to_string()));
        }
        self.entries.insert(
            name.to_string(),
            Variable {
                value,
                bounds,
                on_change: None,
            },
        );
        Ok(())
    }

    /// Registers a boolean variable with `default`.
    pub fn register_bool(&mut self, name: &str, default: bool) -> Result<(), VariableError> {
        self.register(name, VarValue::Bool(default), VarBounds::UNBOUNDED)
    }

    /// Registers an integer variable with `default`, bounded to `bounds`.
    pub fn register_int(
        &mut self,
        name: &str,
        default: i64,
        bounds: VarBounds,
    ) -> Result<(), VariableError> {
        self.register(name, VarValue::Int(default), bounds)
    }

    /// Registers a floating point variable with `default`, bounded to
    /// `bounds`.
    pub fn register_float(
        &mut self,
        name: &str,
        default: f64,
        bounds: VarBounds,
    ) -> Result<(), VariableError> {
        self.register(name, VarValue::Float(default), bounds)
    }

    /// Registers a string variable with `default`.
    pub fn register_str(
        &mut self,
        name: &str,
        default: impl Into<String>,
    ) -> Result<(), VariableError> {
        self.register(name, VarValue::Str(default.into()), VarBounds::UNBOUNDED)
    }

    /// Registers (or replaces) the callback invoked after `name` changes.
    pub fn on_change(
        &mut self,
        name: &str,
        callback: impl FnMut(&VarValue) + Send + 'static,
    ) -> Result<(), VariableError> {
        let variable = self
            .entries
            .get_mut(name)
            .ok_or_else(|| VariableError::NotFound(name.to_string()))?;
        variable.on_change = Some(Box::new(callback));
        Ok(())
    }

    /// Reads the current value of `name`.
    pub fn get(&self, name: &str) -> Result<&VarValue, VariableError> {
        self.entries
            .get(name)
            .map(|variable| &variable.value)
            .ok_or_else(|| VariableError::NotFound(name.to_string()))
    }

    /// Parses `text` according to `name`'s existing type, applies bounds
    /// checking for numeric types, stores the result and runs the change
    /// callback if the assignment succeeds.
    pub fn set(&mut self, name: &str, text: &str) -> Result<(), VariableError> {
        let variable = self
            .entries
            .get_mut(name)
            .ok_or_else(|| VariableError::NotFound(name.to_string()))?;

        let parsed = match &variable.value {
            VarValue::Bool(_) => match text {
                "1" | "true" => VarValue::Bool(true),
                "0" | "false" => VarValue::Bool(false),
                _ => {
                    return Err(VariableError::InvalidValue {
                        name: name.to_string(),
                        text: text.to_string(),
                    });
                }
            },
            VarValue::Int(_) => {
                let value: i64 = text.parse().map_err(|_| VariableError::InvalidValue {
                    name: name.to_string(),
                    text: text.to_string(),
                })?;
                #[allow(clippy::cast_precision_loss)]
                if !variable.bounds.contains(value as f64) {
                    return Err(VariableError::OutOfBounds {
                        name: name.to_string(),
                        #[allow(clippy::cast_precision_loss)]
                        value: value as f64,
                        bounds: variable.bounds,
                    });
                }
                VarValue::Int(value)
            }
            VarValue::Float(_) => {
                let value: f64 = text.parse().map_err(|_| VariableError::InvalidValue {
                    name: name.to_string(),
                    text: text.to_string(),
                })?;
                if !variable.bounds.contains(value) {
                    return Err(VariableError::OutOfBounds {
                        name: name.to_string(),
                        value,
                        bounds: variable.bounds,
                    });
                }
                VarValue::Float(value)
            }
            VarValue::Str(_) => VarValue::Str(text.to_string()),
        };

        variable.value = parsed;
        if let Some(callback) = variable.on_change.as_mut() {
            callback(&variable.value);
        }
        Ok(())
    }

    /// Names of every registered variable, in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::{VarBounds, VarValue, VariableError, Variables};

    #[test]
    fn set_parses_and_stores_typed_values() {
        let mut variables = Variables::new();
        variables.register_bool("dev.flag", false).unwrap();
        variables.set("dev.flag", "true").unwrap();
        assert_eq!(variables.get("dev.flag").unwrap(), &VarValue::Bool(true));
    }

    #[test]
    fn set_rejects_out_of_bounds_numeric_values() {
        let mut variables = Variables::new();
        variables
            .register_float("vol.master", 1.0, VarBounds::new(0.0, 1.0))
            .unwrap();
        let error = variables.set("vol.master", "2.5").unwrap_err();
        assert!(matches!(error, VariableError::OutOfBounds { .. }));
        // The rejected assignment leaves the previous value untouched.
        assert_eq!(variables.get("vol.master").unwrap(), &VarValue::Float(1.0));
    }

    #[test]
    fn set_rejects_invalid_text_for_the_declared_type() {
        let mut variables = Variables::new();
        variables
            .register_int("fov", 90, VarBounds::new(60.0, 120.0))
            .unwrap();
        let error = variables.set("fov", "not-a-number").unwrap_err();
        assert!(matches!(error, VariableError::InvalidValue { .. }));
    }

    #[test]
    fn set_on_unknown_variable_reports_not_found() {
        let mut variables = Variables::new();
        assert!(matches!(
            variables.set("nope", "1"),
            Err(VariableError::NotFound(_))
        ));
    }

    #[test]
    fn change_callback_runs_after_a_successful_assignment() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicI64, Ordering};

        let mut variables = Variables::new();
        variables
            .register_int("sens", 3, VarBounds::UNBOUNDED)
            .unwrap();
        let seen = Arc::new(AtomicI64::new(-1));
        let seen_clone = Arc::clone(&seen);
        variables
            .on_change("sens", move |value| {
                if let VarValue::Int(value) = value {
                    seen_clone.store(*value, Ordering::SeqCst);
                }
            })
            .unwrap();
        variables.set("sens", "7").unwrap();
        assert_eq!(seen.load(Ordering::SeqCst), 7);
    }

    #[test]
    fn registering_the_same_name_twice_fails() {
        let mut variables = Variables::new();
        variables.register_bool("x", false).unwrap();
        assert!(matches!(
            variables.register_bool("x", true),
            Err(VariableError::AlreadyRegistered(_))
        ));
    }
}
