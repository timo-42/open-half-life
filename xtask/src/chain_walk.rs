//! `cargo xtask chain-walk`: the chained campaign walk.
//!
//! The per-map scenarios `xtask/src/combat_smoke.rs` runs each start at
//! their own map's `info_player_start` with an empty inventory. A real
//! campaign never does that: the player arrives through a level change, at
//! an offset from the destination's `info_landmark`, carrying whatever the
//! previous maps gave them. Routes authored from a cold spawn therefore do
//! not compose, and earlier follow-up investigations (recorded in local
//! notes, not part of the repository) found several maps
//! "blocked" for exactly that reason — the cold load has no campaign state
//! to work with.
//!
//! This command walks the campaign the way the campaign is played: it
//! starts the built `open-half-life` binary once, on
//! `ohl_campaign::STARTMAP`, with one `--chain-script` route per hop (see
//! `crates/ohl-app/src/game_run.rs`'s `run_chained`). Route 0 runs from the
//! start map's own player start; each later route runs from where the
//! preceding route's followed level change put the player down, inside the
//! same process, so `ohl_engine::transition`'s carry machinery — health,
//! armor, weapons, ammo, the suit — is what supplies the campaign state.
//!
//! `--aggregate-only` emits only distinct depth, simulated seconds and a
//! fixed verdict. Compatibility summaries may include the caller's start
//! and inventory and parsed arrival counts; private runs must use an outer
//! in-memory capture boundary as well (clap/build errors belong to it).
//!
//! Legacy route files remain unchanged. The default campaign also accepts
//! `hop-NNNN.txt`, using a zero-based route ordinal. Other starts cannot
//! consume those neutral files. Two files claiming an ordinal are an error;
//! the first gap ends assembly and the existing route cap still applies.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;

/// `cargo xtask chain-walk` command line.
#[derive(Debug, Parser)]
#[command(name = "chain-walk")]
struct Args {
    /// An already-imported payload store root (the directory holding a
    /// published tree's `files/` directory, or one level above it).
    #[arg(long, value_name = "DIR")]
    payload_root: PathBuf,

    /// A prebuilt `open-half-life` binary. Without one, `cargo build -p
    /// ohl-app --release` is run first and the release binary is used.
    #[arg(long, value_name = "PATH")]
    bin: Option<PathBuf>,

    /// The map the chain starts on. Must be a name from `ohl_campaign`'s
    /// own cited table; defaults to [`ohl_campaign::STARTMAP`].
    #[arg(long, value_name = "NAME")]
    start: Option<String>,

    /// How many maps the chain must enter for this command to succeed.
    /// One means "the start map loaded"; two means "at least one level
    /// change was followed and the next map's own route ran".
    #[arg(long, default_value_t = 2)]
    min_depth: usize,

    /// Timeout for the whole chain run, in seconds.
    #[arg(long, default_value_t = 600)]
    timeout: u64,

    /// A `--start-inventory` list handed to the app at the *start* map's
    /// load, carried onward by `ohl_engine::transition` exactly as a
    /// picked-up weapon would be. **Empty by default**: the campaign hands
    /// the player nothing at the start map, and the chain's own routes now
    /// detour to what the maps offer (`ohl_engine::route_plan`'s pickup
    /// detours), so a walk that needs a handout is a walk whose routes are
    /// not doing their job. [`CHAIN_START_INVENTORY`] is the harness aid
    /// this used to default to, kept for an explicit opt-in.
    #[arg(long, value_name = "LIST", default_value = "")]
    start_inventory: String,

    /// Emit only allowlisted whole-chain aggregates and fixed error codes.
    #[arg(long)]
    aggregate_only: bool,
}

/// A loadout `cargo xtask chain-walk` can be *asked* to start with, and
/// which the summary then prints on its own row.
///
/// **Not a default, and not a claim about the campaign: a harness aid.**
/// It exists because the chain's routes once walked from one level change
/// to the next without ever stopping for anything, so a run arrived in
/// the later maps carrying nothing while a player who had walked those
/// same maps would be carrying what the maps handed them. The planner now
/// takes pickup detours of its own (`ohl_engine::route_plan`), so the
/// default is an empty loadout again and what the chain carries is what
/// its routes actually collected — reported per arrival by
/// [`ChainReport::arrivals`]. Passing this list is an explicit opt-in, for
/// telling "the routes cannot reach a weapon" apart from "a weapon would
/// not have been enough". The list itself is two
/// `ohl_combat::classify_classname` classnames.
pub const CHAIN_START_INVENTORY: &str = "weapon_357,ammo_357,ammo_357";

/// The most routes one chain may hold, so a stray file cannot make the
/// walk unbounded.
pub const MAX_CHAIN_ROUTES: usize = 16;

/// Why a chain could not be assembled from `xtask/chain-routes/`.
#[derive(Debug, PartialEq, Eq)]
pub enum AssemblyError {
    /// No `<start>.txt` exists, so the chain has no first route.
    NoStartRoute,
    /// The start map is not a name from `ohl_campaign`'s cited table.
    StartNotInCampaignTable,
    /// Both naming conventions claim the same ordinal.
    DuplicateOrdinal,
}

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateOrdinal => "duplicate-route-ordinal",
            Self::NoStartRoute => "no route file exists for the chain's start map",
            Self::StartNotInCampaignTable => {
                "the chain's start map is not a name from ohl_campaign's cited table"
            }
        })
    }
}

/// Whether `map` is a name this repository may write down: one of
/// `ohl_campaign`'s own publicly sourced chapter or hazard-course map
/// names (`docs/CLEAN_ROOM.md` rule 7).
#[must_use]
pub fn is_campaign_table_name(map: &str) -> bool {
    ohl_campaign::chapter_of(map).is_some()
        || ohl_campaign::chapters::HAZARD_COURSE_MAPS
            .iter()
            .any(|name| name.eq_ignore_ascii_case(map))
}

/// Assembles the chain of route files for `start`, in chain order:
/// `<start>.txt`, then `<start>-hop1.txt`, `-hop2.txt`, ... stopping at
/// the first hop with no file (or at [`MAX_CHAIN_ROUTES`]).
///
/// A gap ends the chain rather than being skipped: hop `n`'s route only
/// means anything if hop `n - 1`'s route actually delivered the player
/// there.
///
/// # Errors
/// [`AssemblyError::StartNotInCampaignTable`] when `start` is not a name
/// from `ohl_campaign`'s cited table, and [`AssemblyError::NoStartRoute`]
/// when the directory holds no route for it.
pub fn assemble_chain(routes_dir: &Path, start: &str) -> Result<Vec<PathBuf>, AssemblyError> {
    if !is_campaign_table_name(start) {
        return Err(AssemblyError::StartNotInCampaignTable);
    }
    let mut routes = Vec::new();
    for hop in 0..MAX_CHAIN_ROUTES {
        let legacy = routes_dir.join(if hop == 0 {
            format!("{start}.txt")
        } else {
            format!("{start}-hop{hop}.txt")
        });
        let neutral = routes_dir.join(format!("hop-{hop:04}.txt"));
        let has_legacy = legacy.is_file();
        let has_neutral = start.eq_ignore_ascii_case(ohl_campaign::STARTMAP) && neutral.is_file();
        match (has_legacy, has_neutral) {
            (true, true) => return Err(AssemblyError::DuplicateOrdinal),
            (true, false) => routes.push(legacy),
            (false, true) => routes.push(neutral),
            (false, false) => break,
        }
    }
    if routes.is_empty() {
        return Err(AssemblyError::NoStartRoute);
    }
    Ok(routes)
}

