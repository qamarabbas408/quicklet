//! Runs file operations in the background with progress events and
//! cancellation.
//!
//! Commands validate their input first and then hand the work to
//! [`run_operation`], which claims the busy flag, emits `operation-progress`
//! events, runs the work on a blocking thread, and reports how it ended.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager};

use crate::progress::{Cancelled, OperationError, Progress};
use crate::{ConversionBusyGuard, InputState};

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgressOutput {
    name: Option<String>,
    directory: Option<String>,
    path: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgressSnapshot {
    operation_id: String,
    status: String,
    stage: String,
    message: String,
    determinate: bool,
    completed: Option<u64>,
    total: Option<u64>,
    unit: Option<String>,
    eta_seconds: Option<u64>,
    error: Option<String>,
    output: Option<OperationProgressOutput>,
}

impl OperationProgressSnapshot {
    fn new(operation_id: &str, status: &str, stage: &str, message: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.to_string(),
            status: status.to_string(),
            stage: stage.to_string(),
            message: message.into(),
            determinate: false,
            completed: None,
            total: None,
            unit: None,
            eta_seconds: None,
            error: None,
            output: None,
        }
    }

    fn with_counts(mut self, completed: u64, total: u64, unit: Option<&str>) -> Self {
        self.determinate = true;
        self.completed = Some(completed.min(total));
        self.total = Some(total);
        self.unit = unit.map(str::to_string);
        self
    }

    fn with_eta(mut self, eta_seconds: Option<u64>) -> Self {
        self.eta_seconds = eta_seconds;
        self
    }

    fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    fn with_output(mut self, output: OperationProgressOutput) -> Self {
        self.output = Some(output);
        self
    }
}

#[derive(Default)]
pub struct OperationProgressState {
    next_id: AtomicU64,
    current: Mutex<Option<OperationProgressSnapshot>>,
}

impl OperationProgressState {
    /// Starts a new operation, which makes events from earlier ones stale.
    fn begin(&self, kind: &str, message: &str) -> OperationProgressSnapshot {
        let next = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let snapshot = OperationProgressSnapshot::new(
            &format!("{kind}:{next}"),
            "running",
            "preparing",
            message,
        );
        if let Ok(mut current) = self.current.lock() {
            *current = Some(snapshot.clone());
        }
        snapshot
    }

    /// Stores `snapshot` unless it is stale, and returns whether it was kept.
    fn publish(&self, snapshot: &OperationProgressSnapshot) -> bool {
        let Ok(mut current) = self.current.lock() else {
            return false;
        };
        if let Some(existing) = current.as_ref()
            && !accepts(existing, snapshot)
        {
            return false;
        }
        *current = Some(snapshot.clone());
        true
    }

    fn current_operation_id(&self) -> Option<String> {
        self.snapshot().map(|snapshot| snapshot.operation_id)
    }

    fn snapshot(&self) -> Option<OperationProgressSnapshot> {
        self.current.lock().ok().and_then(|current| current.clone())
    }
}

/// An update is stale when it belongs to another operation, arrives after the
/// operation ended or started cancelling, or moves a stage backwards.
fn accepts(current: &OperationProgressSnapshot, next: &OperationProgressSnapshot) -> bool {
    if current.operation_id != next.operation_id || current.status != "running" {
        return false;
    }
    if next.status == "running" && current.stage == "cancelling" {
        return false;
    }
    if next.status == "running" && current.stage == next.stage {
        let moved_back = |current: Option<u64>, next: Option<u64>| {
            current
                .zip(next)
                .is_some_and(|(current, next)| next < current)
        };
        if moved_back(current.completed, next.completed) || moved_back(current.total, next.total) {
            return false;
        }
    }
    true
}

#[derive(Default)]
pub struct OperationCancellationState {
    tokens: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl OperationCancellationState {
    fn begin(&self, operation_id: &str) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.insert(operation_id.to_string(), token.clone());
        }
        token
    }

    fn request_cancel(&self, operation_id: &str) -> bool {
        let Some(token) = self
            .tokens
            .lock()
            .ok()
            .and_then(|tokens| tokens.get(operation_id).cloned())
        else {
            return false;
        };
        token.store(true, Ordering::SeqCst);
        true
    }

    fn clear(&self, operation_id: &str) {
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.remove(operation_id);
        }
    }
}

