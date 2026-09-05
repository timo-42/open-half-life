//! Deterministic component selection from a runtime-only local recipe.
//!
//! # Why the recipe is not in this repository
//!
//! Which components of a particular lawfully owned edition should be imported,
//! and where they belong, is edition-specific knowledge. A file listing those
//! component names *is itself local proprietary data*, so
//! `docs/MEDIA_IMPORT.md` requires it to be supplied by or generated for the
//! user at runtime and never compiled in or committed. This module therefore
//! ships the *format and the algorithm* and no data: every name in the tests
//! and the documentation below is invented.
//!
//! Consequences that are enforced here rather than merely documented:
//!
//! - a recipe is read from a user-local file at import time
//!   ([`SelectionRecipe::read_from_file`]) and nothing else;
//! - no error, [`core::fmt::Display`] implementation, or public accessor in
//!   this module echoes a component name or a destination back to the caller,
//!   so recipe contents cannot reach a log, a crash report, or a manifest;
//! - what *can* be published is [`SelectionPlan::recipe_identity`], a SHA-256
//!   over the decisions. It binds the staging identity to the exact selection
//!   that produced a payload without revealing what that selection was.
//!
//! # Recipe format
//!
//! ```toml
//! # A user-local file. Never commit one.
//! version = 1
//! default_decision = "exclude"
//!
//! [[component]]
//! name = "sample-fixture-group"
//! decision = "include"
//! destination = "fixtures/sample"
//!
//! [[component]]
//! name = "sample-diagnostic-group"
//! decision = "exclude"
//! ```
//!
//! `version` must be [`RECIPE_FORMAT_VERSION`]. `default_decision` is optional
//! and defaults to `"exclude"`, so a recipe that forgets a component drops it
//! rather than importing it to an unreviewed destination. Each `[[component]]`
//! names a cabinet component or file group, its `decision`, and — for an
//! include — an optional `destination` root that the entry's archive path is
//! appended to. Component names are matched ASCII case-insensitively and must
//! be unique under that folding. Unknown keys are refused, so a typo in
//! `decision` cannot silently fall through to the default.
//!
//! Every bound is checked while parsing: [`MAXIMUM_RECIPE_BYTES`],
//! [`MAXIMUM_RECIPE_COMPONENTS`], [`MAXIMUM_COMPONENT_NAME_BYTES`], and a
//! destination root that must itself be a legal [`PayloadPath`] of at most
//! [`MAXIMUM_DESTINATION_COMPONENTS`] components.
//!
//! # Selection
//!
//! [`select`] is a pure function: the same entries and recipe always produce
//! the same [`SelectionPlan`], in ascending destination
//! [`PayloadPath::portability_key`] order. It runs *before*
//! [`crate::layout::plan_payload_layout`], as `docs/MILESTONES.md` requires —
//! selection decides what exists, layout decides whether what exists can be
//! written safely — and its output is exactly the layout planner's input.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::path::Path;

use ohl_core::StreamingSha256;

use crate::layout::PayloadEntryMetadata;
use crate::path::PayloadPath;

/// The only accepted recipe format version.
pub const RECIPE_FORMAT_VERSION: u64 = 1;

/// The largest accepted recipe file, in bytes.
pub const MAXIMUM_RECIPE_BYTES: usize = 64 * 1024;

/// The largest accepted number of `[[component]]` rules.
pub const MAXIMUM_RECIPE_COMPONENTS: usize = 512;

/// The largest accepted component name, in bytes.
pub const MAXIMUM_COMPONENT_NAME_BYTES: usize = 128;

/// The largest accepted number of components in a destination root.
pub const MAXIMUM_DESTINATION_COMPONENTS: usize = 8;

/// The largest accepted number of entries offered to [`select`].
pub const MAXIMUM_SELECTABLE_ENTRIES: usize = 50_000;

/// The domain separator for the decision digest.
const DIGEST_DOMAIN: &str = "open-half-life-payload-selection";