/// The fixed line `run_chained` logs when a route's level change landed
/// the walk back in a map it had already entered. Always a failure here,
/// whatever depth was reached: a chain that may revisit maps could satisfy
/// any `--min-depth` by ping-ponging across a single boundary.
pub const RE_ENTERED_LINE: &str = "The chain walk re-entered a map it had already visited.";

/// The fixed line `run_chained` logs when a route's level change was
/// followed with the player already dead. Always a failure here, whatever
/// depth was reached: a map that fires its own level change by name does
/// so whether or not the player survived to see it, and a depth aggregate
/// that counted a corpse being carried across a boundary would be worth
/// nothing.
pub const ARRIVED_DEAD_LINE: &str = "The chain walk arrived dead.";

/// The fixed line `run_chained` logs when a `trigger_endsection` ended the
/// game partway through a route: told apart from a route that merely ran
/// out of ticks, though neither is a failure by itself.
pub const SECTION_ENDED_LINE: &str = "The chain walk ended its section.";

/// The five fixed terminal lines `crates/ohl-app/src/game_run.rs`'s
/// `run_chained` ends a chain walk with, in the order this module looks
/// for them.
const TERMINAL_LINES: [&str; 5] = [
    ARRIVED_DEAD_LINE,
    RE_ENTERED_LINE,
    SECTION_ENDED_LINE,
    "The chain walk stopped.",
    "The chain walk has no further route.",
];

/// The fixed prefixes `run_chained`'s two aggregate report lines carry.
const DEPTH_PREFIX: &str = "Chain walk depth: ";
const SECONDS_PREFIX: &str = "Chain walk simulated seconds: ";

/// The fixed prefix `run_chained` gives its per-arrival inventory line,
/// one per map the chain enters after the start map.
const ARRIVAL_PREFIX: &str = "Chain walk arrival ";

/// What one chain run reported, parsed from the app's own fixed lines.
#[derive(Debug, PartialEq)]
pub struct ChainReport {
    /// How many maps the chain entered, the start map included.
    pub depth: usize,
    /// How many simulated seconds the routes that ran took.
    pub seconds: f32,
    /// Which fixed terminal line ended the walk, verbatim, or `None` when
    /// the run ended without logging one at all (a crash, a timeout, or a
    /// load failure).
    pub stopped_at: Option<&'static str>,
    /// Whether the walk ended by re-entering a map it had already
    /// visited, which fails this command regardless of depth.
    pub re_entered: bool,
    /// Whether the walk followed a level change with the player already
    /// dead, which fails this command regardless of depth.
    pub arrived_dead: bool,
    /// How many "A level change was followed." lines the run logged: one
    /// per hop, a cross-check on `depth`.
    pub hops: usize,
    /// What the player carried into each map the chain entered, in
    /// arrival order: the arrival's own index, how many weapons were
    /// owned and how many rounds of every kind were held together.
    ///
    /// This is what says whether the chain's routes are picking anything
    /// up. Counts only — the app logs no weapon or ammo name, and neither
    /// does the summary built from them.
    pub arrivals: Vec<(usize, usize, u32)>,
}

/// Parses one `Chain walk arrival N: weapons W, ammo A.` line's three
/// counts, or `None` when the line is not one.
fn parse_arrival(line: &str) -> Option<(usize, usize, u32)> {
    let rest = line
        .strip_prefix("[info] ")?
        .strip_prefix(ARRIVAL_PREFIX)?
        .strip_suffix('.')?;
    let (arrival, counts) = rest.split_once(": weapons ")?;
    let (weapons, ammo) = counts.split_once(", ammo ")?;
    Some((unsigned(arrival)?, unsigned(weapons)?, unsigned(ammo)?))
}

/// Decimal fields never accept signs, whitespace, arbitrary suffixes or overflow.
pub fn unsigned<T: std::str::FromStr>(value: &str) -> Option<T> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// The app prints nonnegative decimal seconds, never exponents or special values.
pub fn seconds(value: &str) -> Option<f32> {
    let (whole, fraction) = value.split_once('.')?;
    let _: u64 = unsigned(whole)?;
    let _: u64 = unsigned(fraction)?;
    let number: f32 = value.parse().ok()?;
    (number.is_finite() && number >= 0.0).then_some(number)
}

/// Exact known messages only; invalid/missing/duplicate required fields fail closed.
#[must_use]
pub fn parse_report(stderr: &str) -> ChainReport {
    let mut report = ChainReport {
        depth: 0,
        seconds: 0.0,
        stopped_at: None,
        re_entered: false,
        arrived_dead: false,
        hops: 0,
        arrivals: Vec::new(),
    };
    let (mut depth, mut elapsed, mut terminal) = (None, None, None);
    let mut invalid = false;
    for raw in stderr.lines() {
        let Some(line) = raw.strip_prefix("[info] ") else {
            continue;
        };
        if let Some(value) = line.strip_prefix(DEPTH_PREFIX) {
            let value = value.strip_suffix('.').and_then(unsigned::<usize>);
            invalid |= depth.is_some() || value.is_none();
            depth = value;
        } else if let Some(value) = line.strip_prefix(SECONDS_PREFIX) {
            let value = value.strip_suffix('.').and_then(seconds);
            invalid |= elapsed.is_some() || value.is_none();
            elapsed = value;
        } else if line.starts_with("Chain walk depth")
            || line.starts_with("Chain walk simulated seconds")
        {
            invalid = true;
        } else if line.starts_with("The chain walk ") {
            let known = TERMINAL_LINES.into_iter().find(|known| *known == line);
            invalid |= terminal.is_some() || known.is_none();
            terminal = known;
        } else if line == "A level change was followed." {
            report.hops += 1;
        }
        if let Some(arrival) = parse_arrival(raw) {
            report.arrivals.push(arrival);
        }
    }
    report.depth = depth.unwrap_or(0);
    report.seconds = elapsed.unwrap_or(0.0);
    report.re_entered = terminal == Some(RE_ENTERED_LINE);
    report.arrived_dead = terminal == Some(ARRIVED_DEAD_LINE);
    if !invalid
        && depth.is_some_and(|depth| (1..=MAX_CHAIN_ROUTES + 1).contains(&depth))
        && elapsed.is_some()
    {
        report.stopped_at = terminal;
    }
    report
}

