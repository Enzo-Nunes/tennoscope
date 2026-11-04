use super::{
    AccessPolicy, SharedRuntime, apply_outcome, overlay_window, refresh_blocking,
    reward_log::{RewardLogEvent, RewardLogMachine},
    reward_observer::{RewardObservation, RewardObserverState},
    reward_ocr::{self, ScreenRewardSource},
    reward_recognition::{RecognitionTiming, RecognizedRewards, RewardRecognition},
    reward_source::LiveMemoryRewardState,
};
// Only the Linux portal teardown names it now that the kiosk's capture source is gone.
#[cfg(target_os = "linux")]
use super::reward_capture;
use app_core::InventoryRefreshOutcome;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
#[cfg(target_os = "linux")]
use warframe_acquisition::LinuxProc as ProcessObserver;
#[cfg(windows)]
use warframe_acquisition::WindowsProc as ProcessObserver;
use warframe_acquisition::{
    AcquisitionError, CatalogCache, CatalogIndex, GameProcess, MarketPriceCache, MemoryReader,
    ProcessDiscovery, RelicCatalogCache, RelicRewardIndex, RewardCatalogEntry, RewardMemoryScanner,
    WarmOutcome, WfcdCatalogHttp, WfcdRelicCatalogHttp,
};
use warframe_domain::RewardCandidate;

const MARKER: &[u8] = b"Inventory sync done";
/// How long to wait before scanning again for an inventory authorization the game does not hold
/// yet. A scan costs a few seconds of memory reads, and logging in takes a minute or two.
const LOGIN_RETRY_SECONDS: u64 = 30;
const LOGIN_RETRY_LIMIT: u32 = 10;
const CARRY_LIMIT: usize = 64;

pub struct LogObservation {
    identity: String,
    len: u64,
    bytes: Vec<u8>,
}
impl LogObservation {
    pub fn new(identity: impl Into<String>, len: u64, bytes: Vec<u8>) -> Self {
        Self {
            identity: identity.into(),
            len,
            bytes,
        }
    }
}