/// Estimates time remaining within a stage from a smoothed throughput.
#[derive(Default)]
struct EtaEstimator {
    stage: Option<String>,
    smoothed_rate: Option<f64>,
    observations: usize,
}

impl EtaEstimator {
    /// `elapsed_seconds` is measured from the start of `stage`.
    fn record(
        &mut self,
        stage: &str,
        completed: u64,
        total: u64,
        elapsed_seconds: f64,
    ) -> Option<u64> {
        if self.stage.as_deref() != Some(stage) {
            self.stage = Some(stage.to_string());
            self.smoothed_rate = None;
            self.observations = 0;
        }

        if total == 0 || completed == 0 || completed >= total || elapsed_seconds <= 0.0 {
            return None;
        }

        let throughput = completed as f64 / elapsed_seconds;
        let smoothed = match self.smoothed_rate {
            Some(current) => (current * 0.7) + (throughput * 0.3),
            None => throughput,
        };
        self.smoothed_rate = Some(smoothed);
        self.observations += 1;

        if self.observations < 2 || !smoothed.is_finite() || smoothed <= 0.0 {
            return None;
        }

        let eta = total.saturating_sub(completed) as f64 / smoothed;
        (eta.is_finite() && eta > 0.0).then(|| eta.round() as u64)
    }
}

/// Turns an operation stage into the message and unit shown to the user.
fn describe_stage(stage: &str, completed: u64, total: u64) -> (String, Option<&'static str>) {
    let (label, unit) = match stage {
        "reading" => ("Reading images", Some("images")),
        "loading" if total > 0 => ("Loading PDFs", Some("files")),
        "loading" => ("Loading PDF", None),
        "merging" => ("Merging PDFs", Some("files")),
        "writing" => ("Writing PDFs", Some("files")),
        "building" => ("Building PDF", None),
        "saving" => ("Saving PDF", None),
        "streams" => ("Compressing streams", Some("objects")),
        "images" => ("Inspecting images", Some("images")),
        "validating" => ("Validating PDF", None),
        "finalizing" => ("Finalizing PDF", None),
        "reading-image" => ("Reading image", None),
        "decoding" => ("Decoding image", None),
        "orienting" => ("Applying orientation", None),
        "encoding" => ("Encoding image", None),
        "saving-image" => ("Saving image", None),
        _ => ("Working", None),
    };
    if total > 0 {
        (format!("{label} {completed}/{total}"), unit)
    } else {
        (format!("{label}…"), None)
    }
}

/// Handed to operation work on the blocking thread.
pub struct OperationReporter {
    app: AppHandle,
    operation_id: String,
    cancel: Arc<AtomicBool>,
    eta: EtaEstimator,
    stage_started_at: Instant,
}

impl OperationReporter {
    /// For modules that take a cancel flag instead of [`Progress`].
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Emits a running update. A `total` of 0 reports an unmeasured stage.
    pub fn report(&mut self, stage: &str, completed: u64, total: u64) {
        if self.eta.stage.as_deref() != Some(stage) {
            self.stage_started_at = Instant::now();
        }
        let eta = self.eta.record(
            stage,
            completed,
            total,
            self.stage_started_at.elapsed().as_secs_f64(),
        );
        let (message, unit) = describe_stage(stage, completed, total);
        let mut snapshot =
            OperationProgressSnapshot::new(&self.operation_id, "running", stage, message);
        if total > 0 {
            snapshot = snapshot.with_counts(completed, total, unit).with_eta(eta);
        }
        emit_operation_progress(&self.app, &snapshot);
    }
}

impl Progress for OperationReporter {
    fn step(
        &mut self,
        stage: &'static str,
        completed: usize,
        total: usize,
    ) -> Result<(), Cancelled> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err(Cancelled);
        }
        self.report(stage, completed as u64, total as u64);
        Ok(())
    }
}

/// Where an operation writes its result.
pub enum OperationTarget {
    File(PathBuf),
    Directory(PathBuf),
}

impl OperationTarget {
    fn progress_output(&self) -> OperationProgressOutput {
        match self {
            Self::File(path) => OperationProgressOutput {
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
                directory: path.parent().map(|parent| parent.display().to_string()),
                path: Some(path.display().to_string()),
            },
            Self::Directory(path) => OperationProgressOutput {
                name: None,
                directory: Some(path.display().to_string()),
                path: None,
            },
        }
    }