/// The prefix of a rendered [`SelectionPlan::recipe_identity`].
const IDENTITY_PREFIX: &str = "ohl-selection-v1-sha256:";

/// What a recipe says about one component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionDecision {
    /// The component's entries become payload entries.
    Include,
    /// The component's entries are dropped.
    Exclude,
}

impl SelectionDecision {
    /// The exact spelling accepted and digested for this decision.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
        }
    }

    /// Parses the exact accepted spelling.
    fn parse(value: &str) -> Option<Self> {
        match value {
            "include" => Some(Self::Include),
            "exclude" => Some(Self::Exclude),
            _ => None,
        }
    }
}

/// Why a recipe file may not be used.
///
/// No variant carries recipe content; see the [module documentation](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SelectionRecipeError {
    /// The file could not be read at all.
    ReadFailure,
    /// The file exceeded [`MAXIMUM_RECIPE_BYTES`], or was not UTF-8.
    TooLarge,
    /// The file is not well-formed TOML.
    Malformed,
    /// `version` is absent or is not [`RECIPE_FORMAT_VERSION`].
    UnsupportedVersion,
    /// A required key is missing.
    MissingField,
    /// A key has the wrong type or an unaccepted value.
    InvalidField,
    /// An unknown key appeared.
    UnknownField,
    /// More than [`MAXIMUM_RECIPE_COMPONENTS`] rules.
    TooManyComponents,
    /// Two rules name the same component under ASCII case folding.
    DuplicateComponent,
    /// A `destination` root is not a legal payload path, or is too deep.
    InvalidDestination,
}

impl SelectionRecipeError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::ReadFailure => "selection recipe could not be read",
            Self::TooLarge => "selection recipe exceeds the accepted size",
            Self::Malformed => "selection recipe is not well-formed",
            Self::UnsupportedVersion => "selection recipe version is not supported",
            Self::MissingField => "selection recipe is missing a required field",
            Self::InvalidField => "selection recipe field has an unaccepted value",
            Self::UnknownField => "selection recipe has an unknown field",
            Self::TooManyComponents => "selection recipe has too many components",
            Self::DuplicateComponent => "selection recipe names one component twice",
            Self::InvalidDestination => "selection recipe destination is not a legal root",
        }
    }
}

impl core::fmt::Display for SelectionRecipeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for SelectionRecipeError {}

impl From<SelectionRecipeError> for ohl_core::SanitizedError {
    fn from(error: SelectionRecipeError) -> Self {
        match error {
            SelectionRecipeError::ReadFailure => Self::Internal,
            _ => Self::InvalidInput,
        }
    }
}

/// Why a set of entries could not be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SelectionError {
    /// More than [`MAXIMUM_SELECTABLE_ENTRIES`] entries were offered.
    TooManyEntries,
    /// An included entry's destination root joined with its archive path is
    /// not a legal payload path.
    InvalidDestination,
}

impl SelectionError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::TooManyEntries => "more entries were offered than selection accepts",
            Self::InvalidDestination => "a selected entry has no legal destination",
        }
    }
}

impl core::fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for SelectionError {}

impl From<SelectionError> for ohl_core::SanitizedError {
    fn from(_: SelectionError) -> Self {
        Self::InvalidInput
    }
}

/// One validated rule. Private: the name never leaves this module.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ComponentRule {
    /// The ASCII case-folded name used for lookup and digesting.
    folded_name: String,
    /// The decision for this component.
    decision: SelectionDecision,
    /// The destination root, or `None` for the payload root.
    destination: Option<PayloadPath>,
}

/// A validated runtime-only local selection recipe.
///
/// Construct one with [`SelectionRecipe::parse`] or
/// [`SelectionRecipe::read_from_file`]. The value exposes counts and the
/// default decision, but never a component name or a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionRecipe {
    /// Rules keyed by folded name, which also fixes the digest order.
    rules: BTreeMap<String, ComponentRule>,
    /// What happens to a component with no rule.
    default_decision: SelectionDecision,
}