/// Compatibility row names, with no caller text or per-arrival measurements.
#[must_use]
pub fn aggregate_summary(report: &ChainReport, pass: bool) -> String {
    format!(
        "| Measure | Value |\n|---|---|\n| Distinct maps reached (chain depth) | {} |\n| Elapsed game seconds | {:.1} |\n| Result | {} |\n",
        report.depth,
        report.seconds,
        if pass { "Pass" } else { "Fail" }
    )
}

/// The summary row naming the loadout a chain run was handed at the start
/// map, shared with `cargo xtask plan-chain-hop` so both summaries label
/// it the same way.
///
/// Which loadout it is matters to whoever reads the row: the documented
/// harness aid ([`CHAIN_START_INVENTORY`]) says "this run was told to
/// carry something", any other list says "and not even the usual
/// something", and `(none)` says the run carried only what its own routes
/// collected.
#[must_use]
pub fn start_inventory_row(start_inventory: Option<&str>) -> String {
    match start_inventory {
        Some(list) if list == CHAIN_START_INVENTORY => {
            format!("| Start inventory (harness aid) | {list} |\n")
        }
        Some(list) => format!("| Start inventory (caller-supplied) | {list} |\n"),
        None => "| Start inventory | (none) |\n".to_string(),
    }
}

/// Renders the aggregate report. Never prints a route file's name or
/// contents, a payload path, or any map name beyond the caller's own
/// `ohl_campaign`-table start name.
#[must_use]
pub fn write_summary(
    start: &str,
    routes: usize,
    report: &ChainReport,
    min_depth: usize,
    start_inventory: Option<&str>,
    elapsed: Duration,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Chain walk summary\n\n");
    let _ = writeln!(out, "Start map: {start} (ohl_campaign's cited table)");
    let _ = writeln!(out, "Wall-clock elapsed: {:.1}s\n", elapsed.as_secs_f64());
    out.push_str("| Measure | Value |\n|---|---|\n");
    let _ = writeln!(out, "| Routes assembled | {routes} |");
    let _ = writeln!(
        out,
        "| Distinct maps reached (chain depth) | {} |",
        report.depth
    );
    let _ = writeln!(out, "| Level changes followed | {} |", report.hops);
    let _ = writeln!(out, "| Elapsed game seconds | {:.1} |", report.seconds);
    let _ = writeln!(
        out,
        "| Stopped at | {} |",
        report.stopped_at.unwrap_or("(no terminal line was logged)")
    );
    let _ = writeln!(out, "| Required depth | {min_depth} |");
    out.push_str(&start_inventory_row(start_inventory));
    // One row per arrival: what the routes had actually collected by the
    // time they walked into that map. Counts only, never a name.
    for (arrival, weapons, ammo) in &report.arrivals {
        let _ = writeln!(
            out,
            "| Inventory on arrival {arrival} | {weapons} weapon(s), {ammo} round(s) |"
        );
    }
    let _ = writeln!(
        out,
        "| Result | {} |",
        if passed(report, min_depth) {
            "Pass"
        } else {
            "Fail"
        }
    );
    out
}

/// Whether a chain run counts as a pass: it entered at least `min_depth`
/// *distinct* maps, never re-entered one it had already been in, and
/// never followed a level change with the player already dead.
#[must_use]
pub fn passed(report: &ChainReport, min_depth: usize) -> bool {
    report.depth >= min_depth
        && report.seconds.is_finite()
        && report.seconds >= 0.0
        && matches!(
            report.stopped_at,
            Some(
                SECTION_ENDED_LINE
                    | "The chain walk stopped."
                    | "The chain walk has no further route."
            )
        )
        && !report.re_entered
        && !report.arrived_dead
}

pub const APP_BIN_NAME: &str = "open-half-life";

/// The `cargo` arguments both chain subcommands build their binary with.
///
/// `--features dev-tools` is not optional here, and both subcommands must
/// pass exactly this list. The flags these commands drive the app with —
/// `--plan-route` for `plan-chain-hop`, `--start-inventory` for a chain
/// walk that has to arrive somewhere with something in hand — exist only
/// in a `dev-tools` build, and both write their result to the *same*
/// `target/release/open-half-life`. Two subcommands building that path
/// with different feature sets means whichever ran last decides whether
/// the other one's flags exist at all, which is exactly how a chain walk
/// came to report depth 0 on a clean tree ("unexpected argument
/// `--start-inventory`") while passing whenever a `plan-chain-hop` build
/// happened to have gone first.
pub const CHAIN_BINARY_BUILD_ARGS: [&str; 6] = [
    "build",
    "-p",
    "ohl-app",
    "--release",
    "--features",
    "dev-tools",
];

/// The fixed error both subcommands report when that build fails.
const BUILD_FAILED: &str = "cargo build -p ohl-app --release --features dev-tools failed";

/// Builds the release `open-half-life` binary both chain subcommands
/// drive, with [`CHAIN_BINARY_BUILD_ARGS`], and returns its path.
pub fn build_chain_binary(root: &Path) -> Result<PathBuf, &'static str> {
    let status = Command::new("cargo")
        .args(CHAIN_BINARY_BUILD_ARGS)
        .current_dir(root)
        .status()
        .map_err(|_| BUILD_FAILED)?;
    if !status.success() {
        return Err(BUILD_FAILED);
    }
    let name = if cfg!(windows) {
        format!("{APP_BIN_NAME}.exe")
    } else {
        APP_BIN_NAME.to_string()
    };
    Ok(root.join("target").join("release").join(name))
}

#[cfg(test)]
fn help_lists_start_inventory(help: &str) -> bool {
    help.contains("--start-inventory")
}

/// The exact argument list the chain entrypoint drives the app with.
///
/// A pure function so a test can read it: whether `--start-inventory` is
/// passed at all is the difference between a walk and an "unexpected
/// argument" on a binary that does not have it, so it is worth asserting
/// rather than assuming.
fn chain_app_args(
    payload_root: &Path,
    start: &str,
    routes: &[PathBuf],
    start_inventory: Option<&str>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        OsString::from("--payload-root"),
        payload_root.as_os_str().to_owned(),
        OsString::from("--map"),
        OsString::from(start),
    ];
    for route in routes {
        args.push(OsString::from("--chain-script"));
        args.push(route.as_os_str().to_owned());
    }
    // Omitted entirely for an empty list, so a walk that wants nothing in
    // hand never depends on the flag existing.
    if let Some(list) = start_inventory {
        args.push(OsString::from("--start-inventory"));
        args.push(OsString::from(list));
    }
    args.push(OsString::from("--script-log"));
    args
}