pub struct MonitorInput {
    now: u64,
    discovery: Result<Option<u32>, AcquisitionError>,
    launcher_seen: bool,
    log: Result<Option<LogObservation>, AcquisitionError>,
}
impl MonitorInput {
    pub fn running(now: u64, pid: u32, log: Option<LogObservation>) -> Self {
        Self {
            now,
            discovery: Ok(Some(pid)),
            launcher_seen: false,
            log: Ok(log),
        }
    }
    pub fn absent(now: u64, launcher_seen: bool) -> Self {
        Self {
            now,
            discovery: Ok(None),
            launcher_seen,
            log: Ok(None),
        }
    }
    pub fn error(now: u64, error: AcquisitionError) -> Self {
        Self {
            now,
            discovery: Err(error),
            launcher_seen: false,
            log: Ok(None),
        }
    }
    pub fn running_with_log_error(now: u64, pid: u32) -> Self {
        Self {
            now,
            discovery: Ok(Some(pid)),
            launcher_seen: false,
            log: Err(AcquisitionError::MemoryReadFailed { pid }),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct MonitorResult {
    pub refresh: bool,
    pub acquisition_health: Option<AcquisitionError>,
    pub log_health: Option<LogMonitorDiagnostic>,
}

pub struct MonitorMachine {
    cooldown: u64,
    process: Option<u32>,
    attached_since: Option<u64>,
    last_refresh: Option<u64>,
    pending: bool,
    retry_at: Option<u64>,
    retries: u32,
    log_identity: Option<String>,
    log_offset: u64,
    carry: Vec<u8>,
}

impl MonitorMachine {
    pub fn new(cooldown_seconds: u64) -> Self {
        Self {
            cooldown: cooldown_seconds,
            process: None,
            attached_since: None,
            last_refresh: None,
            pending: false,
            retry_at: None,
            retries: 0,
            log_identity: None,
            log_offset: 0,
            carry: Vec::new(),
        }
    }

    pub const fn log_offset(&self) -> u64 {
        self.log_offset
    }
    pub const fn process_pid(&self) -> Option<u32> {
        self.process
    }
    pub fn log_identity(&self) -> Option<&str> {
        self.log_identity.as_deref()
    }
    pub fn attached_since(&self) -> Option<u64> {
        self.attached_since
    }

    /// Ask for another inventory refresh `delay` seconds from `now`, for as long as this process
    /// stays attached. The refresh that runs as the game appears lands before the player has
    /// logged in, and the next "Inventory sync done" can be a whole mission away, so a scan that
    /// found no authorization asks to be run again rather than waiting on the log. Only
    /// `LOGIN_RETRY_LIMIT` retries run per attach: past that the scanner is more likely broken by a
    /// game update than the player still logging in, and the log marker takes over again.
    pub fn retry_after(&mut self, now: u64, delay: u64) {
        if self.process.is_some() && self.retries < LOGIN_RETRY_LIMIT {
            self.retries += 1;
            self.retry_at = Some(now.saturating_add(delay));
        }
    }

    pub fn tick(&mut self, input: MonitorInput) -> MonitorResult {
        let pid = match input.discovery {
            Ok(Some(pid)) => pid,
            Ok(None) => {
                if self.process.is_some() {
                    log::info!("monitor: game process gone, resetting");
                }
                self.reset_process();
                let acquisition_health = if input.launcher_seen {
                    AcquisitionError::LauncherRunning
                } else {
                    AcquisitionError::GameNotRunning
                };
                return MonitorResult {
                    refresh: false,
                    acquisition_health: Some(acquisition_health),
                    log_health: Some(LogMonitorDiagnostic::Unavailable),
                };
            }
            Err(error) => {
                return MonitorResult {
                    refresh: false,
                    acquisition_health: Some(error),
                    log_health: Some(LogMonitorDiagnostic::Unavailable),
                };
            }
        };
        let mut event = false;
        if self.process != Some(pid) {
            self.reset_process();
            self.process = Some(pid);
            self.attached_since = Some(input.now);
            event = true;
        }
        match input.log {
            Err(error) => {
                log::warn!("monitor: EE.log read failed: {error}");
                MonitorResult {
                    refresh: self.schedule(input.now, event),
                    acquisition_health: None,
                    log_health: Some(LogMonitorDiagnostic::ReadFailed),
                }
            }
            Ok(Some(log)) => {
                event |= self.ingest(log);
                MonitorResult {
                    refresh: self.schedule(input.now, event),
                    acquisition_health: None,
                    log_health: Some(LogMonitorDiagnostic::Ready),
                }
            }
            Ok(None) => MonitorResult {
                refresh: self.schedule(input.now, event),
                acquisition_health: None,
                log_health: Some(LogMonitorDiagnostic::Unavailable),
            },
        }
    }

    fn schedule(&mut self, now: u64, event: bool) -> bool {
        if self.retry_at.is_some_and(|at| now >= at) {
            self.retry_at = None;
            self.pending = true;
        }
        self.pending |= event;
        let ready = self
            .last_refresh
            .is_none_or(|last| now.saturating_sub(last) >= self.cooldown);
        if self.pending && ready {
            self.pending = false;
            self.last_refresh = Some(now);
            true
        } else {
            false
        }
    }

    fn ingest(&mut self, log: LogObservation) -> bool {
        let rotated =
            self.log_identity.as_deref() != Some(&log.identity) || log.len < self.log_offset;
        if rotated {
            log::info!("monitor: EE.log rotated or changed identity");
            self.log_identity = Some(log.identity);
            self.log_offset = 0;
            self.carry.clear();
        }
        if self.log_offset == 0 && log.bytes.is_empty() {
            self.log_offset = log.len;
            return false;
        }
        let mut joined = std::mem::take(&mut self.carry);
        joined.extend_from_slice(&log.bytes);
        let mut triggered = false;
        let mut line_start = 0;
        for (index, byte) in joined.iter().enumerate() {
            if *byte == b'\n' {
                triggered |= joined[line_start..index]
                    .windows(MARKER.len())
                    .any(|window| window == MARKER);
                line_start = index + 1;
            }
        }
        self.carry.extend_from_slice(&joined[line_start..]);
        if self.carry.len() > CARRY_LIMIT {
            self.carry.drain(..self.carry.len() - CARRY_LIMIT);
        }
        self.log_offset = log.len;
        triggered
    }

    fn reset_process(&mut self) {
        self.process = None;
        self.attached_since = None;
        self.log_identity = None;
        self.log_offset = 0;
        self.carry.clear();
        self.pending = false;
        self.retry_at = None;
        self.retries = 0;
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogMonitorDiagnostic {
    Ready,
    Unavailable,
    ReadFailed,
}

/// Unix seconds of the session a chunk of EE.log belongs to, taken from the `[UTC: ...]` clock the
/// game writes near the top of every log. EE.log lines themselves carry only engine uptime, so
/// this line is the only place a log says when it happened.
pub fn ee_log_session_start_utc(bytes: &[u8]) -> Option<u64> {
    let start = find_subslice(bytes, b"[UTC: ")? + b"[UTC: ".len();
    let end = start + find_subslice(&bytes[start..], b"]")?;
    let clock = std::str::from_utf8(&bytes[start..end]).ok()?;
    civil_from_clock(clock)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `Sat Aug 22 02:01:50 2026` as seconds since the epoch. The weekday is parsed past rather than
/// checked: the date carries it, and a mismatch is the game's problem to lie about, not ours.
fn civil_from_clock(clock: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut fields = clock.split_whitespace();
    fields.next()?;
    let month_name = fields.next()?;
    let month = MONTHS.iter().position(|name| *name == month_name)? as u32 + 1;
    let day = fields.next()?.parse::<u32>().ok()?;
    let mut time = fields.next()?.split(':');
    let hour = time.next()?.parse::<u32>().ok()?;
    let minute = time.next()?.parse::<u32>().ok()?;
    let second = time.next()?.parse::<u32>().ok()?;
    let year = fields.next()?.parse::<i64>().ok()?;
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(
        (days_from_civil(year, month, day) * 86_400
            + hour as i64 * 3_600
            + minute as i64 * 60
            + second as i64) as u64,
    )
}

/// Days from 1970-01-01, civil algorithm (Hinnant): months are counted from March so a leap day
/// rides at the end of the year instead of splitting February.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_from_march = (month as i64 + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Byte offset of the first line at or after `floor` on the log's wall clock, or `None` when every
/// line is older. A line's wall time is the session start plus its leading uptime; a line without
/// one (the game drops it on a few shutdown lines) rides the uptime of the line above.
pub fn ee_log_stale_prefix_end(bytes: &[u8], session_start: u64, floor: u64) -> Option<usize> {
    let floor = floor as f64;
    let session_start = session_start as f64;
    let mut uptime = 0.0_f64;
    let fresh = |line: &[u8], uptime: &mut f64| -> bool {
        if let Some(parsed) = leading_uptime(line) {
            *uptime = parsed;
        }
        session_start + *uptime >= floor
    };
    let mut line_start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            if fresh(&bytes[line_start..index], &mut uptime) {
                return Some(line_start);
            }
            line_start = index + 1;
        }
    }
    if line_start < bytes.len() && fresh(&bytes[line_start..], &mut uptime) {
        return Some(line_start);
    }
    None
}

/// The engine uptime a line opens with, when it opens with one at all.
fn leading_uptime(line: &[u8]) -> Option<f64> {
    let end = line
        .iter()
        .position(|byte| *byte == b' ' || *byte == b'\t')
        .unwrap_or(line.len());
    let uptime = std::str::from_utf8(&line[..end])
        .ok()?
        .parse::<f64>()
        .ok()?;
    uptime.is_finite().then_some(uptime)
}

/// How far before attachment a line may still have happened and count as fresh. EE.log reaches
/// this process seconds after the events it describes, measured at ~7.5s on 2026-07-27, so a
/// line written just after attach can carry a timestamp from just before it.
pub const EE_LOG_ATTACH_GRACE_SECS: u64 = 60;

/// Where a from-zero read of a replacement EE.log may start, given the moment the monitor attached
/// to the game process it is reading for. `None` means the whole file predates the session and
/// none of it may be replayed.
///
/// 2026-08-22 is why this exists. The EE.log path resolution flipped between Wine prefixes a
/// second after attach, the flip reset the read offset to zero, and the morning's fissure replayed
/// as if it were live: the poller armed from eleven-hour-old relic loads, the reward pipeline ran
/// against a screen that did not exist, and health ended the day degraded for a game that was
/// never running. A file that cannot be placed in time at all is treated as stale: a missed
/// reward is quieter than a false report.
pub fn ee_log_rotation_keep_from(
    bytes: &[u8],
    file_created_unix: Option<u64>,
    attached_since: Option<u64>,
) -> Option<usize> {
    let Some(attached) = attached_since else {
        return Some(0);
    };
    let floor = attached.saturating_sub(EE_LOG_ATTACH_GRACE_SECS);
    if let Some(session_start) = ee_log_session_start_utc(bytes) {
        return ee_log_stale_prefix_end(bytes, session_start, floor);
    }
    if file_created_unix.is_some_and(|created| created >= floor) {
        return Some(0);
    }
    None
}
#[cfg(unix)]
pub(crate) fn inventory_log_path(pid: u32) -> Option<PathBuf> {
    inventory_log_path_at(Path::new("/proc"), pid)
}

/// On Windows the game writes to its own `%LOCALAPPDATA%`, so there is no prefix to discover and
/// the PID is not needed, but the signature is shared with the Wine path, which does need it.
#[cfg(windows)]
pub(crate) fn inventory_log_path(_pid: u32) -> Option<PathBuf> {
    inventory_log_under(Path::new(&std::env::var_os("LOCALAPPDATA")?))
}

/// The log under a given `%LOCALAPPDATA%`, if the game has written one.
///
/// Taking the root as an argument is what makes the layout testable against a synthetic tree; the
/// Wine path is parameterised the same way and for the same reason.
#[cfg(windows)]
pub fn inventory_log_under(local_appdata: &Path) -> Option<PathBuf> {
    let path = local_appdata.join("Warframe/EE.log");
    // `is_file` and not `exists`: an uninstall can leave the folder behind, and taking a directory
    // for the log turns every later read into a permission error instead of "the game has not run".
    path.is_file().then_some(path)
}

#[cfg(unix)]
pub fn inventory_log_path_at(proc_root: &Path, pid: u32) -> Option<PathBuf> {
    let mut prefixes = Vec::new();
    let process_root = proc_root.join(pid.to_string());
    if let Ok(environment) = fs::read(process_root.join("environ")) {
        if let Some(prefix) = environment
            .split(|byte| *byte == 0)
            .find_map(|entry| entry.strip_prefix(b"WINEPREFIX="))
            .and_then(|value| String::from_utf8(value.to_vec()).ok())
        {
            prefixes.push(PathBuf::from(prefix));
        }
    }
    for source in [
        fs::read_link(process_root.join("exe"))
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        fs::read_to_string(process_root.join("maps")).ok(),
    ]
    .into_iter()
    .flatten()
    {
        for line in source.lines() {
            let Some(path_start) = line.find('/') else {
                continue;
            };
            if let Some((prefix, _)) = line[path_start..].rsplit_once("/drive_c/") {
                prefixes.push(PathBuf::from(prefix));
            }
        }
    }
    prefixes.sort();
    prefixes.dedup();
    for prefix in prefixes {
        let users = prefix.join("drive_c/users");
        let Ok(users) = fs::read_dir(users) else {
            continue;
        };
        for user in users.flatten() {
            for relative in [
                "AppData/Local/Warframe/EE.log",
                "Local Settings/Application Data/Warframe/EE.log",
            ] {
                let path = user.path().join(relative);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn should_close_portal(previous: Option<u32>, current: Option<u32>) -> bool {
    previous.is_some() && current.is_none()
}

fn confirmed_process_observation<T: Copy, E>(
    discovered: &Result<Option<T>, E>,
) -> Option<Option<T>> {
    discovered.as_ref().ok().copied()
}

/// The responder scans that belong to one reward attempt.
///
/// A generation invalidates workers already in flight; the two indexes must turn over with that
/// generation or an old responder can block or populate a later fissure.
#[derive(Default)]
struct RecordScans {
    records: Arc<Mutex<BTreeMap<String, String>>>,
    active: Arc<Mutex<BTreeSet<String>>>,
    generation: Arc<AtomicU64>,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl RecordScans {
    fn reset(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut records) = self.records.lock() {
            records.clear();
        }
        if let Ok(mut active) = self.active.lock() {
            active.clear();
        }
        if let Ok(mut workers) = self.workers.lock() {
            for worker in workers.drain(..) {
                let _ = worker.join();
            }
        }
    }

    fn track_worker(&self, worker: std::thread::JoinHandle<()>) {
        if let Ok(mut workers) = self.workers.lock() {
            workers.push(worker);
        } else {
            let _ = worker.join();
        }
    }

    fn scan(
        &self,
        identity: String,
        process: GameProcess,
        candidates: &[warframe_acquisition::RewardNeedle],
        generation: MonitorGeneration,
    ) {
        if candidates.is_empty() {
            return;
        }
        let Ok(mut active) = self.active.lock() else {
            return;
        };
        if !active.insert(identity.clone()) {
            return;
        }
        drop(active);

        let candidates = candidates.to_vec();
        let records = Arc::clone(&self.records);
        let active = Arc::clone(&self.active);
        let mission_generation = Arc::clone(&self.generation);
        let expected_generation = mission_generation.load(Ordering::Acquire);
        if !generation.is_current() {
            release_player_record_scan(&identity, &active);
            return;
        }
        let worker = std::thread::spawn(move || {
            let started = Instant::now();
            if !generation.is_current() {
                release_player_record_scan(&identity, &active);
                return;
            }
            let procfs = ProcessObserver::new();
            let scanner = RewardMemoryScanner::new(
                256 * 1024,
                768 * 1024 * 1024,
                Duration::from_millis(1_500),
            );
            let resolution = scan_player_record_until_ready(
                expected_generation,
                &mission_generation,
                Duration::from_millis(750),
                || {
                    if !generation.is_current() {
                        return warframe_acquisition::RewardResolution::Incomplete;
                    }
                    scanner
                        .resolve_records(
                            &procfs,
                            &process,
                            &candidates,
                            warframe_acquisition::RewardRecordQuery {
                                responders: &[identity.as_str()],
                                local_identity: None,
                                local_choice: None,
                            },
                            warframe_acquisition::RewardRecordPolicy::LiveStructured,
                        )
                        .unwrap_or(warframe_acquisition::RewardResolution::Incomplete)
                },
            );
            trace_responder_reward_scan(&identity, started.elapsed(), &resolution);
            if generation.is_current() {
                store_player_record_if_current(
                    expected_generation,
                    &mission_generation,
                    &identity,
                    resolution,
                    &records,
                );
            }
            release_player_record_scan(&identity, &active);
        });
        self.track_worker(worker);
    }
}

/// What one observed log event may do: the effective access policy, the monitor generation
/// it belongs to, and the second it was observed in. Bundled so handler signatures stay small.
#[derive(Clone, Copy)]
struct EventScope<'a> {
    policy: AccessPolicy,
    generation: &'a MonitorGeneration,
}

/// Everything whose lifetime is one monitored Warframe process's reward stream.
///
/// The monitor supplies events and application side effects; this session owns the coupled
/// observation, responder-scan and recognition state that must turn over together.
/// Recognition owns the catalog-derived relic pool, background OCR, acceptance and
/// stale-result retirement. The monitor never captures synchronously.
struct RewardSession {
    recognition: RewardRecognition<ScreenRewardSource, fn() -> ScreenRewardSource>,
    memory: LiveMemoryRewardState,
    observer: RewardObserverState,
    scans: RecordScans,
    price_cache: MarketPriceCache,
    /// Consecutive background failures and their reason, so routine pre-screen blanks do not
    /// mark health degraded every poll while broken capture still reports immediately.
    degraded_reason: Option<String>,
    degraded_streak: u32,
}

impl RewardSession {
    fn new(
        catalog: Option<CatalogIndex>,
        relics: Option<RelicRewardIndex>,
        rewards: Vec<RewardCatalogEntry>,
        price_cache: MarketPriceCache,
    ) -> Self {
        Self {
            recognition: RewardRecognition::new(
                catalog,
                relics,
                rewards,
                RecognitionTiming::live(),
                ScreenRewardSource::new,
            ),
            memory: LiveMemoryRewardState::new(RewardMemoryScanner::new(
                256 * 1024,
                768 * 1024 * 1024,
                Duration::from_millis(1_500),
            )),
            observer: RewardObserverState::new(1, 1),
            scans: RecordScans::default(),
            price_cache,
            degraded_reason: None,
            degraded_streak: 0,
        }
    }

    fn handle_event(
        &mut self,
        event: RewardLogEvent,
        process: Option<GameProcess>,
        procfs: &ProcessObserver,
        shared: &SharedRuntime,
        app: &AppHandle,
        scope: EventScope<'_>,
    ) {
        match event {
            RewardLogEvent::RewardWindowOpened => {
                if scope.policy.read_process_memory
                    && let Some(process) = process
                {
                    let _ = procfs.reset_recent_writes(&process);
                }
                // The open line is what lets the reader start capturing; a vanished game has
                // no screen to read.
                if process.is_some() {
                    self.recognition.observe(&event);
                }
            }
            RewardLogEvent::ResponderExpected { identity } => {
                self.scan_responder(identity, process, scope.policy, scope.generation);
            }
            RewardLogEvent::ResponderReceived { identity, is_local } => {
                if !is_local {
                    self.scan_responder(identity, process, scope.policy, scope.generation);
                }
            }
            RewardLogEvent::ResponsesComplete { .. } => {
                // Constraints for the background reader; never captures here.
                // A vanished game has no window to read, so skip stale rosters.
                if process.is_some() {
                    self.recognition.observe(&event);
                }
            }
            RewardLogEvent::BaselineRequested { .. } => {
                self.scans.reset();
                let Some(_process) = process else {
                    self.memory.clear();
                    self.recognition.close();
                    return;
                };
                self.recognition.observe(&event);
                // Keep the dormant responder-scan candidates in step with recognition.
                // Recognition owns the catalog derivation; memory only mirrors it.
                let candidates = self.recognition.candidates();
                if scope.policy.read_process_memory {
                    self.memory.prepare_candidates(&candidates);
                } else {
                    self.memory.clear();
                }
                // Price the pool while the mission is still running; the reward screen is
                // minutes away. Warming never blocks event handling.
                let entries = self.recognition.pool_entries();
                spawn_market_price_warm(&entries, &self.price_cache);
            }
            RewardLogEvent::ChoicesReady { .. } => {
                if process.is_none() {
                    return;
                }
                self.recognition.observe(&event);
            }
            RewardLogEvent::Closed => self.close(shared, app),
        }
    }

    fn scan_responder(
        &self,
        identity: String,
        process: Option<GameProcess>,
        policy: AccessPolicy,
        generation: &MonitorGeneration,
    ) {
        if self.recognition.resolved() || !policy.read_process_memory {
            return;
        }
        let Some(process) = process else {
            return;
        };
        self.scans.scan(
            identity,
            process,
            &self.recognition.candidates(),
            generation.clone(),
        );
    }

    fn publish(
        &mut self,
        recognized: RecognizedRewards,
        shared: &SharedRuntime,
        app: &AppHandle,
        now: u64,
        generation: &MonitorGeneration,
    ) {
        let publication = recognized.publication.clone();
        let cards = recognized.cards;
        let elapsed = recognized.elapsed;
        // Only the first publication for this epoch runs; late duplicates and stale
        // delayed effects are declined by the same gate.
        publication.publish(|| {
            let transition = self.observer.observe(cards);
            let mut overlay_notice = None;
            if transition.publish {
                apply_reward_observations(
                    shared,
                    self.recognition.rewards(),
                    &transition.choices,
                    &BTreeMap::new(),
                );
                overlay_notice = overlay_window::show_reward_overlay(app, transition.choices.len());
                let _ = app.emit_to("reward-overlay", "reward-updated", ());
                spawn_market_price_fetch(
                    &transition.choices,
                    shared,
                    app,
                    self.recognition.rewards(),
                    &self.price_cache,
                    now,
                    generation,
                );
            }
            if let Ok(mut runtime) = shared.lock() {
                let _ = runtime.core.record_capture_source_ready(
                    "ocr",
                    elapsed.as_millis(),
                    now.to_string(),
                );
                // Read the cards but could not find the window to draw over: on Windows that is
                // exclusive fullscreen, and the player is the only one who can fix it.
                if let Some(notice) = overlay_notice {
                    let _ = runtime.core.record_capture_degraded(notice);
                }
            }
        });
    }

    fn drain_recognition(
        &mut self,
        shared: &SharedRuntime,
        app: &AppHandle,
        now: u64,
        generation: &MonitorGeneration,
    ) {
        // The monitor never captures here; it only drains what the background reader found.
        // A slow capture therefore cannot delay overlay hide, game-gone or the next event.
        let update = self.recognition.poll();
        let had_recognized = update.recognized.is_some();
        if let Some(recognized) = update.recognized {
            self.publish(recognized, shared, app, now, generation);
        }
        if let Some(failure) = update.failure {
            let (reason, streak) = if self.degraded_reason.as_deref() == Some(&failure.reason) {
                self.degraded_streak = self.degraded_streak.saturating_add(1);
                (
                    self.degraded_reason.clone().unwrap_or_default(),
                    self.degraded_streak,
                )
            } else {
                self.degraded_reason = Some(failure.reason.clone());
                self.degraded_streak = 1;
                (failure.reason.clone(), 1)
            };
            // Routine pre-screen blanks only matter after a sustained streak; broken capture
            // reports immediately. Without this gate a baseline set minutes before the screen
            // would mark health degraded every poll while nothing is expected on screen.
            if crate::reward_recognition::poll_failure_is_worth_warning(&reason, streak)
                && let Ok(mut runtime) = shared.lock()
            {
                let _ = runtime
                    .core
                    .record_capture_degraded(format!("Screen capture failed: {reason}"));
            }
        } else if had_recognized || update.hide {
            self.degraded_reason = None;
            self.degraded_streak = 0;
        }
        // The capture signal beats EE.log's delayed shutdown line and prevents a stale overlay.
        if update.hide && self.observer.miss().hide {
            overlay_window::hide_reward_overlay(app);
        }
    }

    fn game_gone(&mut self, hide: &dyn Fn()) {
        // Suspend without clearing the pool: a transient process-absent tick must not blind the
        // fissure, since relic baselines are one-shot log lines with no recovery path. The worker
        // drops its source and pauses; recognition resumes against the preserved pool when the
        // process returns. Full clearing is reserved for the log's Closed event.
        self.recognition.suspend();
        self.memory.clear();
        self.scans.reset();
        if self.observer.miss().hide {
            hide();
        }
    }

    fn resume(&mut self) {
        self.recognition.resume();
    }

    fn close(&mut self, shared: &SharedRuntime, app: &AppHandle) {
        self.recognition.close();
        self.scans.reset();
        self.memory.clear();
        self.observer.miss();
        // A new screen starts a fresh degraded-health streak; otherwise the next fissure's
        // first routine blank could hit the warning threshold on the previous screen's count.
        self.degraded_reason = None;
        self.degraded_streak = 0;
        overlay_window::hide_reward_overlay(app);
        if let Ok(mut runtime) = shared.lock() {
            let _ = runtime.core.apply_reward_candidates(Vec::new());
        }
    }
}

pub(crate) fn run(
    shared: SharedRuntime,
    app: AppHandle,
    policy: AccessPolicy,
    generation: MonitorGeneration,
) {
    debug_assert!(policy.observe_process_presence);
    let procfs = ProcessObserver::new();
    let mut machine = MonitorMachine::new(15);
    let login_retry_due = Arc::new(AtomicBool::new(false));
    let mut reward_log = RewardLogMachine::default();
    let mut announced_process = None;
    let mut tracked_resolution: Option<(u32, Option<PathBuf>)> = None;
    // Survives across missions on purpose: the same relic pools recur all evening, so a price
    // fetched two runs ago is one this run does not have to make.
    let price_cache = shared
        .lock()
        .map(|runtime| runtime.live_prices.clone())
        .unwrap_or_default();
    let catalog = shared
        .lock()
        .ok()
        .and_then(|runtime| load_catalog(&runtime.app_data));
    if let (Some(catalog), Ok(mut runtime)) = (catalog.as_ref(), shared.lock()) {
        // Before enrichment, so the first view it publishes already carries ducat values.
        runtime
            .core
            .set_collection_ducats(Arc::new(catalog.ducat_table()));
        // A clone of the index reward recognition keeps, so a mark and a price never answer
        // from two parses of the catalogue.
        runtime.core.set_mastery_catalog(Arc::new(catalog.clone()));
        let _ = runtime.core.enrich_collection_from_catalog(catalog);
    }
    let reward_catalog = catalog
        .as_ref()
        .map(CatalogIndex::reward_entries)
        .unwrap_or_default();
    // The path is copied out first: the load can fetch over the network, and the runtime lock
    // must not be held for that.
    let relic_catalog = shared
        .lock()
        .ok()
        .map(|runtime| runtime.app_data.clone())
        .and_then(|app_data| load_relic_catalog(&app_data));
    let mut reward_session =
        RewardSession::new(catalog, relic_catalog, reward_catalog, price_cache.clone());
    // EE.log reaches us seconds after the events it describes (measured at ~7.5s on 2026-07-27),
    // by which time the fifteen-second reward screen can already be gone. The relic-load signal
    // arrives minutes ahead of the screen though, so it can arm a poller that watches for the cards
    // directly. The closed-set match is its own detector: only the reward screen yields four names
    // from this squad's relic pool.

    while generation.is_current() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let discovered = procfs.discover();
        let confirmed_process = confirmed_process_observation(&discovered);
        #[cfg(target_os = "linux")]
        let mut close_portal = false;
        if let Some(process) = confirmed_process {
            if process != announced_process {
                #[cfg(target_os = "linux")]
                {
                    close_portal = should_close_portal(
                        announced_process.map(|process: GameProcess| process.pid()),
                        process.map(|process| process.pid()),
                    );
                }
                if let Ok(mut runtime) = shared.lock() {
                    runtime.game_running = process.is_some();
                    if process.is_some() {
                        let _ = runtime.core.record_game_process_ready();
                    }
                }
                announced_process = process;
            }
        }
        // A discovery error is not evidence that Warframe exited. Keep the last confirmed process
        // for reward teardown and event handling until procfs reports an actual absence.
        let process = confirmed_process.unwrap_or(announced_process);
        let mut adopted_reward_events = Vec::new();
        let (input, log_bytes) = match discovered {
            Ok(None) => (
                MonitorInput::absent(now, procfs.launcher_present()),
                Vec::new(),
            ),
            Err(error) => (MonitorInput::error(now, error), Vec::new()),
            Ok(Some(process)) => {
                let path = inventory_log_path(process.pid());
                if monitor_path_changed(tracked_resolution.as_ref(), process.pid(), path.as_deref())
                {
                    log::debug!(
                        "monitor: EE.log path resolution pid={} found={}",
                        process.pid(),
                        path.is_some()
                    );
                    tracked_resolution = Some((process.pid(), path.clone()));
                    if let Some(ref ee_path) = path {
                        if let Ok(mut runtime) = shared.lock() {
                            runtime.last_ee_log_path = Some(ee_path.clone());
                        }
                        // The live tail starts at EOF, and in an endless mission the next
                        // round's relics load minutes before its reward screen opens, so an app
                        // started in between must recover them from the tail or it never captures.
                        adopted_reward_events =
                            reward_log.adopt_tail(&log_tail(ee_path, REWARD_TAIL_SCAN));
                        if let Some(relic_paths) =
                            adopted_reward_events.iter().find_map(|event| match event {
                                RewardLogEvent::BaselineRequested { relic_paths } => {
                                    Some(relic_paths)
                                }
                                _ => None,
                            })
                        {
                            log::info!(
                                "reward: adopted {} relics from the EE.log tail",
                                relic_paths.len()
                            );
                        }
                    }
                }
                build_monitor_input(&machine, now, process.pid(), path)
            }
        };
        let result = machine.tick(input);
        if login_retry_due.swap(false, Ordering::AcqRel) {
            machine.retry_after(now, LOGIN_RETRY_SECONDS);
        }
        if result.refresh && policy.acquire_inventory {
            let refresh = Arc::clone(&shared);
            let refresh_generation = generation.clone();
            let retry_due = Arc::clone(&login_retry_due);
            spawn_monitor_refresh_task(move || {
                if refresh_generation.is_current()
                    && refresh_blocking(&refresh).is_ok_and(|view| view.health().awaiting_login())
                {
                    retry_due.store(true, Ordering::Release);
                }
            });
        }
        if policy.acquire_inventory
            && let Some(error) = result.acquisition_health
            && generation.is_current()
        {
            let _ = apply_outcome(
                &shared,
                InventoryRefreshOutcome::acquisition_failed(
                    warframe_acquisition::AcquisitionFailure::from_error(error),
                ),
            );
        }
        if let Some(log_health) = result.log_health {
            if let Ok(mut runtime) = shared.lock() {
                let _ = match log_health {
                    LogMonitorDiagnostic::Ready => runtime.core.record_log_monitor_ready(),
                    LogMonitorDiagnostic::Unavailable => {
                        runtime.core.record_log_monitor_idle("Waiting for Warframe")
                    }
                    LogMonitorDiagnostic::ReadFailed => runtime
                        .core
                        .record_log_monitor_failure("EE.log could not be read"),
                };
            }
        }
        for event in adopted_reward_events
            .into_iter()
            .chain(reward_log.observe_bytes(&log_bytes))
        {
            if generation.is_current() {
                reward_session.handle_event(
                    event,
                    process,
                    &procfs,
                    &shared,
                    &app,
                    EventScope {
                        policy,
                        generation: &generation,
                    },
                );
            }
        }
        reward_session.drain_recognition(&shared, &app, now, &generation);
        if process.is_none() {
            reward_session.game_gone(&|| overlay_window::hide_reward_overlay(&app));
            reward_ocr::clear_latest_matched_rect();
            #[cfg(target_os = "linux")]
            if close_portal && let Err(reason) = reward_capture::portal::PortalCapture::close() {
                log::warn!(
                    "[DEBUG-capture] could not close the screencast session ({reason}); \
                     the screen-sharing indicator may stay lit until this process exits"
                );
            }
        } else {
            // A returning process resumes recognition against the preserved pool; suspend
            // made the absence idempotent, so this is a no-op while already active.
            reward_session.resume();
        }
        let poll_interval = if reward_log.reward_window_open() {
            Duration::from_millis(10)
        } else {
            Duration::from_millis(100)
        };
        std::thread::park_timeout(poll_interval);
    }
    reward_session.close(&shared, &app);
    reward_ocr::clear_latest_matched_rect();
    #[cfg(target_os = "linux")]
    let _ = reward_capture::portal::PortalCapture::close();
    if let Ok(mut runtime) = shared.lock() {
        runtime.game_running = false;
    }
}

pub fn spawn_monitor_refresh_task(
    task: impl FnOnce() + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(task)
}

fn load_catalog(app_data: &Path) -> Option<CatalogIndex> {
    let cache = CatalogCache::new(app_data.join("catalog"));
    if let Ok(catalog) = cache.load_cached() {
        return Some(catalog.index().clone());
    }
    let source = WfcdCatalogHttp::new().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    cache
        .load(&source, now)
        .ok()
        .map(|catalog| catalog.index().clone())
}

fn load_relic_catalog(app_data: &Path) -> Option<RelicRewardIndex> {
    let cache = RelicCatalogCache::new(app_data.join("catalog"));
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let Ok(source) = WfcdRelicCatalogHttp::new() else {
        return cache
            .load_cached()
            .ok()
            .map(|catalog| catalog.index().clone());
    };
    cache
        .load_fresh(&source, now)
        .ok()
        .map(|catalog| catalog.index().clone())
}

pub fn release_player_record_scan(identity: &str, active_scans: &Mutex<BTreeSet<String>>) {
    if let Ok(mut active) = active_scans.lock() {
        active.remove(identity);
    }
}

pub fn rotate_choices_to_local(choices: &mut [String], local_name: &str) {
    if let Some(index) = choices.iter().position(|name| name == local_name) {
        choices.rotate_left(index);
    }
}

pub fn assemble_player_record_choices(
    responders: &[&str],
    local_identity: Option<&str>,
    local_choice: Option<&str>,
    records: &std::collections::BTreeMap<String, String>,
) -> Option<Vec<String>> {
    let local_identity = local_identity?;
    let mut choices = vec![local_choice?.to_owned()];
    for identity in responders
        .iter()
        .copied()
        .filter(|identity| *identity != local_identity)
    {
        choices.push(records.get(identity)?.clone());
    }
    (choices.len() == responders.len()).then_some(choices)
}

pub fn store_player_record_if_current(
    expected_generation: u64,
    generation: &AtomicU64,
    identity: &str,
    resolution: warframe_acquisition::RewardResolution,
    records: &Mutex<BTreeMap<String, String>>,
) {
    if generation.load(Ordering::Acquire) != expected_generation {
        return;
    }
    let warframe_acquisition::RewardResolution::Confirmed { choices, .. } = resolution else {
        return;
    };
    let [choice] = choices.as_slice() else {
        return;
    };
    if let Ok(mut records) = records.lock()
        && generation.load(Ordering::Acquire) == expected_generation
    {
        records.insert(identity.to_owned(), choice.clone());
    }
}

pub fn scan_player_record_until_ready(
    expected_generation: u64,
    generation: &AtomicU64,
    timeout: Duration,
    mut scan: impl FnMut() -> warframe_acquisition::RewardResolution,
) -> warframe_acquisition::RewardResolution {
    let started = Instant::now();
    while generation.load(Ordering::Acquire) == expected_generation {
        let resolution = scan();
        if matches!(
            &resolution,
            warframe_acquisition::RewardResolution::Confirmed { choices, .. }
                if choices.len() == 1
        ) {
            return resolution;
        }
        if started.elapsed() >= timeout {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    warframe_acquisition::RewardResolution::Incomplete
}

fn trace_responder_reward_scan(
    identity: &str,
    elapsed: Duration,
    resolution: &warframe_acquisition::RewardResolution,
) {
    let suffix = identity
        .get(identity.len().saturating_sub(6)..)
        .unwrap_or(identity);
    log::debug!(
        "[DEBUG-responder] identity=…{suffix} elapsed_ms={} resolution={resolution:?}",
        elapsed.as_millis(),
    );
}

/// Fetch platinum prices without blocking the overlay.
///
/// Ducats cannot rank relic rewards on their own, since most commons share a value; platinum is
/// what separates them. But the cards matter more than their prices, and the reward screen only
/// lives for fifteen seconds, so the overlay goes up first and the prices land when they land. The
/// cards render an em dash until then.
// Seven thread inputs (choices, runtime, window, catalog, cache, clock and generation);
// bundling them would hide that shape behind a struct built once.
#[allow(clippy::too_many_arguments)]
fn spawn_market_price_fetch(
    choices: &[RewardObservation],
    shared: &SharedRuntime,
    app: &AppHandle,
    reward_catalog: &[RewardCatalogEntry],
    price_cache: &MarketPriceCache,
    now: u64,
    generation: &MonitorGeneration,
) {
    let names = choices.to_vec();
    let price_cache = price_cache.clone();
    let app = app.clone();
    spawn_market_price_worker(
        names,
        Arc::clone(shared),
        reward_catalog.to_vec(),
        now,
        generation.clone(),
        move |choices| {
            // Anything the pool warmed while the mission was still running is already here, so the
            // common case does no requests at all and the overlay never shows a dash. Only a reward
            // the warm pass missed (a pool that never loaded, an API that was down then) is
            // fetched now, and it is fetched with no gap because the screen is already up.
            let mut prices = choices
                .iter()
                .filter_map(|choice| Some((choice.name.clone(), price_cache.get(&choice.name)?)))
                .collect::<BTreeMap<_, _>>();
            let missing = choices
                .iter()
                .filter(|choice| !prices.contains_key(&choice.name))
                .map(|choice| choice.name.clone())
                .collect::<Vec<_>>();
            let mut outcome = WarmOutcome::default();
            if !missing.is_empty()
                && let Some(market) = warframe_acquisition::WarframeMarketHttp::new()
            {
                outcome = price_cache.warm(&market, &missing, Duration::ZERO);
                for name in missing {
                    if let Some(price) = price_cache.get(&name) {
                        prices.insert(name, price);
                    }
                }
            }
            // An oversize response is worth saying even when the cache carried the screen, because
            // it stops every future price and nothing else would report it. An empty screen with no
            // failure to name means no request was made at all.
            let failure = outcome.failure().or_else(|| {
                prices
                    .is_empty()
                    .then_some("warframe.market pricing is unavailable for these rewards")
            });
            (prices, failure)
        },
        move || {
            let _ = app.emit_to("reward-overlay", "reward-updated", ());
        },
    );
}

// Seven inputs plus the fetch/emit closures; see the fetch above.
#[allow(clippy::too_many_arguments)]
fn spawn_market_price_worker<Fetch, Emit>(
    choices: Vec<RewardObservation>,
    shared: SharedRuntime,
    reward_catalog: Vec<RewardCatalogEntry>,
    now: u64,
    generation: MonitorGeneration,
    fetch: Fetch,
    emit: Emit,
) -> std::thread::JoinHandle<()>
where
    Fetch: FnOnce(&[RewardObservation]) -> (BTreeMap<String, u32>, Option<&'static str>)
        + Send
        + 'static,
    Emit: FnOnce() + Send + 'static,
{
    std::thread::spawn(move || {
        let (prices, failure) = fetch(&choices);
        // Delayed prices are gated by the monitor generation alone, as before: the initial
        // overlay publication already consumed the single-use epoch token, so reusing it here
        // would decline every delayed price update.
        generation.publish(|| {
            if let Some(failure) = failure
                && let Ok(mut runtime) = shared.lock()
            {
                let _ = runtime.core.record_market_degraded(failure);
            }
            if prices.is_empty() {
                return;
            }
            apply_reward_observations(&shared, &reward_catalog, &choices, &prices);
            if let Ok(mut runtime) = shared.lock() {
                let _ = runtime
                    .core
                    .record_market_ready(prices.len(), now.to_string());
            }
            emit();
        });
    })
}

/// Price the whole relic pool while the mission is still being played.
///
/// The pool is known when the relics load and the reward screen is minutes away, so there is time
/// to be unhurried and polite about it. Doing this later, when the cards are actually on screen,
/// is what made every card show a dash for the first seconds of a fifteen-second window.
fn spawn_market_price_warm(pool: &[RewardCatalogEntry], price_cache: &MarketPriceCache) {
    let names = pool
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    if names.is_empty() {
        return;
    }
    let price_cache = price_cache.clone();
    std::thread::spawn(move || {
        if let Some(market) = warframe_acquisition::WarframeMarketHttp::new() {
            price_cache.warm(&market, &names, warframe_acquisition::MARKET_MIN_GAP);
        }
    });
}

fn apply_reward_observations(
    shared: &SharedRuntime,
    catalog: &[RewardCatalogEntry],
    observations: &[RewardObservation],
    prices: &BTreeMap<String, u32>,
) {
    let Ok(mut runtime) = shared.lock() else {
        return;
    };
    let Ok(view) = runtime.core.current_view() else {
        return;
    };
    // One snapshot per publish, so two cards of one item cannot disagree. Taken only while marks
    // are on; a failed lookup costs the cards nothing rather than leaving one half-joined.
    let marks = runtime
        .preferences
        .mastery_marks
        .then(|| runtime.core.mastery_view().ok())
        .flatten();
    let candidates = observations
        .iter()
        .filter_map(|observation| {
            let ducats = catalog
                .iter()
                .find(|entry| {
                    warframe_acquisition::reward_name_matches(&entry.name, &observation.name)
                })
                .map_or(0, |entry| entry.ducats);
            let owned = view
                .collection()
                .items()
                .iter()
                .find(|item| {
                    warframe_acquisition::holding_matches_reward(item.name(), &observation.name)
                })
                .map_or(0, |item| item.quantity());
            RewardCandidate::new(
                &observation.name,
                prices.get(&observation.name).copied().unwrap_or(0),
                ducats,
                owned,
                marks
                    .as_ref()
                    .and_then(|marks| marks.reward_mark(&observation.name)),
                observation.confidence,
            )
            .ok()
        })
        .collect();
    let _ = runtime.core.apply_reward_candidates(candidates);
}

/// A string that stays the same while the log grows and changes when the log is replaced.
///
/// The monitor resumes at a byte offset, so it has to be able to tell "the same file, longer" from
/// "a new file that happens to be at the same path": getting that wrong either re-reads the whole
/// log or silently skips the start of a new one.
///
/// This was `dev:ino`, which is exactly the right answer and does not exist on Windows. Creation
/// time is the portable stand-in: the game rotates `EE.log` by writing a new file, which gets a new
/// creation time, while appending to the open one does not. Where the platform has no creation time
/// the path alone still distinguishes logs; only rotation-in-place goes unnoticed, and the length
/// check the caller already does catches the truncation that comes with it.
///
/// Seconds, not the full precision the platform offers: under Wine the reported creation time
/// jitters by a few hundred microseconds between reads of the same unmodified file, which would
/// make every poll look like a rotation and re-read the log from zero. A rotation and the append
/// before it cannot share a second and also matter, since the replacement log starts empty, so the
/// length check catches it either way.
pub fn log_identity(path: &Path, metadata: &fs::Metadata) -> String {
    let created = metadata
        .created()
        .ok()
        .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs());
    match created {
        Some(created) => format!("{}:{created}", path.display()),
        None => path.display().to_string(),
    }
}

/// Whether the log line about the followed EE.log path differs from the last one emitted.
///
/// The resolution debug line exists to explain which game the monitor is following (Wine prefix
/// surprises are the usual cause for confusion), so it must print when that state changes
/// (pid found, pid lost, another pid, another path) and stay silent while the same path is
/// being polled at up to ten times a second.
fn monitor_path_changed(
    tracked: Option<&(u32, Option<PathBuf>)>,
    pid: u32,
    path: Option<&Path>,
) -> bool {
    match (tracked, path) {
        (None, _) => true,
        (Some((tracked_pid, _)), None) => *tracked_pid != pid,
        (Some((tracked_pid, Some(tracked))), Some(path)) => {
            *tracked_pid != pid || tracked.as_path() != path
        }
        (Some(_), Some(_)) => false,
    }
}

/// How much of EE.log to fold when recovering a relic round already under way at attach. The
/// relics load right after the previous screen shuts down and the next one opens minutes later
/// (about five in a Void Cascade, 1.4 MB of log on 2026-10-29), so this spans a full round of
/// mission chatter.
const REWARD_TAIL_SCAN: u64 = 2 * 1024 * 1024;

/// The last stretch of a file, for one-shot state recovery. Any failure reads as "no tail":
/// the caller simply learns nothing, which is where it started.
fn log_tail(path: &Path, limit: u64) -> Vec<u8> {
    let Ok(metadata) = fs::metadata(path) else {
        return Vec::new();
    };
    let len = metadata.len();
    let start = len.saturating_sub(limit);
    let Ok(mut file) = fs::File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::with_capacity((len - start) as usize);
    if file.take(len - start).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    bytes
}

pub fn build_monitor_input(
    machine: &MonitorMachine,
    now: u64,
    pid: u32,
    path: Option<PathBuf>,
) -> (MonitorInput, Vec<u8>) {
    let Some(path) = path else {
        return (MonitorInput::running(now, pid, None), Vec::new());
    };
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!("monitor: EE.log open failed: {error}");
            return (MonitorInput::running_with_log_error(now, pid), Vec::new());
        }
    };
    let identity = log_identity(&path, &metadata);
    if machine.process_pid() != Some(pid) {
        return (
            MonitorInput::running(
                now,
                pid,
                Some(LogObservation::new(identity, metadata.len(), Vec::new())),
            ),
            Vec::new(),
        );
    }
    let offset = if machine.log_identity() == Some(identity.as_str())
        && metadata.len() >= machine.log_offset()
    {
        machine.log_offset()
    } else {
        0
    };
    if metadata.len() == offset {
        return (
            MonitorInput::running(
                now,
                pid,
                Some(LogObservation::new(identity, offset, Vec::new())),
            ),
            Vec::new(),
        );
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!("monitor: EE.log open failed: {error}");
            return (MonitorInput::running_with_log_error(now, pid), Vec::new());
        }
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return (MonitorInput::running_with_log_error(now, pid), Vec::new());
    }
    let requested = (metadata.len() - offset).min(1024 * 1024);
    let mut bytes = Vec::with_capacity(requested as usize);
    if file.take(requested).read_to_end(&mut bytes).is_err() {
        return (MonitorInput::running_with_log_error(now, pid), Vec::new());
    }
    // A read from zero means the log changed identity under the same process: a rotation, or the
    // path resolution settling on a different Wine prefix's EE.log. Everything from before this
    // process was attached is not this session's events, and replaying it as if it were is the
    // whole of the 2026-08-22 ghost report: an hours-old fissure armed the poller, ran the
    // reward pipeline against a screen that was not there, and left health degraded for a game
    // that was never running.
    let mut observation_len = offset + bytes.len() as u64;
    if offset == 0 {
        let created_unix = metadata
            .created()
            .ok()
            .and_then(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_secs());
        match ee_log_rotation_keep_from(&bytes, created_unix, machine.attached_since()) {
            Some(0) => {}
            Some(keep) => {
                log::warn!(
                    "monitor: EE.log changed identity under the game process; skipping {} bytes \
                     that predate this session",
                    keep
                );
                bytes.drain(..keep);
            }
            None => {
                log::warn!(
                    "monitor: EE.log changed identity to a log from an earlier session; skipping \
                     all {} bytes",
                    bytes.len()
                );
                bytes.clear();
                // Past this read, not just up to it: a stale file larger than the read cap would
                // otherwise hand its remainder over one incremental chunk at a time.
                observation_len = metadata.len();
            }
        }
    }
    let log_bytes = bytes.clone();
    (
        MonitorInput::running(
            now,
            pid,
            Some(LogObservation::new(identity, observation_len, bytes)),
        ),
        log_bytes,
    )
}

/// A generation token captured by every worker and publication owned by one monitor run.
#[derive(Clone, Debug)]
pub struct MonitorGeneration {
    id: u64,
    current: Arc<AtomicU64>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    publication: Arc<Mutex<()>>,
}

impl MonitorGeneration {
    pub fn new(id: u64, current: Arc<AtomicU64>) -> Self {
        current.store(id, Ordering::Release);
        Self {
            id,
            current,
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            publication: Arc::new(Mutex::new(())),
        }
    }

    pub const fn id(&self) -> u64 {
        self.id
    }

    pub fn request_stop(&self) {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.stop.store(true, Ordering::Release);
    }

    pub fn is_current(&self) -> bool {
        !self.stop.load(Ordering::Acquire) && self.current.load(Ordering::Acquire) == self.id
    }

    pub fn publish(&self, publication: impl FnOnce()) -> bool {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !self.is_current() {
            return false;
        }
        publication();
        true
    }
}

/// Minimal lifecycle body used by contract tests without constructing a Tauri application.
pub fn spawn_generation_worker(
    generation: MonitorGeneration,
    started: std::sync::mpsc::Sender<u64>,
    retired: std::sync::mpsc::Sender<u64>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = started.send(generation.id());
        while generation.is_current() {
            std::thread::park_timeout(Duration::from_millis(10));
        }
        let _ = retired.send(generation.id());
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_store::SnapshotMeta;
    use warframe_acquisition::MasteryFacts;
    use warframe_domain::{
        CatalogItem, Category, InventoryEntry, InventorySnapshot, ItemId, MasteryMark,
    };

    #[cfg(target_os = "linux")]
    #[test]
    fn portal_close_is_requested_once_when_the_game_exits() {
        assert!(should_close_portal(Some(42), None));
        assert!(!should_close_portal(None, None));
        assert!(!should_close_portal(None, Some(42)));
        assert!(!should_close_portal(Some(42), Some(42)));
        assert!(!should_close_portal(Some(42), Some(43)));
    }

    #[test]
    fn monitor_path_changed_fires_on_the_first_observation() {
        assert!(monitor_path_changed(None, 42, None));
    }

    #[test]
    fn monitor_path_changed_is_silent_while_the_same_path_is_tracked() {
        let path = Path::new("/prefix/drive_c/EE.log");
        let tracked = Some((42, Some(path.to_path_buf())));
        assert!(!monitor_path_changed(tracked.as_ref(), 42, Some(path)));
    }

    #[test]
    fn monitor_path_changed_silent_when_the_log_disappears() {
        let path = Some((42u32, Some(PathBuf::from("/p/EE.log"))));
        assert!(!monitor_path_changed(path.as_ref(), 42, None));
    }

    #[test]
    fn monitor_path_changed_fires_when_the_pid_changes() {
        let tracked = Some((42u32, Some(PathBuf::from("/p/EE.log"))));
        assert!(monitor_path_changed(
            tracked.as_ref(),
            43,
            Some(Path::new("/p/EE.log"))
        ));
    }

    #[test]
    fn monitor_path_changed_fires_when_the_path_moves() {
        let tracked = Some((42u32, Some(PathBuf::from("/p/EE.log"))));
        assert!(monitor_path_changed(
            tracked.as_ref(),
            42,
            Some(Path::new("/q/EE.log"))
        ));
    }
    #[test]
    fn resetting_record_scans_waits_for_memory_workers_to_retire() {
        let scans = Arc::new(RecordScans::default());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).expect("report worker start");
            release_rx.recv().expect("release memory worker");
        });
        scans.track_worker(worker);
        started_rx.recv().expect("memory worker starts");

        let retiring = Arc::clone(&scans);
        let (retired_tx, retired_rx) = std::sync::mpsc::channel();
        let resetter = std::thread::spawn(move || {
            retiring.reset();
            retired_tx.send(()).expect("report retirement");
        });
        assert!(
            retired_rx.recv_timeout(Duration::from_millis(20)).is_err(),
            "reset returned while a process-memory worker was still active"
        );

        release_tx.send(()).expect("release worker");
        retired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reset returns after memory worker retires");
        resetter.join().expect("resetter exits");
    }
    #[test]

    fn resetting_record_scans_invalidates_workers_and_clears_both_indexes() {
        let scans = RecordScans::default();
        scans.generation.store(41, Ordering::Release);
        scans
            .records
            .lock()
            .expect("records")
            .insert("squadmate".to_owned(), "Forma Blueprint".to_owned());
        scans
            .active
            .lock()
            .expect("active scans")
            .insert("squadmate".to_owned());

        scans.reset();

        assert_eq!(scans.generation.load(Ordering::Acquire), 42);
        assert!(scans.records.lock().expect("records").is_empty());
        assert!(scans.active.lock().expect("active scans").is_empty());
    }

    /// Delayed market prices are gated by the monitor generation alone: a retired generation
    /// must not mutate reward candidates, market health, or emit updates.
    #[test]
    fn retired_generation_rejects_detached_price_worker_effects() {
        let directory = tempfile::tempdir().expect("temporary runtime");
        let shared = crate::tests::test_runtime(directory.path());
        let current = Arc::new(AtomicU64::new(0));
        let generation = MonitorGeneration::new(16, current);
        let (fetch_started_tx, fetch_started_rx) = std::sync::mpsc::channel();
        let (release_fetch_tx, release_fetch_rx) = std::sync::mpsc::channel();
        let emitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_emitted = Arc::clone(&emitted);

        let worker = spawn_market_price_worker(
            vec![RewardObservation::certain("Forma Blueprint")],
            Arc::clone(&shared),
            vec![RewardCatalogEntry {
                name: "Forma Blueprint".to_owned(),
                ducats: 25,
            }],
            123,
            generation.clone(),
            move |_| {
                fetch_started_tx.send(()).expect("report fetch completion");
                release_fetch_rx.recv().expect("release price worker");
                (
                    BTreeMap::from([("Forma Blueprint".to_owned(), 12)]),
                    Some("delayed market failure"),
                )
            },
            move || {
                worker_emitted.fetch_add(1, Ordering::AcqRel);
            },
        );
        fetch_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("price worker reaches delayed publication");

        generation.request_stop();
        release_fetch_tx.send(()).expect("release price worker");
        worker.join().expect("price worker exits");

        let runtime = shared.lock().expect("runtime");
        let view = runtime.core.current_view().expect("view builds");
        assert!(
            view.reward().cards().is_empty(),
            "a retired price worker mutated the reward candidates"
        );
        assert_eq!(
            view.health().market().message(),
            "warframe.market pricing idle; nothing to price yet",
            "a retired price worker mutated market health"
        );
        assert_eq!(
            emitted.load(Ordering::Acquire),
            0,
            "a retired price worker emitted reward-updated"
        );
    }
    #[test]
    fn retiring_generation_waits_for_in_flight_publication() {
        let current = Arc::new(AtomicU64::new(0));
        let generation = MonitorGeneration::new(16, current);
        let worker_generation = generation.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_generation.publish(|| {
                entered_tx.send(()).expect("report publication start");
                release_rx.recv().expect("release publication");
            })
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("publication starts");

        let retiring_generation = generation.clone();
        let (retired_tx, retired_rx) = std::sync::mpsc::channel();
        let retire = std::thread::spawn(move || {
            retiring_generation.request_stop();
            retired_tx.send(()).expect("report retirement");
        });
        assert!(
            retired_rx.recv_timeout(Duration::from_millis(20)).is_err(),
            "retirement returned while a reward publication was still mutating runtime state"
        );

        release_tx.send(()).expect("release publication");
        retired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("retirement completes after publication");
        assert!(worker.join().expect("publication worker exits"));
        retire.join().expect("retirement worker exits");
    }

    #[test]
    fn retired_generation_rejects_delayed_reward_publication() {
        let current = Arc::new(AtomicU64::new(0));
        let generation = MonitorGeneration::new(17, Arc::clone(&current));
        let published = std::cell::Cell::new(false);

        assert!(generation.publish(|| published.set(true)));
        assert!(published.replace(false));

        current.fetch_add(1, Ordering::AcqRel);
        generation.request_stop();
        assert!(!generation.publish(|| published.set(true)));
        assert!(
            !published.get(),
            "a retired monitor must not publish a delayed price fetch"
        );
    }

    /// The smallest catalogue that still exercises every mark the ledger can return: a
    /// blueprint, a blade two builds need, and a handle.
    const GUANDAO_JSON: &str = r#"[{"uniqueName": "/Lotus/Weapons/Tenno/Melee/Polearms/GuandaoPrime",
  "name": "Guandao Prime", "category": "Melee", "type": "Melee", "masterable": true,
  "tradable": false, "masteryReq": 12, "maxLevelCap": 30, "components": [
  {"uniqueName": "/Lotus/Types/Recipes/Weapons/GuandaoPrimeBlueprint", "name": "Blueprint",
   "itemCount": 1, "tradable": true, "ducats": 45, "imageName": "blueprint.png"},
  {"uniqueName": "/Lotus/Types/Recipes/Weapons/WeaponParts/GuandaoPrimeBlade", "name": "Blade",
   "itemCount": 2, "tradable": true, "ducats": 15, "imageName": "GenericWeaponPrimeBlade.png"},
  {"uniqueName": "/Lotus/Types/Recipes/Weapons/WeaponParts/GuandaoPrimeHandle", "name": "Handle",
   "itemCount": 1, "tradable": true, "ducats": 100, "imageName": "GenericWeaponPrimeHandle.png"}]}]"#;