impl SelectionRecipe {
    /// Parses and validates a recipe from TOML text.
    ///
    /// # Errors
    ///
    /// See [`SelectionRecipeError`].
    pub fn parse(text: &str) -> Result<Self, SelectionRecipeError> {
        if text.len() > MAXIMUM_RECIPE_BYTES {
            return Err(SelectionRecipeError::TooLarge);
        }
        let document = text
            .parse::<toml::Table>()
            .map_err(|_| SelectionRecipeError::Malformed)?;

        let version = document
            .get("version")
            .ok_or(SelectionRecipeError::UnsupportedVersion)?
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(SelectionRecipeError::UnsupportedVersion)?;
        if version != RECIPE_FORMAT_VERSION {
            return Err(SelectionRecipeError::UnsupportedVersion);
        }

        let default_decision = match document.get("default_decision") {
            None => SelectionDecision::Exclude,
            Some(value) => value
                .as_str()
                .and_then(SelectionDecision::parse)
                .ok_or(SelectionRecipeError::InvalidField)?,
        };

        for key in document.keys() {
            if !matches!(key.as_str(), "version" | "default_decision" | "component") {
                return Err(SelectionRecipeError::UnknownField);
            }
        }

        let components = match document.get("component") {
            None => &[][..],
            Some(value) => value
                .as_array()
                .ok_or(SelectionRecipeError::InvalidField)?
                .as_slice(),
        };
        if components.len() > MAXIMUM_RECIPE_COMPONENTS {
            return Err(SelectionRecipeError::TooManyComponents);
        }

        let mut rules = BTreeMap::new();
        for component in components {
            let rule = parse_component(component)?;
            if rules.insert(rule.folded_name.clone(), rule).is_some() {
                return Err(SelectionRecipeError::DuplicateComponent);
            }
        }

        Ok(Self {
            rules,
            default_decision,
        })
    }

    /// Reads a user-local recipe file, refusing anything oversized.
    ///
    /// At most [`MAXIMUM_RECIPE_BYTES`] plus one byte is ever read, so an
    /// enormous or endless file costs a bounded read rather than the process.
    ///
    /// # Errors
    ///
    /// See [`SelectionRecipeError`]. The path is never echoed.
    pub fn read_from_file(path: &Path) -> Result<Self, SelectionRecipeError> {
        let file = fs::File::open(path).map_err(|_| SelectionRecipeError::ReadFailure)?;
        let limit = u64::try_from(MAXIMUM_RECIPE_BYTES).unwrap_or(u64::MAX);
        let mut text = String::new();
        match file.take(limit.saturating_add(1)).read_to_string(&mut text) {
            Ok(_) if text.len() > MAXIMUM_RECIPE_BYTES => Err(SelectionRecipeError::TooLarge),
            Ok(_) => Self::parse(&text),
            // A non-UTF-8 file is refused rather than lossily decoded into a
            // different recipe than the user wrote.
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                Err(SelectionRecipeError::TooLarge)
            }
            Err(_) => Err(SelectionRecipeError::ReadFailure),
        }
    }

    /// The decision applied to a component with no rule.
    pub const fn default_decision(&self) -> SelectionDecision {
        self.default_decision
    }

    /// The number of validated rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// The rule for one component name, folded case-insensitively.
    fn rule_for(&self, component: &str) -> Option<&ComponentRule> {
        self.rules.get(&component.to_ascii_lowercase())
    }
}