/// Owned child outcome shared by both tools. Success-looking stderr is
/// never proof of completion; callers require `Exited(0)` as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildEnd {
    Exited(i32),
    TimedOut,
    SpawnFailed,
    WaitFailed,
    ReadFailed,
    OutputLimitExceeded,
}

impl ChildEnd {
    pub fn code(self) -> &'static str {
        match self {
            Self::Exited(0) => "child-ok",
            Self::Exited(_) => "child-exit",
            Self::TimedOut => "child-timeout",
            Self::SpawnFailed => "child-spawn",
            Self::WaitFailed => "child-wait",
            Self::ReadFailed => "child-read",
            Self::OutputLimitExceeded => "child-output-limit",
        }
    }
}

#[derive(Debug)]
pub struct CapturedRun {
    pub end: ChildEnd,
    pub stderr: String,
}

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

fn read_bounded(mut reader: impl std::io::Read, limit: usize) -> Result<String, ChildEnd> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk).map_err(|_| ChildEnd::ReadFailed)?;
        if count == 0 {
            break;
        }
        if count > limit.saturating_sub(bytes.len()) {
            return Err(ChildEnd::OutputLimitExceeded);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes).map_err(|_| ChildEnd::ReadFailed)
}

pub fn capture_stderr(command: Command, timeout: Duration) -> CapturedRun {
    capture_limited(command, timeout, OUTPUT_LIMIT)
}

fn capture_limited(mut command: Command, timeout: Duration, limit: usize) -> CapturedRun {
    let failed = |end| CapturedRun {
        end,
        stderr: String::new(),
    };
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return failed(ChildEnd::SpawnFailed);
    };
    capture_spawned(&mut child, timeout, limit)
}