    /// Remembers the result so it can be opened or revealed afterwards.
    fn record(&self, state: &InputState) {
        let (file, directory) = match self {
            Self::File(path) => (Some(path.clone()), None),
            Self::Directory(path) => (None, Some(path.clone())),
        };
        if let Ok(mut last_output) = state.last_output.lock() {
            *last_output = file;
        }
        if let Ok(mut last_output_directory) = state.last_output_directory.lock() {
            *last_output_directory = directory;
        }
    }
}

fn emit_operation_progress(app: &AppHandle, snapshot: &OperationProgressSnapshot) {
    if app.state::<OperationProgressState>().publish(snapshot) {
        let _ = app.emit("operation-progress", snapshot);
    }
}

/// Runs `work` on a blocking thread as the single active operation.
///
/// `kind` prefixes the operation id and `title` names the operation in
/// messages ("Merge cancelled"). A cancelled operation returns an error
/// containing "cancelled", which the frontend treats as not being a failure.
pub async fn run_operation<T, E, W>(
    app: &AppHandle,
    kind: &'static str,
    title: &'static str,
    target: OperationTarget,
    work: W,
) -> Result<T, String>
where
    T: Send + 'static,
    E: OperationError + Send + 'static,
    W: FnOnce(&mut OperationReporter) -> Result<T, E> + Send + 'static,
{
    let input_state = app.state::<InputState>();
    if input_state
        .conversion_in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A conversion is already running.".to_string());
    }

    let started = app
        .state::<OperationProgressState>()
        .begin(kind, &format!("Preparing {}…", title.to_lowercase()));
    let operation_id = started.operation_id.clone();
    let cancel = app
        .state::<OperationCancellationState>()
        .begin(&operation_id);
    let _ = app.emit("operation-progress", &started);

    let mut reporter = OperationReporter {
        app: app.clone(),
        operation_id: operation_id.clone(),
        cancel,
        eta: EtaEstimator::default(),
        stage_started_at: Instant::now(),
    };
    let guard_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _guard = ConversionBusyGuard { app: guard_app };
        work(&mut reporter)
    })
    .await;
    app.state::<OperationCancellationState>()
        .clear(&operation_id);

    let (result, snapshot) = match joined {
        Ok(Ok(value)) => {
            target.record(&input_state);
            let snapshot = OperationProgressSnapshot::new(
                &operation_id,
                "succeeded",
                "complete",
                format!("{title} complete"),
            )
            .with_output(target.progress_output());
            (Ok(value), snapshot)
        }
        Ok(Err(error)) if error.is_cancelled() => {
            let message = format!("{title} cancelled");
            let snapshot =
                OperationProgressSnapshot::new(&operation_id, "cancelled", "cancelled", &message);
            (Err(message), snapshot)
        }
        Ok(Err(error)) => {
            let message = error.to_string();
            let snapshot = OperationProgressSnapshot::new(
                &operation_id,
                "failed",
                "error",
                format!("{title} failed"),
            )
            .with_error(&message);
            (Err(message), snapshot)
        }
        Err(error) => {
            input_state
                .conversion_in_progress
                .store(false, Ordering::SeqCst);
            let message = format!("{title} failed: {error}");
            let snapshot = OperationProgressSnapshot::new(
                &operation_id,
                "failed",
                "error",
                format!("{title} failed"),
            )
            .with_error(&message);
            (Err(message), snapshot)
        }
    };
    emit_operation_progress(app, &snapshot);
    result
}

#[tauri::command]
pub fn get_operation_progress(app: AppHandle) -> Option<OperationProgressSnapshot> {
    app.state::<OperationProgressState>().snapshot()
}