/// Validates one `[[component]]` table.
fn parse_component(value: &toml::Value) -> Result<ComponentRule, SelectionRecipeError> {
    let table = value.as_table().ok_or(SelectionRecipeError::InvalidField)?;
    for key in table.keys() {
        if !matches!(key.as_str(), "name" | "decision" | "destination") {
            return Err(SelectionRecipeError::UnknownField);
        }
    }

    let name = table
        .get("name")
        .ok_or(SelectionRecipeError::MissingField)?
        .as_str()
        .ok_or(SelectionRecipeError::InvalidField)?;
    if name.is_empty()
        || name.len() > MAXIMUM_COMPONENT_NAME_BYTES
        || !name.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(SelectionRecipeError::InvalidField);
    }

    let decision = table
        .get("decision")
        .ok_or(SelectionRecipeError::MissingField)?
        .as_str()
        .and_then(SelectionDecision::parse)
        .ok_or(SelectionRecipeError::InvalidField)?;

    // An excluded component's destination is still validated, so a rule that
    // is later flipped to `include` cannot smuggle in an illegal root.
    let destination = match table.get("destination") {
        None => None,
        Some(value) => {
            let text = value.as_str().ok_or(SelectionRecipeError::InvalidField)?;
            if text.is_empty() {
                None
            } else {
                let path = PayloadPath::parse(text)
                    .map_err(|_| SelectionRecipeError::InvalidDestination)?;
                if path.components().count() > MAXIMUM_DESTINATION_COMPONENTS {
                    return Err(SelectionRecipeError::InvalidDestination);
                }
                Some(path)
            }
        }
    };

    Ok(ComponentRule {
        folded_name: name.to_ascii_lowercase(),
        decision,
        destination,
    })
}

/// One entry a reader offers for selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectableEntry {
    /// The opaque, transport-local handle for this entry.
    pub source_token: u64,
    /// The cabinet component or file-group name this entry belongs to.
    pub component: String,
    /// The archive-relative name inside that component.
    pub archive_path: String,
    /// The size the archive declares.
    pub size_bytes: u64,
}

/// The deterministic result of applying a recipe to a set of entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionPlan {
    /// Included entries, in ascending destination collision-key order.
    entries: Vec<PayloadEntryMetadata>,
    /// How many offered entries were dropped.
    excluded: usize,
    /// The digest binding the decisions that produced `entries`.
    digest: [u8; 32],
}

impl SelectionPlan {
    /// The selected entries, ready for [`crate::layout::plan_payload_layout`].
    pub fn entries(&self) -> &[PayloadEntryMetadata] {
        &self.entries
    }

    /// Consumes the plan and returns its entries.
    pub fn into_entries(self) -> Vec<PayloadEntryMetadata> {
        self.entries
    }

    /// How many offered entries the recipe dropped.
    pub const fn excluded_count(&self) -> usize {
        self.excluded
    }

    /// The raw SHA-256 over the decisions that produced this plan.
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    /// The publishable identity of the decisions.
    ///
    /// This is the value to pass as
    /// [`crate::stage::PayloadStageRequest::recipe_identity`]. It is a one-way
    /// digest, so binding it into a staging identity — and therefore into a
    /// cache key — reveals nothing about the recipe's contents while still
    /// guaranteeing that a payload staged under one selection can never be
    /// reused for another.
    pub fn recipe_identity(&self) -> String {
        let mut identity = String::from(IDENTITY_PREFIX);
        for byte in self.digest {
            identity.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
            identity.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
        }
        identity
    }
}

/// Applies `recipe` to `entries`.
///
/// Pure and total: no filesystem access, no ordering dependence on the input,
/// and no hidden state. See the [module documentation](self).
///
/// # Errors
///
/// See [`SelectionError`].
pub fn select(
    entries: &[SelectableEntry],
    recipe: &SelectionRecipe,
) -> Result<SelectionPlan, SelectionError> {
    if entries.len() > MAXIMUM_SELECTABLE_ENTRIES {
        return Err(SelectionError::TooManyEntries);
    }

    let mut selected = Vec::new();
    let mut excluded = 0usize;
    for entry in entries {
        let rule = recipe.rule_for(&entry.component);
        let decision = rule.map_or(recipe.default_decision, |rule| rule.decision);
        if decision == SelectionDecision::Exclude {
            excluded += 1;
            continue;
        }
        let joined = match rule.and_then(|rule| rule.destination.as_ref()) {
            Some(root) => format!("{}/{}", root.as_str(), entry.archive_path),
            None => entry.archive_path.clone(),
        };
        let destination =
            PayloadPath::parse(&joined).map_err(|_| SelectionError::InvalidDestination)?;
        selected.push((destination, entry.source_token, entry.size_bytes));
    }

    selected.sort_by(|first, second| {
        first
            .0
            .portability_key()
            .cmp(second.0.portability_key())
            // Two entries that collide are refused by layout planning, not
            // here; the token keeps this order total until then.
            .then(first.1.cmp(&second.1))
    });

    let digest = decision_digest(recipe);
    Ok(SelectionPlan {
        entries: selected
            .into_iter()
            .map(
                |(destination, source_token, size_bytes)| PayloadEntryMetadata {
                    source_token,
                    archive_path: String::from(destination.as_str()),
                    size_bytes,
                },
            )
            .collect(),
        excluded,
        digest,
    })
}