fn capture_spawned(
    child: &mut std::process::Child,
    timeout: Duration,
    limit: usize,
) -> CapturedRun {
    let failed = |end| CapturedRun {
        end,
        stderr: String::new(),
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return failed(ChildEnd::ReadFailed);
    };
    let (send, receive) = mpsc::channel();
    // A reader may outlive the deadline if another process inherited its pipe.
    // It owns at most `limit` bytes and never prevents the child from being reaped.
    let _reader = thread::spawn(move || {
        let _ = send.send(read_bounded(stderr, limit));
    });
    let started = Instant::now();
    let (mut end, mut captured) = (None, None);
    loop {
        match receive.try_recv() {
            Ok(Ok(text)) => captured = Some(text),
            Ok(Err(error)) => {
                let _ = child.kill();
                let _ = child.wait();
                return failed(error);
            }
            Err(mpsc::TryRecvError::Disconnected) if captured.is_none() => {
                let _ = child.kill();
                let _ = child.wait();
                return failed(ChildEnd::ReadFailed);
            }
            Err(_) => {}
        }
        if end.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => end = Some(ChildEnd::Exited(status.code().unwrap_or(-1))),
                Ok(None) => {}
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return failed(ChildEnd::WaitFailed);
                }
            }
        }
        if let Some(end) = end
            && let Some(stderr) = captured.take()
        {
            return CapturedRun { end, stderr };
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return failed(ChildEnd::TimedOut);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Entry point for `cargo xtask chain-walk`, given the arguments after the
/// subcommand name.
pub fn run(root: &Path, raw_args: &[String]) -> ExitCode {
    run_with(root, raw_args, &mut std::io::stdout(), capture_stderr)
}

fn run_with(
    root: &Path,
    raw_args: &[String],
    output: &mut impl std::io::Write,
    mut capture: impl FnMut(Command, Duration) -> CapturedRun,
) -> ExitCode {
    let args = match Args::try_parse_from(
        std::iter::once("chain-walk".to_string()).chain(raw_args.iter().cloned()),
    ) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(2);
        }
    };
    let start = args
        .start
        .clone()
        .unwrap_or_else(|| ohl_campaign::STARTMAP.to_string());
    let Ok(routes) = assemble_chain(&root.join("xtask/chain-routes"), &start) else {
        let _ = writeln!(output, "error: chain-assembly");
        return ExitCode::FAILURE;
    };
    let bin = if let Some(bin) = args.bin {
        bin
    } else {
        let Ok(bin) = build_chain_binary(root) else {
            let _ = writeln!(output, "error: child-build");
            return ExitCode::FAILURE;
        };
        bin
    };
    let inventory = Some(args.start_inventory.as_str()).filter(|list| !list.is_empty());
    let mut command = Command::new(&bin);
    command.args(chain_app_args(
        &args.payload_root,
        &start,
        &routes,
        inventory,
    ));
    let started = Instant::now();
    let captured = capture(command, Duration::from_secs(args.timeout));
    let mut report = parse_report(&captured.stderr);
    let pass = captured.end == ChildEnd::Exited(0) && passed(&report, args.min_depth);
    if captured.end != ChildEnd::Exited(0) {
        report.stopped_at = None;
        let _ = writeln!(output, "error: {}", captured.end.code());
    }
    let summary = if args.aggregate_only {
        aggregate_summary(&report, pass)
    } else {
        write_summary(
            &start,
            routes.len(),
            &report,
            args.min_depth,
            inventory,
            started.elapsed(),
        )
    };
    let _ = write!(output, "{summary}");
    if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The build both chain subcommands run must ask for `dev-tools`.
    ///
    /// Without it the app has no `--start-inventory` (nor `--plan-route`),
    /// and a chain walk on a clean tree dies with "unexpected argument"
    /// before it loads a map — reporting depth 0 as though the walk had
    /// gone nowhere.
    #[test]
    fn the_chain_binary_is_built_with_dev_tools() {
        assert!(
            CHAIN_BINARY_BUILD_ARGS
                .windows(2)
                .any(|pair| pair == ["--features", "dev-tools"]),
            "both chain subcommands build the app with dev-tools"
        );
        assert!(CHAIN_BINARY_BUILD_ARGS.contains(&"--release"));
    }

    #[test]
    fn an_empty_loadout_never_passes_the_start_inventory_flag() {
        let routes = [PathBuf::from("c0a0.txt")];
        let args = chain_app_args(Path::new("/payload"), "c0a0", &routes, None);
        assert!(
            !args.iter().any(|arg| arg == "--start-inventory"),
            "a walk with nothing in hand must not need the flag to exist"
        );
        assert!(args.iter().any(|arg| arg == "--chain-script"));
        assert!(args.iter().any(|arg| arg == "--script-log"));
    }

    #[test]
    fn a_loadout_is_passed_through_verbatim() {
        let routes = [PathBuf::from("c0a0.txt")];
        let args = chain_app_args(
            Path::new("/payload"),
            "c0a0",
            &routes,
            Some(CHAIN_START_INVENTORY),
        );
        let index = args
            .iter()
            .position(|arg| arg == "--start-inventory")
            .expect("the flag is passed");
        assert_eq!(args[index + 1], OsString::from(CHAIN_START_INVENTORY));
    }

    /// The preflight reads the binary's own `--help`, so a `--bin` built
    /// without `dev-tools` is reported as such instead of running and
    /// printing a table that looks like a walk which went nowhere.
    #[test]
    fn a_help_text_without_the_flag_is_recognised() {
        assert!(help_lists_start_inventory(
            "Options:\n  --start-inventory <LIST>\n  --script-log\n"
        ));
        assert!(!help_lists_start_inventory(
            "Options:\n  --chain-script <PATH>\n  --script-log\n"
        ));
    }

    fn touch(directory: &Path, name: &str) {
        std::fs::write(directory.join(name), "1 wait\n").expect("write a route file");
    }

    #[test]
    fn assembles_the_start_route_and_every_consecutive_hop() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path();
        touch(path, "c0a0.txt");
        touch(path, "c0a0-hop1.txt");
        touch(path, "c0a0-hop2.txt");

        let routes = assemble_chain(path, "c0a0").expect("the chain assembles");
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0], path.join("c0a0.txt"));
        assert_eq!(routes[2], path.join("c0a0-hop2.txt"));
    }

    #[test]
    fn a_gap_ends_the_chain_rather_than_being_skipped() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path();
        touch(path, "c0a0.txt");
        touch(path, "c0a0-hop1.txt");
        // No `-hop2.txt`: hop 3's route describes a map hop 2 never
        // delivered the player to, so it must not be picked up.
        touch(path, "c0a0-hop3.txt");

        let routes = assemble_chain(path, "c0a0").expect("the chain assembles");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn a_missing_start_route_is_an_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        touch(directory.path(), "c0a0-hop1.txt");
        assert_eq!(
            assemble_chain(directory.path(), "c0a0"),
            Err(AssemblyError::NoStartRoute)
        );
    }

    #[test]
    fn a_start_map_outside_the_cited_table_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        touch(directory.path(), "not_a_real_map.txt");
        assert_eq!(
            assemble_chain(directory.path(), "not_a_real_map"),
            Err(AssemblyError::StartNotInCampaignTable)
        );
        assert!(is_campaign_table_name(ohl_campaign::STARTMAP));
        assert!(is_campaign_table_name(ohl_campaign::TRAINMAP));
    }

    #[test]
    fn the_shipped_chain_assembles_and_reaches_at_least_two_maps_worth_of_routes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lives one directory below the workspace root");
        let routes_dir = root.join("xtask").join("chain-routes");
        let routes =
            assemble_chain(&routes_dir, ohl_campaign::STARTMAP).expect("the shipped chain exists");
        assert!(
            routes.len() >= 2,
            "the shipped chain must be able to reach a second map"
        );
        for route in &routes {
            let bytes = std::fs::read(route).expect("a shipped route file is readable");
            assert!(
                !bytes.is_empty(),
                "a shipped route file must schedule some ticks"
            );
        }
    }

    #[test]
    fn parses_the_apps_fixed_report_lines() {
        let stderr = "[info] Scripted input loaded.\n\
             [info] A level change was followed.\n\
             [info] Scripted input finished.\n\
             [info] The chain walk stopped.\n\
             [info] Chain walk depth: 2.\n\
             [info] Chain walk simulated seconds: 61.5.\n";
        let report = parse_report(stderr);
        assert_eq!(report.depth, 2);
        assert_eq!(report.hops, 1);
        assert!((report.seconds - 61.5).abs() < f32::EPSILON);
        assert_eq!(report.stopped_at, Some("The chain walk stopped."));
        assert!(!report.re_entered);
    }

    /// A dead arrival fails whatever depth was reached: a map that fires
    /// its own level change by name does so with or without a live
    /// player, and the depth aggregate must not grow off a corpse.
    #[test]
    fn a_walk_that_arrived_dead_fails_however_deep_it_got() {
        let stderr = "[info] A level change was followed.\n\
             [info] The player died.\n\
             [info] The chain walk arrived dead.\n\
             [info] Chain walk depth: 12.\n\
             [info] Chain walk simulated seconds: 660.8.\n";
        let report = parse_report(stderr);
        assert!(report.arrived_dead);
        assert_eq!(report.stopped_at, Some(ARRIVED_DEAD_LINE));
        assert!(
            !passed(&report, 2),
            "a dead arrival is a failure at any depth"
        );
        let summary = write_summary("c0a0", 11, &report, 2, None, Duration::from_secs(9));
        assert!(summary.contains("| Result | Fail |"));
        assert!(summary.contains("| Stopped at | The chain walk arrived dead. |"));
    }

    /// The harness aid is still a documented, parseable loadout — it is
    /// no longer a *default*, which is the whole point of this row of the
    /// summary: a walk that carries something says where it came from.
    #[test]
    fn the_harness_loadout_is_an_opt_in_not_a_default() {
        let args = Args::try_parse_from(["chain-walk", "--payload-root", "."])
            .expect("the command line parses");
        assert!(
            args.start_inventory.is_empty(),
            "the chain walks with what its routes collect"
        );
        let opted_in = Args::try_parse_from([
            "chain-walk",
            "--payload-root",
            ".",
            "--start-inventory",
            CHAIN_START_INVENTORY,
        ])
        .expect("the command line parses");
        assert_eq!(opted_in.start_inventory, CHAIN_START_INVENTORY);
        let summary = write_summary(
            "c0a0",
            2,
            &parse_report(""),
            2,
            Some(CHAIN_START_INVENTORY),
            Duration::from_secs(1),
        );
        assert!(summary.contains("| Start inventory (harness aid) |"));
    }

    /// The measurement this command exists to make honest: what the
    /// chain's own routes had collected by the time they walked into each
    /// map. Counts only — the app logs no weapon or ammo name.
    #[test]
    fn the_per_arrival_inventory_is_parsed_and_reported() {
        let stderr = "[info] A level change was followed.\n\
             [info] Chain walk arrival 2: weapons 1, ammo 34.\n\
             [info] A level change was followed.\n\
             [info] Chain walk arrival 3: weapons 2, ammo 52.\n\
             [info] The chain walk has no further route.\n\
             [info] Chain walk depth: 3.\n\
             [info] Chain walk simulated seconds: 120.0.\n";
        let report = parse_report(stderr);
        assert_eq!(report.arrivals, vec![(2, 1, 34), (3, 2, 52)]);
        let summary = write_summary("c0a0", 3, &report, 2, None, Duration::from_secs(9));
        assert!(summary.contains("| Inventory on arrival 2 | 1 weapon(s), 34 round(s) |"));
        assert!(summary.contains("| Inventory on arrival 3 | 2 weapon(s), 52 round(s) |"));
        // No start inventory was passed, and the summary says so rather
        // than quietly implying one.
        assert!(summary.contains("| Start inventory | (none) |"));
    }

    /// A walk whose routes collected nothing is reported as exactly that,
    /// rather than as a walk with no inventory line at all.
    #[test]
    fn an_empty_handed_arrival_is_reported_as_zeroes() {
        let report = parse_report("[info] Chain walk arrival 2: weapons 0, ammo 0.\n");
        assert_eq!(report.arrivals, vec![(2, 0, 0)]);
        let summary = write_summary("c0a0", 2, &report, 2, None, Duration::from_secs(1));
        assert!(summary.contains("| Inventory on arrival 2 | 0 weapon(s), 0 round(s) |"));
    }

    /// The arrival line is written by the app and read here, and the two
    /// share no crate, so this ties them together through the app's own
    /// source: the prefix it formats the line with must be
    /// [`ARRIVAL_PREFIX`], and the line it formats must be the shape
    /// [`parse_arrival`] reads.
    #[test]
    fn the_arrival_line_parsed_here_is_the_one_the_app_writes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lives one directory below the workspace root");
        let source = std::fs::read_to_string(
            root.join("crates")
                .join("ohl-app")
                .join("src")
                .join("game_run.rs"),
        )
        .expect("the app's chain runner is readable");
        assert!(
            source.contains(&format!(
                "const CHAIN_ARRIVAL_PREFIX: &str = {ARRIVAL_PREFIX:?};"
            )),
            "the app's arrival prefix is this parser's"
        );
        assert!(
            source.contains("\"{CHAIN_ARRIVAL_PREFIX}{index}: weapons {weapons}, ammo {ammo}.\""),
            "the app's arrival line has the shape this parser reads"
        );
        let line = format!("[info] {ARRIVAL_PREFIX}3: weapons 1, ammo 18.");
        assert_eq!(parse_arrival(&line), Some((3, 1, 18)));
    }

    /// A walk a `trigger_endsection` ended says so on its own row, not as
    /// a route that ran out of ticks — and the line it reads is the one
    /// the app writes.
    #[test]
    fn a_section_that_ended_the_walk_is_its_own_stopping_reason() {
        let stderr = "[info] A level change was followed.\n\
             [info] The section ended.\n\
             [info] The chain walk ended its section.\n\
             [info] Chain walk depth: 2.\n\
             [info] Chain walk simulated seconds: 30.0.\n";
        let report = parse_report(stderr);
        assert_eq!(report.stopped_at, Some(SECTION_ENDED_LINE));
        let summary = write_summary("c0a0", 2, &report, 2, None, Duration::from_secs(1));
        assert!(summary.contains("| Stopped at | The chain walk ended its section. |"));
        assert!(summary.contains("| Result | Pass |"));

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lives one directory below the workspace root");
        let source = std::fs::read_to_string(
            root.join("crates")
                .join("ohl-app")
                .join("src")
                .join("game_run.rs"),
        )
        .expect("the app's chain runner is readable");
        assert!(
            source.contains(&format!(
                "const CHAIN_SECTION_ENDED: &str = {SECTION_ENDED_LINE:?};"
            )),
            "the app's section-ended line is this parser's"
        );
    }

    /// Every loadout is named on its own row, and labelled for what it
    /// is: the harness aid, some other list, or nothing.
    #[test]
    fn the_start_inventory_row_says_which_loadout_it_was() {
        assert_eq!(
            start_inventory_row(Some(CHAIN_START_INVENTORY)),
            format!("| Start inventory (harness aid) | {CHAIN_START_INVENTORY} |\n")
        );
        assert_eq!(
            start_inventory_row(Some("weapon_shotgun")),
            "| Start inventory (caller-supplied) | weapon_shotgun |\n"
        );
        assert_eq!(start_inventory_row(None), "| Start inventory | (none) |\n");
    }

    /// A line that is not an arrival line contributes nothing.
    #[test]
    fn an_unrelated_line_is_not_read_as_an_arrival() {
        assert!(parse_arrival("[info] The chain walk stopped.").is_none());
        assert!(parse_arrival("[info] Chain walk depth: 3.").is_none());
        assert!(parse_arrival("[info] Chain walk arrival 2: weapons one, ammo 3.").is_none());
    }

    #[test]
    fn a_run_that_logged_nothing_parses_as_depth_zero() {
        let report = parse_report("");
        assert_eq!(report.depth, 0);
        assert_eq!(report.hops, 0);
        assert_eq!(report.stopped_at, None);
    }

    #[test]
    fn the_summary_reports_aggregates_only() {
        let report = ChainReport {
            depth: 2,
            seconds: 61.5,
            stopped_at: Some("The chain walk stopped."),
            re_entered: false,
            arrived_dead: false,
            hops: 1,
            arrivals: vec![(2, 1, 34)],
        };
        let summary = write_summary("c0a0", 2, &report, 2, None, Duration::from_secs(9));
        assert!(summary.contains("| Distinct maps reached (chain depth) | 2 |"));
        assert!(summary.contains("| Elapsed game seconds | 61.5 |"));
        assert!(summary.contains("| Stopped at | The chain walk stopped. |"));
        assert!(summary.contains("| Result | Pass |"));
        // No route file name, no payload path, no destination map name.
        assert!(!summary.contains(".txt"));
        assert!(!summary.contains('/'));
        assert!(!summary.contains("hop1"));
    }

    #[test]
    fn the_summary_fails_below_the_required_depth() {
        let report = ChainReport {
            depth: 1,
            seconds: 3.0,
            stopped_at: Some("The chain walk stopped."),
            re_entered: false,
            arrived_dead: false,
            hops: 0,
            arrivals: Vec::new(),
        };
        let summary = write_summary("c0a0", 2, &report, 2, None, Duration::from_secs(1));
        assert!(summary.contains("| Result | Fail |"));
    }

    #[test]
    fn a_re_entry_fails_however_deep_the_walk_got() {
        // The exact shape a ping-pong across one boundary produces: the
        // depth requirement is met, but a map repeated, so the walk made
        // no real progress and this command must not call it a pass.
        let report = ChainReport {
            depth: 2,
            seconds: 42.1,
            stopped_at: Some(RE_ENTERED_LINE),
            re_entered: true,
            arrived_dead: false,
            hops: 2,
            arrivals: vec![(2, 0, 0)],
        };
        assert!(!passed(&report, 2));
        let summary = write_summary("c0a0", 2, &report, 2, None, Duration::from_secs(3));
        assert!(summary.contains("| Result | Fail |"));
        assert!(summary.contains(RE_ENTERED_LINE));
    }

    #[test]
    fn the_re_entry_line_is_recognised_when_parsing() {
        let stderr = format!(
            "[info] A level change was followed.\n\
             [info] A level change was followed.\n\
             [info] {RE_ENTERED_LINE}\n\
             [info] Chain walk depth: 2.\n\
             [info] Chain walk simulated seconds: 42.1.\n"
        );
        let report = parse_report(&stderr);
        assert!(report.re_entered);
        assert_eq!(report.stopped_at, Some(RE_ENTERED_LINE));
        // Two hops, but only two distinct maps: the aggregate the app
        // reports is the distinct one, and the hop count exposes the gap.
        assert_eq!(report.hops, 2);
        assert_eq!(report.depth, 2);
        assert!(!passed(&report, 2));
    }
}