#[tauri::command]
pub fn cancel_operation(app: AppHandle, operation_id: Option<String>) -> Result<(), String> {
    let target = operation_id
        .or_else(|| app.state::<OperationProgressState>().current_operation_id())
        .ok_or_else(|| "No active operation to cancel.".to_string())?;

    if !app
        .state::<OperationCancellationState>()
        .request_cancel(&target)
    {
        return Err("No active operation to cancel.".to_string());
    }

    emit_operation_progress(
        &app,
        &OperationProgressSnapshot::new(&target, "running", "cancelling", "Cancelling…"),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(id: &str, stage: &str, completed: u64, total: u64) -> OperationProgressSnapshot {
        OperationProgressSnapshot::new(id, "running", stage, "").with_counts(completed, total, None)
    }

    #[test]
    fn operation_ids_are_unique_and_newest_is_current() {
        let state = OperationProgressState::default();
        let first = state.begin("images.convert_to_pdf", "Preparing conversion");
        let second = state.begin("images.convert_to_pdf", "Preparing conversion");

        assert_ne!(first.operation_id, second.operation_id);
        assert_eq!(state.current_operation_id(), Some(second.operation_id));
    }

    #[test]
    fn ignores_events_from_earlier_operations() {
        let state = OperationProgressState::default();
        let first = state.begin("pdf.merge", "Preparing merge");
        let second = state.begin("pdf.merge", "Preparing merge");

        assert!(!state.publish(&running(&first.operation_id, "loading", 1, 4)));
        assert!(state.publish(&running(&second.operation_id, "loading", 1, 4)));
    }

    #[test]
    fn ignores_progress_that_moves_backwards_within_a_stage() {
        let state = OperationProgressState::default();
        let id = state.begin("pdf.merge", "Preparing merge").operation_id;

        assert!(state.publish(&running(&id, "loading", 2, 4)));
        assert!(!state.publish(&running(&id, "loading", 1, 4)));
        assert!(!state.publish(&running(&id, "loading", 3, 3)));
    }

    #[test]
    fn accepts_a_new_stage_with_smaller_counts() {
        let state = OperationProgressState::default();
        let id = state
            .begin("pdf.compress", "Preparing compression")
            .operation_id;

        assert!(state.publish(&running(&id, "streams", 900, 1000)));
        assert!(state.publish(&running(&id, "images", 1, 12)));
    }

    #[test]
    fn ignores_running_updates_after_cancelling_or_finishing() {
        let state = OperationProgressState::default();
        let id = state.begin("pdf.split", "Preparing split").operation_id;

        assert!(state.publish(&OperationProgressSnapshot::new(
            &id,
            "running",
            "cancelling",
            ""
        )));
        assert!(!state.publish(&running(&id, "writing", 2, 5)));
        assert!(state.publish(&OperationProgressSnapshot::new(
            &id,
            "cancelled",
            "cancelled",
            ""
        )));
        assert!(!state.publish(&OperationProgressSnapshot::new(
            &id,
            "succeeded",
            "complete",
            ""
        )));
    }

    #[test]
    fn counts_never_exceed_the_total() {
        let snapshot = running("pdf.merge:1", "loading", 5, 3);
        assert_eq!(snapshot.completed, Some(3));
    }

    #[test]
    fn eta_needs_two_observations() {
        let mut estimator = EtaEstimator::default();

        assert_eq!(estimator.record("streams", 1, 10, 0.25), None);
        assert_eq!(estimator.record("streams", 5, 10, 1.5), Some(1));
        assert_eq!(estimator.record("streams", 7, 10, 2.5), Some(1));
    }

    #[test]
    fn eta_resets_when_stage_changes() {
        let mut estimator = EtaEstimator::default();

        assert_eq!(estimator.record("streams", 6, 10, 3.0), None);
        assert_eq!(estimator.record("saving", 1, 1, 0.25), None);
        assert_eq!(estimator.record("streams", 8, 10, 3.5), None);
        assert_eq!(estimator.record("streams", 9, 10, 4.0), Some(0));
    }

    #[test]
    fn describes_measured_and_unmeasured_stages() {
        assert_eq!(
            describe_stage("reading", 2, 4),
            ("Reading images 2/4".to_string(), Some("images"))
        );
        assert_eq!(
            describe_stage("saving", 0, 0),
            ("Saving PDF…".to_string(), None)
        );
    }

    #[test]
    fn cancellation_tracks_active_operation_tokens() {
        let state = OperationCancellationState::default();
        let token = state.begin("pdf.compress:9");

        assert!(!token.load(Ordering::SeqCst));
        assert!(state.request_cancel("pdf.compress:9"));
        assert!(token.load(Ordering::SeqCst));

        state.clear("pdf.compress:9");
        assert!(!state.request_cancel("pdf.compress:9"));
    }
}