    fn guandao_catalog() -> CatalogIndex {
        CatalogIndex::from_wfcd_json(GUANDAO_JSON.as_bytes()).expect("Guandao catalogue parses")
    }

    /// A runtime whose core holds the Guandao recipes; each test sets the facts and preference
    /// a mark is built on. The directory comes back too, because the store lives in it.
    fn mastery_runtime() -> (tempfile::TempDir, SharedRuntime) {
        let directory = tempfile::tempdir().expect("temporary runtime");
        let shared = crate::tests::test_runtime(directory.path());
        shared
            .lock()
            .expect("runtime")
            .core
            .set_mastery_catalog(Arc::new(guandao_catalog()));
        (directory, shared)
    }

    /// A reward card carries its set only while marks are on, and one publish reads one
    /// snapshot, so two cards of the same item never disagree.
    #[test]
    fn reward_cards_carry_mastery_marks_only_while_the_preference_is_on() {
        let (_directory, shared) = mastery_runtime();
        let observation = RewardObservation::certain("Guandao Prime Blade");
        let mark = |shared: &SharedRuntime| {
            shared
                .lock()
                .expect("runtime")
                .core
                .current_view()
                .expect("view builds")
                .reward()
                .cards()
                .first()
                .expect("the observation became a card")
                .mastery
                .clone()
        };
        shared
            .lock()
            .expect("runtime")
            .core
            .set_mastery_facts(MasteryFacts::default());

        apply_reward_observations(
            &shared,
            &[],
            std::slice::from_ref(&observation),
            &BTreeMap::new(),
        );
        assert!(
            matches!(mark(&shared), Some(MasteryMark::Unmastered { .. })),
            "live facts name the set the blade belongs to"
        );

        shared.lock().expect("runtime").preferences.mastery_marks = false;
        apply_reward_observations(
            &shared,
            &[],
            std::slice::from_ref(&observation),
            &BTreeMap::new(),
        );
        assert_eq!(
            mark(&shared),
            None,
            "marks switched off leave the card as it was before the feature existed"
        );
    }