#[cfg(test)]
pub(crate) mod child_fixtures {
    use super::*;

    pub const POISONS: [&str; 7] = [
        "synthetic-start-secret",
        "synthetic-loadout-secret",
        "synthetic/path-secret",
        "synthetic-warning-secret",
        "synthetic-suffix-secret",
        "987654",
        "876543",
    ];

    pub fn command(case: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "chain_walk::child_fixtures::child_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("OHL_SYNTHETIC_CHILD", case);
        command
    }

    /// Executed only by a spawned copy of this test executable, on every OS.
    #[test]
    #[ignore = "synthetic child entrypoint"]
    fn child_helper() {
        use std::io::Write as _;
        let Ok(case) = std::env::var("OHL_SYNTHETIC_CHILD") else {
            return;
        };
        let mut stderr = std::io::stderr().lock();
        let chain = "[info] The chain walk has no further route.\n[info] Chain walk depth: 2.\n[info] Chain walk simulated seconds: 1.0.\n";
        let planner = "[info] Route plan cells: 1.\n[info] Route plan segments: 1.\n[info] Route plan ladder climbs: 0.\n[info] Route plan pickup detours: 0.\n[info] Route plan door presses: 0.\n[info] Route plan replay attempts: 1.\n[info] Route plan simulated seconds: 1.0.\n[info] Route plan written.\n";
        writeln!(stderr, "[warn] {}", POISONS[3]).expect("stderr");
        writeln!(stderr, "[info] Synthetic diagnostic: {}", POISONS[4]).expect("stderr");
        writeln!(
            stderr,
            "[info] Chain walk arrival 2: weapons {}, ammo {}.",
            POISONS[5], POISONS[6]
        )
        .expect("stderr");
        write!(stderr, "{chain}{planner}").expect("stderr");
        stderr.flush().expect("flush");
        if let Some(ready) = std::env::var_os("OHL_SYNTHETIC_READY") {
            std::fs::write(ready, b"markers flushed").expect("ready");
        }
        if let Some(out) = std::env::var_os("OHL_SYNTHETIC_OUT")
            && case != "missing"
        {
            let body = if case == "grammar" {
                "1 forward synthetic-suffix-secret\n"
            } else {
                "1 forward\n"
            };
            std::fs::write(
                out,
                format!("{}{body}", crate::plan_chain_hop::candidate_header()),
            )
            .expect("candidate");
        }
        match case.as_str() {
            "invalid-utf8" => {
                stderr.write_all(&[0xff]).expect("stderr");
            }
            "nonzero" => std::process::exit(7),
            "timeout" => std::thread::sleep(Duration::from_secs(10)),
            "overflow" => {
                for _ in 0..512 {
                    stderr.write_all(&[b'x'; 8192]).expect("stderr");
                }
            }
            "suffix" => {
                writeln!(stderr, "[info] Route plan written. {}", POISONS[4]).expect("stderr");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::child_fixtures::{POISONS, command};
    use super::*;

    #[test]
    fn chain_child_late_nonzero_and_overflow_fail_closed() {
        let late = capture_stderr(command("nonzero"), Duration::from_secs(5));
        assert_eq!(late.end, ChildEnd::Exited(7));
        assert!(passed(&parse_report(&late.stderr), 2));
        let overflow = capture_limited(command("overflow"), Duration::from_secs(5), 1024);
        assert_eq!(overflow.end, ChildEnd::OutputLimitExceeded);
        assert!(overflow.stderr.is_empty());
        let overflow_default = capture_stderr(command("overflow"), Duration::from_secs(5));
        assert_eq!(overflow_default.end, ChildEnd::OutputLimitExceeded);
    }

    #[test]
    fn chain_child_timeout_and_overflow_are_killed_and_reaped() {
        for (case, limit, expected) in [
            ("timeout", OUTPUT_LIMIT, ChildEnd::TimedOut),
            ("overflow", 1024, ChildEnd::OutputLimitExceeded),
        ] {
            let directory = tempfile::tempdir().expect("ready directory");
            let ready = directory.path().join("ready.txt");
            let mut child = command(case)
                .env("OHL_SYNTHETIC_READY", &ready)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn");
            let captured = capture_spawned(&mut child, Duration::from_secs(1), limit);
            assert!(ready.is_file(), "valid markers were flushed before failure");
            assert_eq!(captured.end, expected);
            let status = child
                .try_wait()
                .expect("wait after capture")
                .expect("child terminated");
            assert!(!status.success());
            assert_eq!(child.wait().expect("reaped child cached status"), status);
        }
    }

    #[test]
    fn chain_child_spawn_and_read_errors_are_typed() {
        struct Broken;
        impl std::io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("synthetic read failure"))
            }
        }
        let captured = capture_stderr(
            Command::new("synthetic-missing-child-executable"),
            Duration::from_secs(1),
        );
        assert_eq!(captured.end, ChildEnd::SpawnFailed);
        assert_eq!(read_bounded(Broken, 10), Err(ChildEnd::ReadFailed));
        assert_eq!(read_bounded(&[0xff][..], 10), Err(ChildEnd::ReadFailed));
        let malformed = capture_stderr(command("invalid-utf8"), Duration::from_secs(5));
        assert_eq!(malformed.end, ChildEnd::ReadFailed);
        let mut no_pipe = command("valid")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn without pipe");
        assert_eq!(
            capture_spawned(&mut no_pipe, Duration::from_secs(5), OUTPUT_LIMIT).end,
            ChildEnd::ReadFailed
        );
        assert!(no_pipe.try_wait().expect("wait after capture").is_some());
    }