/// Digests every decision the recipe expresses, in folded-name order.
fn decision_digest(recipe: &SelectionRecipe) -> [u8; 32] {
    /// Length-prefixes one field so no two field boundaries can be confused.
    fn absorb(hash: &mut StreamingSha256, bytes: &[u8]) {
        hash.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(bytes);
    }

    let mut hash = StreamingSha256::new();
    absorb(&mut hash, DIGEST_DOMAIN.as_bytes());
    hash.update(&RECIPE_FORMAT_VERSION.to_be_bytes());
    absorb(&mut hash, recipe.default_decision.as_str().as_bytes());
    hash.update(
        &u64::try_from(recipe.rules.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for rule in recipe.rules.values() {
        absorb(&mut hash, rule.folded_name.as_bytes());
        absorb(&mut hash, rule.decision.as_str().as_bytes());
        absorb(
            &mut hash,
            rule.destination
                .as_ref()
                .map_or("", PayloadPath::as_str)
                .as_bytes(),
        );
    }
    hash.finalize()
}

#[cfg(test)]
mod tests {
    use super::{
        MAXIMUM_COMPONENT_NAME_BYTES, MAXIMUM_RECIPE_BYTES, MAXIMUM_RECIPE_COMPONENTS,
        MAXIMUM_SELECTABLE_ENTRIES, SelectableEntry, SelectionDecision, SelectionError,
        SelectionRecipe, SelectionRecipeError, select,
    };
    use std::fmt::Write as _;

    // Every component name below is invented for these tests.
    const RECIPE: &str = r#"
version = 1
default_decision = "exclude"

[[component]]
name = "sample-fixture-group"
decision = "include"
destination = "fixtures/sample"

[[component]]
name = "sample-shared-group"
decision = "include"

[[component]]
name = "sample-diagnostic-group"
decision = "exclude"
"#;

    fn entry(token: u64, component: &str, path: &str, size: u64) -> SelectableEntry {
        SelectableEntry {
            source_token: token,
            component: String::from(component),
            archive_path: String::from(path),
            size_bytes: size,
        }
    }

    fn offered() -> Vec<SelectableEntry> {
        vec![
            entry(1, "sample-fixture-group", "Second.bin", 20),
            entry(2, "SAMPLE-FIXTURE-GROUP", "First.bin", 10),
            entry(3, "sample-shared-group", "Shared/Third.bin", 30),
            entry(4, "sample-diagnostic-group", "Dropped.bin", 40),
            entry(5, "sample-unlisted-group", "AlsoDropped.bin", 50),
        ]
    }

    #[test]
    fn a_recipe_maps_components_to_decisions_and_destination_roots() {
        let recipe = SelectionRecipe::parse(RECIPE).expect("valid recipe");
        assert_eq!(recipe.rule_count(), 3);
        assert_eq!(recipe.default_decision(), SelectionDecision::Exclude);

        let plan = select(&offered(), &recipe).expect("selected");
        assert_eq!(plan.excluded_count(), 2);
        assert_eq!(
            plan.entries()
                .iter()
                .map(|entry| (entry.source_token, entry.archive_path.as_str()))
                .collect::<Vec<_>>(),
            [
                (2, "fixtures/sample/First.bin"),
                (1, "fixtures/sample/Second.bin"),
                (3, "Shared/Third.bin"),
            ]
        );
        assert_eq!(plan.entries()[0].size_bytes, 10);
        assert_eq!(plan.clone().into_entries().len(), 3);
        assert_eq!(plan.digest().len(), 32);
    }

    #[test]
    fn selection_is_pure_and_order_independent() {
        let recipe = SelectionRecipe::parse(RECIPE).expect("valid recipe");
        let forward = select(&offered(), &recipe).expect("selected");
        let mut reversed = offered();
        reversed.reverse();
        let backward = select(&reversed, &recipe).expect("selected");
        assert_eq!(forward, backward);
        assert_eq!(forward, select(&offered(), &recipe).expect("selected"));
    }

    #[test]
    fn the_identity_binds_the_decisions_and_nothing_else() {
        let recipe = SelectionRecipe::parse(RECIPE).expect("valid recipe");
        let identity = select(&offered(), &recipe)
            .expect("selected")
            .recipe_identity();
        assert!(identity.starts_with("ohl-selection-v1-sha256:"));
        assert_eq!(identity.len(), "ohl-selection-v1-sha256:".len() + 64);
        assert!(
            identity
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b':')
        );
        // No component name survives into the identity.
        assert!(!identity.contains("sample"));

        // The identity depends on the recipe, not on which entries were
        // offered.
        let fewer = select(&offered()[..1], &recipe).expect("selected");
        assert_eq!(fewer.recipe_identity(), identity);

        // Flipping one decision changes it.
        let flipped = SelectionRecipe::parse(&RECIPE.replacen(
            "name = \"sample-diagnostic-group\"\ndecision = \"exclude\"",
            "name = \"sample-diagnostic-group\"\ndecision = \"include\"",
            1,
        ))
        .expect("valid recipe");
        assert_ne!(
            select(&offered(), &flipped)
                .expect("selected")
                .recipe_identity(),
            identity
        );

        // So does moving a destination root.
        let moved = SelectionRecipe::parse(&RECIPE.replacen(
            "destination = \"fixtures/sample\"",
            "destination = \"fixtures/other\"",
            1,
        ))
        .expect("valid recipe");
        assert_ne!(
            select(&offered(), &moved)
                .expect("selected")
                .recipe_identity(),
            identity
        );

        // And so does the default decision.
        let defaulted = SelectionRecipe::parse(&RECIPE.replacen(
            "default_decision = \"exclude\"",
            "default_decision = \"include\"",
            1,
        ))
        .expect("valid recipe");
        assert_ne!(
            select(&offered(), &defaulted)
                .expect("selected")
                .recipe_identity(),
            identity
        );
    }

    #[test]
    fn an_absent_default_decision_drops_unlisted_components() {
        let recipe = SelectionRecipe::parse(
            "version = 1\n[[component]]\nname = \"listed\"\ndecision = \"include\"\n",
        )
        .expect("valid recipe");
        assert_eq!(recipe.default_decision(), SelectionDecision::Exclude);
        let plan = select(
            &[
                entry(1, "listed", "a.bin", 1),
                entry(2, "other", "b.bin", 2),
            ],
            &recipe,
        )
        .expect("selected");
        assert_eq!(plan.entries().len(), 1);
        assert_eq!(plan.excluded_count(), 1);
    }

    #[test]
    fn an_include_default_can_be_narrowed_by_exclusions() {
        let recipe = SelectionRecipe::parse(
            "version = 1\ndefault_decision = \"include\"\n[[component]]\nname = \"drop-me\"\ndecision = \"exclude\"\n",
        )
        .expect("valid recipe");
        let plan = select(
            &[
                entry(1, "keep-me", "a.bin", 1),
                entry(2, "drop-me", "b.bin", 2),
            ],
            &recipe,
        )
        .expect("selected");
        assert_eq!(plan.entries().len(), 1);
        assert_eq!(plan.entries()[0].archive_path, "a.bin");
    }

    #[test]
    fn an_empty_recipe_selects_nothing() {
        let recipe = SelectionRecipe::parse("version = 1\n").expect("valid recipe");
        assert_eq!(recipe.rule_count(), 0);
        let plan = select(&offered(), &recipe).expect("selected");
        assert!(plan.entries().is_empty());
        assert_eq!(plan.excluded_count(), 5);
    }

    #[test]
    fn every_malformed_recipe_is_refused_with_its_own_code() {
        let cases: &[(&str, SelectionRecipeError)] = &[
            ("not toml at all {{{", SelectionRecipeError::Malformed),
            (
                "default_decision = \"include\"\n",
                SelectionRecipeError::UnsupportedVersion,
            ),
            ("version = 2\n", SelectionRecipeError::UnsupportedVersion),
            ("version = -1\n", SelectionRecipeError::UnsupportedVersion),
            (
                "version = \"1\"\n",
                SelectionRecipeError::UnsupportedVersion,
            ),
            (
                "version = 1\ndefault_decision = \"maybe\"\n",
                SelectionRecipeError::InvalidField,
            ),
            (
                "version = 1\nunexpected = true\n",
                SelectionRecipeError::UnknownField,
            ),
            (
                "version = 1\ncomponent = 5\n",
                SelectionRecipeError::InvalidField,
            ),
            (
                "version = 1\n[[component]]\ndecision = \"include\"\n",
                SelectionRecipeError::MissingField,
            ),
            (
                "version = 1\n[[component]]\nname = \"a\"\n",
                SelectionRecipeError::MissingField,
            ),
            (
                "version = 1\n[[component]]\nname = \"\"\ndecision = \"include\"\n",
                SelectionRecipeError::InvalidField,
            ),
            (
                "version = 1\n[[component]]\nname = \"has space\"\ndecision = \"include\"\n",
                SelectionRecipeError::InvalidField,
            ),
            (
                "version = 1\n[[component]]\nname = 7\ndecision = \"include\"\n",
                SelectionRecipeError::InvalidField,
            ),
            (
                "version = 1\n[[component]]\nname = \"a\"\ndecision = \"maybe\"\n",
                SelectionRecipeError::InvalidField,
            ),
            (
                "version = 1\n[[component]]\nname = \"a\"\ndecision = \"include\"\nextra = 1\n",
                SelectionRecipeError::UnknownField,
            ),
            (
                "version = 1\n[[component]]\nname = \"a\"\ndecision = \"include\"\ndestination = \"../escape\"\n",
                SelectionRecipeError::InvalidDestination,
            ),
            (
                "version = 1\n[[component]]\nname = \"a\"\ndecision = \"include\"\ndestination = \"/rooted\"\n",
                SelectionRecipeError::InvalidDestination,
            ),
            (
                "version = 1\n[[component]]\nname = \"a\"\ndecision = \"include\"\ndestination = 4\n",
                SelectionRecipeError::InvalidField,
            ),
            (
                "version = 1\n[[component]]\nname = \"a\"\ndecision = \"include\"\n[[component]]\nname = \"A\"\ndecision = \"exclude\"\n",
                SelectionRecipeError::DuplicateComponent,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(
                SelectionRecipe::parse(text).expect_err("refused"),
                *expected,
                "recipe classified wrongly"
            );
        }

        let deep = format!(
            "version = 1\n[[component]]\nname = \"a\"\ndecision = \"include\"\ndestination = \"{}\"\n",
            ["d"; 9].join("/")
        );
        assert_eq!(
            SelectionRecipe::parse(&deep).expect_err("refused"),
            SelectionRecipeError::InvalidDestination
        );
    }

    #[test]
    fn recipe_bounds_are_enforced() {
        let oversized = format!("version = 1\n# {}\n", "p".repeat(MAXIMUM_RECIPE_BYTES));
        assert_eq!(
            SelectionRecipe::parse(&oversized).expect_err("refused"),
            SelectionRecipeError::TooLarge
        );

        let mut many = String::from("version = 1\n");
        for index in 0..=MAXIMUM_RECIPE_COMPONENTS {
            write!(
                many,
                "[[component]]\nname = \"c{index}\"\ndecision = \"exclude\"\n"
            )
            .expect("writing to a String cannot fail");
        }
        assert_eq!(
            SelectionRecipe::parse(&many).expect_err("refused"),
            SelectionRecipeError::TooManyComponents
        );

        let long_name = format!(
            "version = 1\n[[component]]\nname = \"{}\"\ndecision = \"include\"\n",
            "n".repeat(MAXIMUM_COMPONENT_NAME_BYTES + 1)
        );
        assert_eq!(
            SelectionRecipe::parse(&long_name).expect_err("refused"),
            SelectionRecipeError::InvalidField
        );
    }

    #[test]
    fn a_destination_that_cannot_be_a_payload_path_is_refused() {
        let recipe = SelectionRecipe::parse(
            "version = 1\ndefault_decision = \"include\"\n[[component]]\nname = \"g\"\ndecision = \"include\"\ndestination = \"root\"\n",
        )
        .expect("valid recipe");
        assert_eq!(
            select(&[entry(1, "g", "../escape", 1)], &recipe).expect_err("refused"),
            SelectionError::InvalidDestination
        );
        assert_eq!(
            select(&[entry(1, "g", "nul.txt", 1)], &recipe).expect_err("refused"),
            SelectionError::InvalidDestination
        );
    }

    #[test]
    fn a_recipe_is_read_from_a_user_local_file_within_bounds() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("selection.recipe.toml");
        std::fs::write(&path, RECIPE).expect("write recipe");
        let recipe = SelectionRecipe::read_from_file(&path).expect("read recipe");
        assert_eq!(recipe.rule_count(), 3);

        let missing = directory.path().join("absent.recipe.toml");
        assert_eq!(
            SelectionRecipe::read_from_file(&missing).expect_err("refused"),
            SelectionRecipeError::ReadFailure
        );

        let oversized = directory.path().join("oversized.recipe.toml");
        std::fs::write(&oversized, "p".repeat(MAXIMUM_RECIPE_BYTES + 1)).expect("write");
        assert_eq!(
            SelectionRecipe::read_from_file(&oversized).expect_err("refused"),
            SelectionRecipeError::TooLarge
        );

        let binary = directory.path().join("binary.recipe.toml");
        std::fs::write(&binary, [0xffu8, 0xfe, 0xfd]).expect("write");
        assert_eq!(
            SelectionRecipe::read_from_file(&binary).expect_err("refused"),
            SelectionRecipeError::TooLarge
        );
    }

    #[test]
    fn too_many_offered_entries_are_refused() {
        let recipe = SelectionRecipe::parse("version = 1\n").expect("valid recipe");
        let entries = (0..=MAXIMUM_SELECTABLE_ENTRIES)
            .map(|index| entry(u64::try_from(index).expect("index"), "unlisted", "a.bin", 0))
            .collect::<Vec<_>>();
        assert_eq!(
            select(&entries, &recipe).expect_err("refused"),
            SelectionError::TooManyEntries
        );
    }

    #[test]
    fn no_error_message_echoes_recipe_content() {
        for error in [
            SelectionRecipeError::ReadFailure,
            SelectionRecipeError::TooLarge,
            SelectionRecipeError::Malformed,
            SelectionRecipeError::UnsupportedVersion,
            SelectionRecipeError::MissingField,
            SelectionRecipeError::InvalidField,
            SelectionRecipeError::UnknownField,
            SelectionRecipeError::TooManyComponents,
            SelectionRecipeError::DuplicateComponent,
            SelectionRecipeError::InvalidDestination,
        ] {
            let message = error.to_string();
            assert!(!message.is_empty());
            assert!(!message.contains("sample"));
            let _: ohl_core::SanitizedError = error.into();
        }
        for error in [
            SelectionError::TooManyEntries,
            SelectionError::InvalidDestination,
        ] {
            assert!(!error.to_string().is_empty());
            let _: ohl_core::SanitizedError = error.into();
        }
    }
}