    /// The collection keeps built Forma beside Forma blueprints, and a built Ash Prime beside its
    /// blueprint. A slip's held count accepted the blueprint trim, so whichever came first by path
    /// answered for the reward: a Forma Blueprint slip read "Owned ×2" over eleven held blueprints
    /// and two built Forma (2026-10-02).
    #[test]
    fn a_reward_card_counts_the_reward_and_not_what_it_builds() {
        let directory = tempfile::tempdir().expect("temporary runtime");
        let shared = crate::tests::test_runtime(directory.path());
        let held = |path: &str, name: &str, category: Category, quantity: u32| {
            InventoryEntry::new(
                CatalogItem::new(ItemId::new(path).expect("item id"), name, category)
                    .expect("catalog item"),
                quantity,
            )
        };
        shared
            .lock()
            .expect("runtime")
            .core
            .apply_inventory_snapshot(
                InventorySnapshot::coherent(vec![
                    held(
                        "/Lotus/Powersuits/Ninja/AshPrime",
                        "Ash Prime",
                        Category::Frame,
                        1,
                    ),
                    held(
                        "/Lotus/Types/Items/MiscItems/Forma",
                        "Forma",
                        Category::Resource,
                        2,
                    ),
                    held(
                        "/Lotus/Types/Recipes/Components/FormaBlueprint",
                        "Forma Blueprint",
                        Category::Blueprint,
                        11,
                    ),
                ])
                .expect("collection"),
                SnapshotMeta::fake("saved").expect("meta"),
            )
            .expect("snapshot applies");

        apply_reward_observations(
            &shared,
            &[],
            &[
                RewardObservation::certain("Forma Blueprint"),
                RewardObservation::certain("2X Forma Blueprint"),
                RewardObservation::certain("Ash Prime Blueprint"),
            ],
            &BTreeMap::new(),
        );

        let owned = shared
            .lock()
            .expect("runtime")
            .core
            .current_view()
            .expect("view builds")
            .reward()
            .cards()
            .iter()
            .map(|card| (card.name.clone(), card.owned))
            .collect::<Vec<_>>();
        assert_eq!(
            owned,
            [
                ("Forma Blueprint".to_owned(), 11),
                ("2X Forma Blueprint".to_owned(), 11),
                ("Ash Prime Blueprint".to_owned(), 0),
            ]
        );
    }
}