    #[test]
    fn chain_reports_require_exact_complete_unique_terminal_and_numbers() {
        let valid = "[info] The chain walk stopped.\n[info] Chain walk depth: 12.\n[info] Chain walk simulated seconds: 660.8.\n";
        assert!(passed(&parse_report(valid), 12));
        for terminal in TERMINAL_LINES {
            let text = valid.replace("The chain walk stopped.", terminal);
            assert_eq!(
                passed(&parse_report(&text), 12),
                terminal != RE_ENTERED_LINE && terminal != ARRIVED_DEAD_LINE
            );
        }
        for text in [
            valid.replace("[info] The chain walk stopped.\n", ""),
            format!("{valid}[info] The chain walk stopped.\n"),
            format!("{valid}[info] The chain walk arrived dead.\n"),
            format!("{valid}[info] Chain walk depth: 12.\n"),
            format!("{valid}[info] Chain walk simulated seconds: 660.8.\n"),
            valid.replace("stopped.", "stopped. synthetic-suffix-secret"),
            valid.replace("12.", "12. synthetic-suffix-secret"),
            valid.replace("660.8.", "NaN."),
            valid.replace("660.8.", "inf."),
            valid.replace("660.8.", "-1.0."),
            valid.replace("[info]", "synthetic-prefix [info]"),
            valid.replace("12.", "999999999999999999999999999999."),
        ] {
            assert!(
                !passed(&parse_report(&text), 12),
                "malformed synthetic report accepted"
            );
        }
        assert!(!passed(&parse_report(&valid.replace("12.", "11.")), 12));
        let generic = parse_report(&valid.replace("12.", "2."));
        assert!(passed(&generic, 2));
        assert!(!passed(&generic, 12));
        let args = Args::try_parse_from(["chain-walk", "--payload-root", "."]).expect("args");
        assert_eq!(args.min_depth, 2);
    }

    #[test]
    fn chain_neutral_assembly_preserves_order_scope_gaps_cap_and_rejects_duplicates() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path();
        let start = ohl_campaign::STARTMAP;
        let legacy = path.join(format!("{start}.txt"));
        std::fs::write(&legacy, b"1 wait\n").expect("route");
        std::fs::write(path.join("hop-0001.txt"), b"2 wait\n").expect("route");
        std::fs::write(path.join("hop-0003.txt"), b"4 wait\n").expect("route");
        let routes = assemble_chain(path, start).expect("mixed assembly");
        assert_eq!(routes, vec![legacy.clone(), path.join("hop-0001.txt")]);
        assert_eq!(std::fs::read(&legacy).expect("old bytes"), b"1 wait\n");
        std::fs::write(path.join(format!("{start}-hop1.txt")), b"3 wait\n").expect("route");
        assert_eq!(
            assemble_chain(path, start),
            Err(AssemblyError::DuplicateOrdinal)
        );
        std::fs::remove_file(path.join(format!("{start}-hop1.txt")))
            .expect("remove competing fixture");
        let other = ohl_campaign::TRAINMAP;
        std::fs::write(path.join(format!("{other}.txt")), b"1 wait\n").expect("route");
        assert_eq!(assemble_chain(path, other).expect("other scope").len(), 1);
        for ordinal in 2..=MAX_CHAIN_ROUTES {
            std::fs::write(path.join(format!("hop-{ordinal:04}.txt")), b"1 wait\n").expect("route");
        }
        assert_eq!(
            assemble_chain(path, start).expect("cap").len(),
            MAX_CHAIN_ROUTES
        );
        std::fs::remove_file(&legacy).expect("legacy fixture");
        std::fs::write(path.join("hop-0000.txt"), b"1 wait\n").expect("neutral first");
        assert_eq!(
            assemble_chain(path, start).expect("neutral only").len(),
            MAX_CHAIN_ROUTES
        );
    }

    fn entrypoint_fixture() -> (tempfile::TempDir, Vec<String>) {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir_all(root.path().join("xtask/chain-routes")).expect("routes");
        std::fs::write(
            root.path().join("xtask/chain-routes/hop-0000.txt"),
            b"1 wait\n",
        )
        .expect("route");
        let raw: Vec<_> = [
            "--payload-root",
            POISONS[2],
            "--bin",
            POISONS[2],
            "--start-inventory",
            POISONS[1],
            "--aggregate-only",
            "--min-depth",
            "2",
        ]
        .map(String::from)
        .into();
        (root, raw)
    }

    #[test]
    fn chain_full_entrypoint_aggregate_boundary_rejects_child_failure_and_poison() {
        let (root, raw) = entrypoint_fixture();
        for (case, success) in [
            ("valid", true),
            ("nonzero", false),
            ("timeout", false),
            ("overflow", false),
        ] {
            let mut output = Vec::new();
            let exit = run_with(root.path(), &raw, &mut output, |_, _| {
                capture_stderr(
                    command(case),
                    if case == "timeout" {
                        Duration::from_secs(1)
                    } else {
                        Duration::from_secs(5)
                    },
                )
            });
            assert_eq!(exit == ExitCode::SUCCESS, success);
            let text = String::from_utf8(output).expect("utf8");
            for poison in POISONS {
                assert!(!text.contains(poison));
            }
            assert!(!text.contains(ohl_campaign::STARTMAP));
            assert!(text.contains(if success {
                "| Result | Pass |"
            } else {
                "| Result | Fail |"
            }));
        }
    }

    #[test]
    fn chain_full_entrypoint_preserves_explicit_minimum_and_typed_failure() {
        let (root, raw) = entrypoint_fixture();
        let mut continuation = raw.clone();
        let minimum = continuation
            .iter()
            .position(|arg| arg == "--min-depth")
            .expect("minimum");
        continuation[minimum + 1] = "12".to_string();
        let mut output = Vec::new();
        assert_eq!(
            run_with(root.path(), &continuation, &mut output, |_, _| {
                capture_stderr(command("valid"), Duration::from_secs(5))
            }),
            ExitCode::FAILURE
        );
        let compatibility: Vec<_> = raw
            .iter()
            .filter(|arg| *arg != "--aggregate-only")
            .cloned()
            .collect();
        let mut output = Vec::new();
        assert_eq!(
            run_with(root.path(), &compatibility, &mut output, |_, _| {
                capture_stderr(command("nonzero"), Duration::from_secs(5))
            }),
            ExitCode::FAILURE
        );
        assert!(
            String::from_utf8(output)
                .expect("utf8")
                .contains("| Result | Fail |")
        );
        for end in [
            ChildEnd::WaitFailed,
            ChildEnd::ReadFailed,
            ChildEnd::SpawnFailed,
        ] {
            let mut output = Vec::new();
            let exit = run_with(root.path(), &raw, &mut output, |_, _| {
                CapturedRun {
                end,
                stderr: "[info] The chain walk stopped.\n[info] Chain walk depth: 2.\n[info] Chain walk simulated seconds: 1.0.\n".to_string(),
            }
            });
            assert_eq!(exit, ExitCode::FAILURE);
            assert!(
                String::from_utf8(output)
                    .expect("utf8")
                    .contains(end.code())
            );
        }
        // A caller-controlled invalid start must also cross the entrypoint safely.
        let mut raw = raw;
        raw.extend(["--start".to_string(), POISONS[0].to_string()]);
        let mut output = Vec::new();
        assert_eq!(
            run_with(root.path(), &raw, &mut output, |_, _| panic!(
                "invalid start never spawns"
            )),
            ExitCode::FAILURE
        );
        for poison in POISONS {
            assert!(!String::from_utf8_lossy(&output).contains(poison));
        }
    }
}
