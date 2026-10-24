pub mod compress;
pub mod merge;
pub mod naming;
pub mod organize;
pub mod pdf;
pub mod split;

use chrono::Local;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;

const MENU_WINDOW_LABEL: &str = "main";
const DEFAULT_MENU_SIZE: f64 = 440.0;
const POSITION_MARGIN_LOGICAL: f64 = 8.0;
const BLUR_GRACE_PERIOD: Duration = Duration::from_millis(350);
const MAX_INPUT_FILES: usize = 8;

#[cfg(target_os = "macos")]
const SHORTCUT_LABEL: &str = "Cmd+Shift+Space";
#[cfg(not(target_os = "macos"))]
const SHORTCUT_LABEL: &str = "Ctrl+Shift+Space";

#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(QuickletPanel {
        config: {
            can_become_key_window: true,
            is_floating_panel: true
        }
    })
}

#[derive(Default)]
struct MenuState {
    last_shown_at: Mutex<Option<Instant>>,
    shortcut: Mutex<ShortcutStatus>,
}

#[derive(Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ShortcutStatus {
    registered: bool,
    label: String,
    error: Option<String>,
}

#[derive(Default)]
struct InputState {
    next_id: Mutex<u64>,
    action_id: Mutex<Option<String>>,
    files: Mutex<Vec<InputFile>>,
    output_path: Mutex<Option<PathBuf>>,
    split_output_dir: Mutex<Option<PathBuf>>,
    last_output: Mutex<Option<PathBuf>>,
    last_output_directory: Mutex<Option<PathBuf>>,
    dialog_open: AtomicBool,
    drop_mode: AtomicBool,
    conversion_in_progress: AtomicBool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionFile {
    name: String,
    pages: usize,
    bytes_written: u64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionCompression {
    original_bytes: u64,
    output_bytes: u64,
    saved_bytes: u64,
    reduction_percent: f64,
    preset: String,
    status: String,
    images_recompressed: usize,
    images_skipped: usize,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionResult {
    pages: usize,
    bytes_written: u64,
    name: String,
    directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<Vec<ConversionFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compression: Option<ConversionCompression>,
}

struct ConversionBusyGuard {
    app: AppHandle,
}

impl Drop for ConversionBusyGuard {
    fn drop(&mut self) {
        self.app
            .state::<InputState>()
            .conversion_in_progress
            .store(false, Ordering::SeqCst);
    }
}

fn ensure_not_converting(state: &InputState) -> Result<(), String> {
    if state.conversion_in_progress.load(Ordering::SeqCst) {
        Err("A conversion is already running.".to_string())
    } else {
        Ok(())
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionInfo {
    id: String,
    label: String,
    description: String,
    enabled: bool,
    input: Option<String>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputInfo {
    name: String,
    directory: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputDirectoryInfo {
    directory: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SplitSourceInfo {
    name: String,
    pages: usize,
    bytes: u64,
    max_pages: usize,
    max_outputs: usize,
    max_input_bytes: u64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CompressSourceInfo {
    name: String,
    pages: usize,
    bytes: u64,
    has_signature: bool,
    images: usize,
    max_pages: usize,
    max_images: usize,
    max_input_bytes: u64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PageSourceInfo {
    name: String,
    pages: usize,
    bytes: u64,
    max_pages: usize,
    max_ranges: usize,
    max_input_bytes: u64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InputFile {
    id: u64,
    name: String,
    extension: Option<String>,
    kind: String,
    size: Option<u64>,
    #[serde(skip)]
    #[allow(dead_code)]
    path: PathBuf,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InputSelection {
    action_id: String,
    files: Vec<InputFile>,
    limit: usize,
    duplicates: usize,
    limit_skipped: usize,
    rejected: usize,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DragState {
    active: bool,
    count: usize,
}

struct IngestOutcome {
    selection: InputSelection,
    accepted: usize,
}

fn menu_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let modifiers = Modifiers::SUPER | Modifiers::SHIFT;
    #[cfg(not(target_os = "macos"))]
    let modifiers = Modifiers::CONTROL | Modifiers::SHIFT;

    Shortcut::new(Some(modifiers), Code::Space)
}

#[tauri::command]
fn open_menu(app: AppHandle, x: Option<f64>, y: Option<f64>) -> Result<(), String> {
    show_menu(&app, x, y)
}

#[tauri::command]
fn dismiss_menu(app: AppHandle) -> Result<(), String> {
    if app
        .state::<InputState>()
        .conversion_in_progress
        .load(Ordering::SeqCst)
    {
        return Ok(());
    }

    clear_input_state(&app.state::<InputState>());

    #[cfg(target_os = "macos")]
    {
        let panel = menu_panel(&app)?;
        panel.hide();
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let window = menu_window(&app)?;
        window.hide().map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn get_menu_status(app: AppHandle) -> ShortcutStatus {
    app.state::<MenuState>()
        .shortcut
        .lock()
        .map(|status| status.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

#[tauri::command]
async fn pick_input_files(
    app: AppHandle,
    window: WebviewWindow,
    append: bool,
) -> Result<Option<InputSelection>, String> {
    ensure_not_converting(&app.state::<InputState>())?;

    let action_id = app
        .state::<InputState>()
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None)
        .ok_or_else(|| "Choose an action first.".to_string())?;

    if append {
        let current_count = app
            .state::<InputState>()
            .files
            .lock()
            .map(|files| files.len())
            .unwrap_or(0);
        let limit = max_files_for_action(&action_id);
        if current_count >= limit {
            return Err(format!(
                "Maximum of {limit} {} reached. Remove a file first.",
                if limit == 1 { "file" } else { "files" }
            ));
        }
    }

    app.state::<InputState>()
        .dialog_open
        .store(true, Ordering::SeqCst);

    let mut dialog = app
        .dialog()
        .file()
        .set_title("Select files")
        .set_parent(&window);

    if let Some((name, extensions)) = filter_for_action(&action_id) {
        dialog = dialog.add_filter(name, extensions);
    }

    let (sender, receiver) = std::sync::mpsc::channel();
    dialog.pick_files(move |paths| {
        let _ = sender.send(paths);
    });

    let received =
        tauri::async_runtime::spawn_blocking(move || receiver.recv().ok().flatten()).await;

    app.state::<InputState>()
        .dialog_open
        .store(false, Ordering::SeqCst);
    note_menu_shown(&app);

    let picked = received.map_err(|error| error.to_string())?;
    let Some(file_paths) = picked else {
        return Ok(None);
    };
    if file_paths.is_empty() {
        return Ok(None);
    }

    let paths: Vec<PathBuf> = file_paths
        .into_iter()
        .filter_map(|path| path.into_path().ok())
        .collect();
    if paths.is_empty() {
        return Ok(None);
    }

    let state = app.state::<InputState>();
    let outcome = ingest_paths(&state, &action_id, paths, append);

    if !append && outcome.accepted == 0 {
        return Err("No readable files were selected.".to_string());
    }

    Ok(Some(outcome.selection))
}

#[tauri::command]
fn set_drop_mode(app: AppHandle, active: bool) -> Result<(), String> {
    if active {
        ensure_not_converting(&app.state::<InputState>())?;
    }

    app.state::<InputState>()
        .drop_mode
        .store(active, Ordering::SeqCst);

    if !active {
        note_menu_shown(&app);
    }

    Ok(())
}

#[tauri::command]
fn get_category_actions(category_id: String) -> Vec<ActionInfo> {
    category_actions(&category_id)
}

#[tauri::command]
fn start_action(app: AppHandle, action_id: String) -> Result<ActionInfo, String> {
    ensure_not_converting(&app.state::<InputState>())?;

    let action = find_action(&action_id).ok_or_else(|| "Unknown action.".to_string())?;

    if !action.enabled {
        return Err("This action is not available yet.".to_string());
    }

    let state = app.state::<InputState>();
    let current_action = state
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None);

    if current_action.as_deref() != Some(action_id.as_str()) {
        clear_input_state(&state);
        if let Ok(mut stored_action) = state.action_id.lock() {
            *stored_action = Some(action_id);
        }
    }

    Ok(action)
}

#[tauri::command]
fn get_input(app: AppHandle) -> InputSelection {
    input_snapshot(&app.state::<InputState>())
}

#[tauri::command]
fn reorder_input(app: AppHandle, ids: Vec<u64>) -> Result<InputSelection, String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;
    let mut files = state
        .files
        .lock()
        .map_err(|_| "Input state is unavailable.".to_string())?;
    let mut by_id: HashMap<u64, InputFile> = files.drain(..).map(|file| (file.id, file)).collect();
    let mut ordered = Vec::with_capacity(by_id.len());

    for id in ids {
        if let Some(file) = by_id.remove(&id) {
            ordered.push(file);
        }
    }

    let mut remaining: Vec<InputFile> = by_id.into_values().collect();
    remaining.sort_by_key(|file| file.id);
    ordered.extend(remaining);
    *files = ordered;
    drop(files);

    Ok(input_snapshot(&state))
}

#[tauri::command]
async fn choose_output(
    app: AppHandle,
    window: WebviewWindow,
    preset: Option<String>,
) -> Result<Option<OutputInfo>, String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;
    let action_id = state
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None)
        .ok_or_else(|| "Choose an action first.".to_string())?;
    let files = state
        .files
        .lock()
        .map(|files| files.clone())
        .unwrap_or_default();

    if files.is_empty() {
        return Err("Add files before choosing an output.".to_string());
    }

    let compression_preset = if action_id == "pdf.compress" {
        let preset = preset
            .as_deref()
            .ok_or_else(|| "Choose a compression preset first.".to_string())?;
        Some(
            compress::CompressionPreset::parse(preset)
                .ok_or_else(|| "Choose a valid compression preset.".to_string())?,
        )
    } else {
        None
    };
    let default_name = naming::default_output_name(&action_id, compression_preset, Local::now())
        .ok_or_else(|| "This action does not use a single output file.".to_string())?;

    state.dialog_open.store(true, Ordering::SeqCst);

    let dialog = app
        .dialog()
        .file()
        .set_title("Save PDF")
        .set_parent(&window)
        .add_filter("PDF", &["pdf"])
        .set_file_name(default_name);

    let (sender, receiver) = std::sync::mpsc::channel();
    dialog.save_file(move |path| {
        let _ = sender.send(path);
    });

    let received =
        tauri::async_runtime::spawn_blocking(move || receiver.recv().ok().flatten()).await;

    state.dialog_open.store(false, Ordering::SeqCst);
    note_menu_shown(&app);

    let picked = received.map_err(|error| error.to_string())?;
    let Some(file_path) = picked else {
        return Ok(None);
    };

    let path = ensure_pdf_extension(file_path.into_path().map_err(|error| error.to_string())?);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output.pdf".to_string());
    let directory = path
        .parent()
        .map(|directory| directory.display().to_string())
        .unwrap_or_default();

    if let Ok(mut output_path) = state.output_path.lock() {
        *output_path = Some(path);
    }
    if let Ok(mut split_output_dir) = state.split_output_dir.lock() {
        *split_output_dir = None;
    }
    if let Ok(mut last_output) = state.last_output.lock() {
        *last_output = None;
    }
    if let Ok(mut last_output_directory) = state.last_output_directory.lock() {
        *last_output_directory = None;
    }

    Ok(Some(OutputInfo { name, directory }))
}

#[tauri::command]
async fn convert_to_pdf(app: AppHandle) -> Result<ConversionResult, String> {
    let (paths, output) = {
        let state = app.state::<InputState>();
        ensure_not_converting(&state)?;

        let action_id = state
            .action_id
            .lock()
            .map(|action_id| action_id.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an action first.".to_string())?;

        if action_id != "images.convert_to_pdf" {
            return Err("This action is not available yet.".to_string());
        }

        let files = state
            .files
            .lock()
            .map(|files| files.clone())
            .unwrap_or_default();

        if files.is_empty() {
            return Err("Add images before converting.".to_string());
        }

        let output = state
            .output_path
            .lock()
            .map(|output| output.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an output destination first.".to_string())?;

        if state
            .conversion_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("A conversion is already running.".to_string());
        }

        (
            files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>(),
            output,
        )
    };

    let guard_app = app.clone();
    let task_output = output.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _guard = ConversionBusyGuard { app: guard_app };
        pdf::convert_images_to_pdf_with_overwrite(&paths, &task_output)
    })
    .await;

    let outcome = match joined {
        Ok(result) => result.map_err(|error| error.to_string())?,
        Err(error) => {
            app.state::<InputState>()
                .conversion_in_progress
                .store(false, Ordering::SeqCst);
            return Err(error.to_string());
        }
    };

    if let Ok(mut last_output) = app.state::<InputState>().last_output.lock() {
        *last_output = Some(output.clone());
    }
    if let Ok(mut last_output_directory) = app.state::<InputState>().last_output_directory.lock() {
        *last_output_directory = None;
    }

    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output.pdf".to_string());
    let directory = output
        .parent()
        .map(|directory| directory.display().to_string())
        .unwrap_or_default();

    Ok(ConversionResult {
        pages: outcome.pages,
        bytes_written: outcome.bytes_written,
        name,
        directory,
        files: None,
        compression: None,
    })
}

#[tauri::command]
async fn merge_pdfs(app: AppHandle) -> Result<ConversionResult, String> {
    let (paths, output) = {
        let state = app.state::<InputState>();
        ensure_not_converting(&state)?;

        let action_id = state
            .action_id
            .lock()
            .map(|action_id| action_id.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an action first.".to_string())?;

        if action_id != "pdf.merge" {
            return Err("This action is not available yet.".to_string());
        }

        let files = state
            .files
            .lock()
            .map(|files| files.clone())
            .unwrap_or_default();

        if files.len() < 2 {
            return Err("Select at least two PDF files to merge.".to_string());
        }

        let output = state
            .output_path
            .lock()
            .map(|output| output.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an output destination first.".to_string())?;

        if state
            .conversion_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("A conversion is already running.".to_string());
        }

        (
            files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>(),
            output,
        )
    };

    let guard_app = app.clone();
    let task_output = output.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _guard = ConversionBusyGuard { app: guard_app };
        merge::merge_pdfs_with_overwrite(&paths, &task_output)
    })
    .await;

    let outcome = match joined {
        Ok(result) => result.map_err(|error| error.to_string())?,
        Err(error) => {
            app.state::<InputState>()
                .conversion_in_progress
                .store(false, Ordering::SeqCst);
            return Err(error.to_string());
        }
    };

    if let Ok(mut last_output) = app.state::<InputState>().last_output.lock() {
        *last_output = Some(output.clone());
    }
    if let Ok(mut last_output_directory) = app.state::<InputState>().last_output_directory.lock() {
        *last_output_directory = None;
    }

    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output.pdf".to_string());
    let directory = output
        .parent()
        .map(|directory| directory.display().to_string())
        .unwrap_or_default();

    Ok(ConversionResult {
        pages: outcome.pages,
        bytes_written: outcome.bytes_written,
        name,
        directory,
        files: None,
        compression: None,
    })
}

#[tauri::command]
async fn inspect_split_source(app: AppHandle) -> Result<SplitSourceInfo, String> {
    let path = split_input_path(&app)?;
    let task_path = path.clone();

    let info = tauri::async_runtime::spawn_blocking(move || split::inspect_pdf(&task_path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.pdf".to_string());

    Ok(SplitSourceInfo {
        name,
        pages: info.pages,
        bytes: info.bytes,
        max_pages: split::MAX_SPLIT_PAGES,
        max_outputs: split::MAX_SPLIT_OUTPUTS,
        max_input_bytes: split::MAX_SPLIT_INPUT_BYTES,
    })
}

#[tauri::command]
fn plan_split(
    app: AppHandle,
    page_count: usize,
    mode: String,
    ranges: String,
) -> Result<split::SplitPlan, String> {
    let path = split_input_path(&app)?;
    let mode =
        split::SplitMode::parse(&mode).ok_or_else(|| "Choose a valid split mode.".to_string())?;
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Document".to_string());

    split::plan_split(page_count, &stem, mode, &ranges).map_err(|error| error.to_string())
}

#[tauri::command]
async fn choose_output_directory(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<Option<OutputDirectoryInfo>, String> {
    split_input_path(&app)?;
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;

    state.dialog_open.store(true, Ordering::SeqCst);

    let dialog = app
        .dialog()
        .file()
        .set_title("Choose output folder")
        .set_parent(&window);

    let (sender, receiver) = std::sync::mpsc::channel();
    dialog.pick_folder(move |path| {
        let _ = sender.send(path);
    });

    let received =
        tauri::async_runtime::spawn_blocking(move || receiver.recv().ok().flatten()).await;

    state.dialog_open.store(false, Ordering::SeqCst);
    note_menu_shown(&app);

    let picked = received.map_err(|error| error.to_string())?;
    let Some(folder) = picked else {
        return Ok(None);
    };

    let path = folder.into_path().map_err(|error| error.to_string())?;
    if !path.is_dir() {
        return Err("Choose an existing folder.".to_string());
    }

    if let Ok(mut split_output_dir) = state.split_output_dir.lock() {
        *split_output_dir = Some(path.clone());
    }
    if let Ok(mut output_path) = state.output_path.lock() {
        *output_path = None;
    }
    if let Ok(mut last_output) = state.last_output.lock() {
        *last_output = None;
    }
    if let Ok(mut last_output_directory) = state.last_output_directory.lock() {
        *last_output_directory = None;
    }

    Ok(Some(OutputDirectoryInfo {
        directory: path.display().to_string(),
    }))
}

#[tauri::command]
async fn split_pdf(
    app: AppHandle,
    mode: String,
    ranges: String,
) -> Result<ConversionResult, String> {
    let (path, output_directory) = {
        let state = app.state::<InputState>();
        ensure_not_converting(&state)?;

        let action_id = state
            .action_id
            .lock()
            .map(|action_id| action_id.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an action first.".to_string())?;

        if action_id != "pdf.split" {
            return Err("This action is not available yet.".to_string());
        }

        let files = state
            .files
            .lock()
            .map(|files| files.clone())
            .unwrap_or_default();

        if files.len() != 1 {
            return Err("Select exactly one PDF to split.".to_string());
        }

        let output_directory = state
            .split_output_dir
            .lock()
            .map(|directory| directory.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an output folder first.".to_string())?;

        if state
            .conversion_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("A conversion is already running.".to_string());
        }

        (files[0].path.clone(), output_directory)
    };

    let mode =
        split::SplitMode::parse(&mode).ok_or_else(|| "Choose a valid split mode.".to_string())?;

    let guard_app = app.clone();
    let task_directory = output_directory.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _guard = ConversionBusyGuard { app: guard_app };
        split::split_pdf(&path, &task_directory, mode, &ranges)
    })
    .await;

    let outcome = match joined {
        Ok(result) => result.map_err(|error| error.to_string())?,
        Err(error) => {
            app.state::<InputState>()
                .conversion_in_progress
                .store(false, Ordering::SeqCst);
            return Err(error.to_string());
        }
    };

    let Some(first_file) = outcome.files.first() else {
        return Err("No output files were created.".to_string());
    };
    let first_output = output_directory.join(&first_file.name);
    if let Ok(mut last_output) = app.state::<InputState>().last_output.lock() {
        *last_output = Some(first_output);
    }
    if let Ok(mut last_output_directory) = app.state::<InputState>().last_output_directory.lock() {
        *last_output_directory = Some(output_directory.clone());
    }

    let files = outcome
        .files
        .into_iter()
        .map(|file| ConversionFile {
            name: file.name,
            pages: file.pages,
            bytes_written: file.bytes_written,
        })
        .collect::<Vec<_>>();

    Ok(ConversionResult {
        pages: outcome.pages,
        bytes_written: outcome.bytes_written,
        name: format!(
            "{} {}",
            files.len(),
            if files.len() == 1 { "PDF" } else { "PDFs" }
        ),
        directory: output_directory.display().to_string(),
        files: Some(files),
        compression: None,
    })
}

#[tauri::command]
fn open_output_directory(app: AppHandle) -> Result<(), String> {
    let state = app.state::<InputState>();
    let directory = state
        .last_output_directory
        .lock()
        .map(|directory| directory.clone())
        .unwrap_or(None)
        .or_else(|| {
            state
                .last_output
                .lock()
                .ok()
                .and_then(|path| path.clone())
                .and_then(|path| path.parent().map(PathBuf::from))
        })
        .ok_or_else(|| "No split output yet.".to_string())?;

    app.opener()
        .open_path(directory.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn inspect_compress_source(app: AppHandle) -> Result<CompressSourceInfo, String> {
    let path = compress_input_path(&app)?;
    let task_path = path.clone();

    let source = tauri::async_runtime::spawn_blocking(move || compress::inspect_pdf(&task_path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.pdf".to_string());

    Ok(CompressSourceInfo {
        name,
        pages: source.pages,
        bytes: source.bytes,
        has_signature: source.has_signature,
        images: source.images,
        max_pages: compress::MAX_COMPRESS_PAGES,
        max_images: compress::MAX_COMPRESS_IMAGE_OBJECTS,
        max_input_bytes: compress::MAX_COMPRESS_INPUT_BYTES,
    })
}

#[tauri::command]
async fn compress_pdf(app: AppHandle, preset: String) -> Result<ConversionResult, String> {
    let (path, output_path, preset) = {
        let state = app.state::<InputState>();
        ensure_not_converting(&state)?;

        let action_id = state
            .action_id
            .lock()
            .map(|action_id| action_id.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an action first.".to_string())?;

        if action_id != "pdf.compress" {
            return Err("This action is not available yet.".to_string());
        }

        let files = state
            .files
            .lock()
            .map(|files| files.clone())
            .unwrap_or_default();

        if files.len() != 1 {
            return Err("Select exactly one PDF to compress.".to_string());
        }

        let output_path = state
            .output_path
            .lock()
            .map(|output| output.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an output destination first.".to_string())?;

        let preset = compress::CompressionPreset::parse(&preset)
            .ok_or_else(|| "Choose a valid compression preset.".to_string())?;

        if state
            .conversion_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("A conversion is already running.".to_string());
        }

        (files[0].path.clone(), output_path, preset)
    };

    let guard_app = app.clone();
    let task_output = output_path.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _guard = ConversionBusyGuard { app: guard_app };
        compress::compress_pdf_with_overwrite(&path, &task_output, preset)
    })
    .await;

    let outcome = match joined {
        Ok(result) => result.map_err(|error| error.to_string())?,
        Err(error) => {
            app.state::<InputState>()
                .conversion_in_progress
                .store(false, Ordering::SeqCst);
            return Err(error.to_string());
        }
    };

    if let Ok(mut last_output) = app.state::<InputState>().last_output.lock() {
        *last_output = Some(output_path.clone());
    }
    if let Ok(mut last_output_directory) = app.state::<InputState>().last_output_directory.lock() {
        *last_output_directory = None;
    }

    let name = output_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output.pdf".to_string());
    let directory = output_path
        .parent()
        .map(|directory| directory.display().to_string())
        .unwrap_or_default();
    let status = match outcome.status {
        compress::CompressionStatus::Reduced => "reduced",
        compress::CompressionStatus::NoReduction => "no-reduction",
    };

    Ok(ConversionResult {
        pages: outcome.pages,
        bytes_written: outcome.output_bytes,
        name,
        directory,
        files: None,
        compression: Some(ConversionCompression {
            original_bytes: outcome.original_bytes,
            output_bytes: outcome.output_bytes,
            saved_bytes: outcome.saved_bytes,
            reduction_percent: outcome.reduction_percent,
            preset: preset.label().to_string(),
            status: status.to_string(),
            images_recompressed: outcome.images_recompressed,
            images_skipped: outcome.images_skipped,
        }),
    })
}

#[tauri::command]
async fn inspect_page_source(app: AppHandle) -> Result<PageSourceInfo, String> {
    let (_, path) = page_selection_input_path(&app)?;
    let task_path = path.clone();

    let source = tauri::async_runtime::spawn_blocking(move || split::inspect_pdf(&task_path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.pdf".to_string());

    Ok(PageSourceInfo {
        name,
        pages: source.pages,
        bytes: source.bytes,
        max_pages: split::MAX_SPLIT_PAGES,
        max_ranges: split::MAX_SPLIT_RANGES,
        max_input_bytes: split::MAX_SPLIT_INPUT_BYTES,
    })
}

#[tauri::command]
fn plan_page_selection(
    app: AppHandle,
    page_count: usize,
    selection: String,
) -> Result<organize::PageSelectionPlan, String> {
    let (action_id, _) = page_selection_input_path(&app)?;
    let mode = organize::PageSelectionMode::from_action_id(&action_id)
        .ok_or_else(|| "This action is not available yet.".to_string())?;

    organize::plan(page_count, &selection, mode).map_err(|error| error.to_string())
}

#[tauri::command]
async fn extract_pages(app: AppHandle, selection: String) -> Result<ConversionResult, String> {
    run_page_selection(app, organize::PageSelectionMode::Extract, selection).await
}

#[tauri::command]
async fn delete_pages(app: AppHandle, selection: String) -> Result<ConversionResult, String> {
    run_page_selection(app, organize::PageSelectionMode::Delete, selection).await
}

async fn run_page_selection(
    app: AppHandle,
    mode: organize::PageSelectionMode,
    selection: String,
) -> Result<ConversionResult, String> {
    let (action_id, path) = page_selection_input_path(&app)?;
    if organize::PageSelectionMode::from_action_id(&action_id) != Some(mode) {
        return Err("This action is not available yet.".to_string());
    }

    let output_path = {
        let state = app.state::<InputState>();
        let output_path = state
            .output_path
            .lock()
            .map(|output| output.clone())
            .unwrap_or(None)
            .ok_or_else(|| "Choose an output destination first.".to_string())?;

        if state
            .conversion_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("A conversion is already running.".to_string());
        }

        output_path
    };

    let guard_app = app.clone();
    let task_output = output_path.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _guard = ConversionBusyGuard { app: guard_app };
        match mode {
            organize::PageSelectionMode::Extract => {
                organize::extract(&path, &task_output, &selection, true)
            }
            organize::PageSelectionMode::Delete => {
                organize::delete(&path, &task_output, &selection, true)
            }
        }
    })
    .await;

    let outcome = match joined {
        Ok(result) => result.map_err(|error| error.to_string())?,
        Err(error) => {
            app.state::<InputState>()
                .conversion_in_progress
                .store(false, Ordering::SeqCst);
            return Err(error.to_string());
        }
    };

    if let Ok(mut last_output) = app.state::<InputState>().last_output.lock() {
        *last_output = Some(output_path.clone());
    }
    if let Ok(mut last_output_directory) = app.state::<InputState>().last_output_directory.lock() {
        *last_output_directory = None;
    }

    let name = output_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output.pdf".to_string());
    let directory = output_path
        .parent()
        .map(|directory| directory.display().to_string())
        .unwrap_or_default();

    Ok(ConversionResult {
        pages: outcome.pages,
        bytes_written: outcome.bytes_written,
        name,
        directory,
        files: None,
        compression: None,
    })
}

fn page_selection_input_path(app: &AppHandle) -> Result<(String, PathBuf), String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;

    let action_id = state
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None)
        .ok_or_else(|| "Choose an action first.".to_string())?;

    if organize::PageSelectionMode::from_action_id(&action_id).is_none() {
        return Err("This action is not available yet.".to_string());
    }

    let files = state
        .files
        .lock()
        .map(|files| files.clone())
        .unwrap_or_default();

    if files.len() != 1 {
        return Err("Select exactly one PDF.".to_string());
    }

    Ok((action_id, files[0].path.clone()))
}

fn last_output_path(state: &InputState) -> Result<PathBuf, String> {
    state
        .last_output
        .lock()
        .map(|output| output.clone())
        .unwrap_or(None)
        .ok_or_else(|| "No converted PDF yet.".to_string())
}

#[tauri::command]
fn reveal_output(app: AppHandle) -> Result<(), String> {
    let path = last_output_path(&app.state::<InputState>())?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_output(app: AppHandle) -> Result<(), String> {
    let path = last_output_path(&app.state::<InputState>())?;
    app.opener()
        .open_path(path.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| error.to_string())
}

fn ensure_pdf_extension(path: PathBuf) -> PathBuf {
    let has_pdf_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));

    if has_pdf_extension {
        path
    } else {
        path.with_extension("pdf")
    }
}

fn ingest_paths(
    state: &InputState,
    action_id: &str,
    paths: Vec<PathBuf>,
    append: bool,
) -> IngestOutcome {
    let existing = if append {
        state
            .files
            .lock()
            .map(|files| files.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let limit = max_files_for_action(action_id);
    let capacity = if append {
        limit.saturating_sub(existing.len())
    } else {
        limit
    };

    let mut next_id = state.next_id.lock().map(|id| *id).unwrap_or(0);
    let mut new_files: Vec<InputFile> = Vec::new();
    let mut duplicates: usize = 0;
    let mut limit_skipped: usize = 0;
    let mut rejected: usize = 0;

    for path in paths {
        let metadata = std::fs::metadata(&path).ok();
        let is_directory = metadata
            .as_ref()
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);

        if is_directory {
            rejected += 1;
            continue;
        }

        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase());

        if !extension_is_allowed(action_id, extension.as_deref()) {
            rejected += 1;
            continue;
        }

        if existing.iter().any(|file| file.path == path)
            || new_files.iter().any(|file| file.path == path)
        {
            duplicates += 1;
            continue;
        }

        if new_files.len() >= capacity {
            limit_skipped += 1;
            continue;
        }

        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let size = metadata.map(|metadata| metadata.len());
        let id = next_id;
        next_id += 1;

        new_files.push(InputFile {
            id,
            name,
            kind: describe_kind(extension.as_deref()),
            extension,
            size,
            path,
        });
    }

    let accepted = new_files.len();

    if append || accepted > 0 {
        if let Ok(mut counter) = state.next_id.lock() {
            *counter = next_id;
        }
        if append {
            if let Ok(mut stored_files) = state.files.lock() {
                stored_files.extend(new_files);
            }
        } else if let Ok(mut stored_files) = state.files.lock() {
            *stored_files = new_files;
        }
        if let Ok(mut stored_action) = state.action_id.lock() {
            *stored_action = Some(action_id.to_string());
        }
        if let Ok(mut output_path) = state.output_path.lock() {
            *output_path = None;
        }
        if let Ok(mut split_output_dir) = state.split_output_dir.lock() {
            *split_output_dir = None;
        }
        if let Ok(mut last_output) = state.last_output.lock() {
            *last_output = None;
        }
        if let Ok(mut last_output_directory) = state.last_output_directory.lock() {
            *last_output_directory = None;
        }
    }

    let mut selection = input_snapshot(state);
    selection.duplicates = duplicates;
    selection.limit_skipped = limit_skipped;
    selection.rejected = rejected;

    IngestOutcome {
        selection,
        accepted,
    }
}

fn extension_is_allowed(action_id: &str, extension: Option<&str>) -> bool {
    match filter_for_action(action_id) {
        Some((_, allowed)) => extension.is_some_and(|extension| allowed.contains(&extension)),
        None => true,
    }
}

fn handle_file_drop(app: &AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }

    let state = app.state::<InputState>();
    let action_id = state
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None);

    state.drop_mode.store(false, Ordering::SeqCst);

    let Some(action_id) = action_id else {
        if let Some(window) = app.get_webview_window(MENU_WINDOW_LABEL) {
            let _ = window.emit(
                "input-drag",
                DragState {
                    active: false,
                    count: 0,
                },
            );
        }
        return;
    };

    let outcome = ingest_paths(&state, &action_id, paths, true);

    note_menu_shown(app);
    focus_menu(app);

    if let Some(window) = app.get_webview_window(MENU_WINDOW_LABEL) {
        let _ = window.emit("input-updated", &outcome.selection);
        let _ = window.emit(
            "input-drag",
            DragState {
                active: false,
                count: 0,
            },
        );
    }
}

fn focus_menu(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if let Ok(panel) = menu_panel(app) {
        panel.show_and_make_key();
    }

    if let Some(window) = app.get_webview_window(MENU_WINDOW_LABEL) {
        #[cfg(target_os = "macos")]
        {
            let webview: &tauri::Webview = window.as_ref();
            let _ = webview.set_focus();
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = window.set_focus();
        }
    }
}

#[tauri::command]
fn remove_input_file(app: AppHandle, id: u64) -> Result<InputSelection, String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;

    if let Ok(mut files) = state.files.lock() {
        files.retain(|file| file.id != id);
    }
    if let Ok(mut output_path) = state.output_path.lock() {
        *output_path = None;
    }
    if let Ok(mut split_output_dir) = state.split_output_dir.lock() {
        *split_output_dir = None;
    }
    if let Ok(mut last_output) = state.last_output.lock() {
        *last_output = None;
    }
    if let Ok(mut last_output_directory) = state.last_output_directory.lock() {
        *last_output_directory = None;
    }

    Ok(input_snapshot(&state))
}

#[tauri::command]
fn clear_input(app: AppHandle) -> Result<(), String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;

    if let Ok(mut files) = state.files.lock() {
        files.clear();
    }
    if let Ok(mut output_path) = state.output_path.lock() {
        *output_path = None;
    }
    if let Ok(mut split_output_dir) = state.split_output_dir.lock() {
        *split_output_dir = None;
    }
    if let Ok(mut last_output) = state.last_output.lock() {
        *last_output = None;
    }
    if let Ok(mut last_output_directory) = state.last_output_directory.lock() {
        *last_output_directory = None;
    }

    Ok(())
}

fn input_snapshot(state: &InputState) -> InputSelection {
    let action_id = state
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None)
        .unwrap_or_default();
    let files = state
        .files
        .lock()
        .map(|files| files.clone())
        .unwrap_or_default();

    let limit = max_files_for_action(&action_id);
    InputSelection {
        action_id,
        files,
        limit,
        duplicates: 0,
        limit_skipped: 0,
        rejected: 0,
    }
}

fn clear_input_state(state: &InputState) {
    if state.conversion_in_progress.load(Ordering::SeqCst) {
        return;
    }

    if let Ok(mut files) = state.files.lock() {
        files.clear();
    }
    if let Ok(mut action_id) = state.action_id.lock() {
        *action_id = None;
    }
    if let Ok(mut output_path) = state.output_path.lock() {
        *output_path = None;
    }
    if let Ok(mut split_output_dir) = state.split_output_dir.lock() {
        *split_output_dir = None;
    }
    if let Ok(mut last_output) = state.last_output.lock() {
        *last_output = None;
    }
    if let Ok(mut last_output_directory) = state.last_output_directory.lock() {
        *last_output_directory = None;
    }
}

fn split_input_path(app: &AppHandle) -> Result<PathBuf, String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;

    let action_id = state
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None)
        .ok_or_else(|| "Choose an action first.".to_string())?;

    if action_id != "pdf.split" {
        return Err("This action is not available yet.".to_string());
    }

    let files = state
        .files
        .lock()
        .map(|files| files.clone())
        .unwrap_or_default();

    if files.len() != 1 {
        return Err("Select exactly one PDF to split.".to_string());
    }

    Ok(files[0].path.clone())
}

fn compress_input_path(app: &AppHandle) -> Result<PathBuf, String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;

    let action_id = state
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None)
        .ok_or_else(|| "Choose an action first.".to_string())?;

    if action_id != "pdf.compress" {
        return Err("This action is not available yet.".to_string());
    }

    let files = state
        .files
        .lock()
        .map(|files| files.clone())
        .unwrap_or_default();

    if files.len() != 1 {
        return Err("Select exactly one PDF to compress.".to_string());
    }

    Ok(files[0].path.clone())
}

fn max_files_for_action(action_id: &str) -> usize {
    if matches!(
        action_id,
        "pdf.split" | "pdf.compress" | "pdf.extract_pages" | "pdf.delete_pages"
    ) {
        1
    } else {
        MAX_INPUT_FILES
    }
}

fn action_input_kind(action_id: &str) -> Option<&'static str> {
    match action_id {
        "images.convert_to_pdf" => Some("images"),
        "pdf.merge" | "pdf.split" | "pdf.compress" | "pdf.extract_pages" | "pdf.delete_pages" => {
            Some("pdf")
        }
        _ => None,
    }
}

fn filter_for_action(action_id: &str) -> Option<(&'static str, &'static [&'static str])> {
    match action_input_kind(action_id) {
        Some("images") => Some((
            "Images",
            &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"],
        )),
        Some("pdf") => Some(("PDF documents", &["pdf"])),
        Some("documents") => Some((
            "Documents",
            &["pdf", "doc", "docx", "txt", "md", "markdown", "rtf", "odt"],
        )),
        _ => None,
    }
}

fn category_actions(category_id: &str) -> Vec<ActionInfo> {
    let action =
        |id: &str, label: &str, description: &str, enabled: bool, input: Option<&str>| ActionInfo {
            id: id.to_string(),
            label: label.to_string(),
            description: description.to_string(),
            enabled,
            input: input.map(str::to_string),
        };

    match category_id {
        "images" => vec![
            action(
                "images.convert_to_pdf",
                "Convert to PDF",
                "Turn images into a PDF document",
                true,
                Some("images"),
            ),
            action(
                "images.combine_to_pdf",
                "Combine images into one PDF",
                "Merge images into a single PDF",
                false,
                Some("images"),
            ),
            action(
                "images.convert_format",
                "Convert format",
                "PNG, JPEG, WebP, and more",
                false,
                Some("images"),
            ),
            action(
                "images.resize_compress",
                "Resize or compress",
                "Change dimensions or file size",
                false,
                Some("images"),
            ),
        ],
        "pdf" => vec![
            action(
                "pdf.merge",
                "Merge PDFs",
                "Combine PDF files into one document",
                true,
                Some("pdf"),
            ),
            action(
                "pdf.split",
                "Split PDF",
                "Split into separate PDFs or custom ranges",
                true,
                Some("pdf"),
            ),
            action(
                "pdf.compress",
                "Compress PDF",
                "Reduce PDF file size with lossless or image recompression",
                true,
                Some("pdf"),
            ),
            action(
                "pdf.extract_pages",
                "Extract Pages",
                "Keep only selected pages in a new PDF",
                true,
                Some("pdf"),
            ),
            action(
                "pdf.delete_pages",
                "Delete Pages",
                "Remove selected pages from a copy",
                true,
                Some("pdf"),
            ),
        ],
        "compress" => vec![
            action(
                "compress.files",
                "Compress files",
                "Reduce file size",
                false,
                None,
            ),
            action(
                "compress.extract",
                "Extract archive",
                "Unpack an archive",
                false,
                None,
            ),
        ],
        "documents" => vec![
            action(
                "documents.convert",
                "Convert format",
                "Convert document formats",
                false,
                None,
            ),
            action(
                "documents.extract_text",
                "Extract text",
                "Save text from a document",
                false,
                None,
            ),
        ],
        "tools" => vec![action(
            "tools.utilities",
            "Quick utilities",
            "More tools coming soon",
            false,
            None,
        )],
        "settings" => vec![
            action(
                "settings.preferences",
                "Preferences",
                "Appearance and behavior",
                false,
                None,
            ),
            action(
                "settings.shortcuts",
                "Shortcuts",
                "Change the global shortcut",
                false,
                None,
            ),
        ],
        _ => Vec::new(),
    }
}

fn all_actions() -> Vec<ActionInfo> {
    [
        "images",
        "pdf",
        "compress",
        "documents",
        "tools",
        "settings",
    ]
    .into_iter()
    .flat_map(category_actions)
    .collect()
}

fn find_action(action_id: &str) -> Option<ActionInfo> {
    all_actions()
        .into_iter()
        .find(|action| action.id == action_id)
}

fn describe_kind(extension: Option<&str>) -> String {
    match extension {
        Some("png") => "PNG image".to_string(),
        Some("jpg" | "jpeg") => "JPEG image".to_string(),
        Some("webp") => "WebP image".to_string(),
        Some("gif") => "GIF image".to_string(),
        Some("bmp") => "BMP image".to_string(),
        Some("tif" | "tiff") => "TIFF image".to_string(),
        Some("heic") => "HEIC image".to_string(),
        Some("avif") => "AVIF image".to_string(),
        Some("svg") => "SVG image".to_string(),
        Some("pdf") => "PDF document".to_string(),
        Some("doc" | "docx" | "odt" | "rtf") => "Document".to_string(),
        Some("txt" | "md" | "markdown") => "Text document".to_string(),
        Some(extension) => format!("{} file", extension.to_uppercase()),
        None => "File".to_string(),
    }
}

fn menu_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window(MENU_WINDOW_LABEL)
        .ok_or_else(|| format!("window `{MENU_WINDOW_LABEL}` was not found"))
}

#[cfg(target_os = "macos")]
fn menu_panel(app: &AppHandle) -> Result<tauri_nspanel::PanelHandle<tauri::Wry>, String> {
    use tauri_nspanel::ManagerExt;

    app.get_webview_panel(MENU_WINDOW_LABEL)
        .map_err(|error| format!("panel `{MENU_WINDOW_LABEL}` was not found: {error:?}"))
}

fn show_menu(app: &AppHandle, x: Option<f64>, y: Option<f64>) -> Result<(), String> {
    if app
        .state::<InputState>()
        .conversion_in_progress
        .load(Ordering::SeqCst)
    {
        return Ok(());
    }

    let window = menu_window(app)?;

    let cursor = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            let position = app.cursor_position().map_err(|error| error.to_string())?;
            (position.x, position.y)
        }
    };

    let monitor = app
        .monitor_from_point(cursor.0, cursor.1)
        .map_err(|error| error.to_string())?
        .or(app.primary_monitor().map_err(|error| error.to_string())?)
        .ok_or_else(|| "no monitor found for cursor position".to_string())?;

    let area = monitor.work_area();
    let scale_factor = monitor.scale_factor();
    let (logical_width, logical_height) = menu_logical_size(app);

    let window_size = (
        (logical_width * scale_factor).round() as u32,
        (logical_height * scale_factor).round() as u32,
    );
    let margin = (POSITION_MARGIN_LOGICAL * scale_factor).round() as i32;
    let position = compute_menu_position(
        cursor,
        window_size,
        (area.position.x, area.position.y),
        (area.size.width, area.size.height),
        margin,
    );

    note_menu_shown(app);
    clear_input_state(&app.state::<InputState>());

    window
        .set_position(PhysicalPosition::new(position.0, position.1))
        .map_err(|error| error.to_string())?;

    #[cfg(target_os = "macos")]
    {
        let panel = menu_panel(app)?;
        panel.show_and_make_key();

        let webview: &tauri::Webview = window.as_ref();
        let _ = webview.set_focus();
    }

    #[cfg(not(target_os = "macos"))]
    {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
    }

    eprintln!(
        "quicklet: menu shown at physical ({}, {})",
        position.0, position.1
    );
    window
        .emit("menu-opened", ())
        .map_err(|error| error.to_string())?;

    Ok(())
}

fn menu_logical_size(app: &AppHandle) -> (f64, f64) {
    app.config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MENU_WINDOW_LABEL)
        .map(|window| (window.width, window.height))
        .unwrap_or((DEFAULT_MENU_SIZE, DEFAULT_MENU_SIZE))
}

fn note_menu_shown(app: &AppHandle) {
    if let Ok(mut last_shown_at) = app.state::<MenuState>().last_shown_at.lock() {
        *last_shown_at = Some(Instant::now());
    }
}

fn compute_menu_position(
    cursor: (f64, f64),
    window_size: (u32, u32),
    area_position: (i32, i32),
    area_size: (u32, u32),
    margin: i32,
) -> (i32, i32) {
    let target_x = (cursor.0 - window_size.0 as f64 / 2.0).round() as i32;
    let target_y = (cursor.1 - window_size.1 as f64 / 2.0).round() as i32;

    let min_x = area_position.0 + margin;
    let min_y = area_position.1 + margin;
    let max_x = (area_position.0 + area_size.0 as i32 - window_size.0 as i32 - margin).max(min_x);
    let max_y = (area_position.1 + area_size.1 as i32 - window_size.1 as i32 - margin).max(min_y);

    (target_x.clamp(min_x, max_x), target_y.clamp(min_y, max_y))
}

fn toggle_menu(app: &AppHandle) -> Result<(), String> {
    let input_state = app.state::<InputState>();
    if input_state.dialog_open.load(Ordering::SeqCst)
        || input_state.conversion_in_progress.load(Ordering::SeqCst)
    {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let panel = menu_panel(app)?;
        if panel.is_visible() {
            app.state::<InputState>()
                .drop_mode
                .store(false, Ordering::SeqCst);
            panel.hide();
            Ok(())
        } else {
            show_menu(app, None, None)
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let window = menu_window(app)?;
        if window.is_visible().map_err(|error| error.to_string())? {
            app.state::<InputState>()
                .drop_mode
                .store(false, Ordering::SeqCst);
            window.hide().map_err(|error| error.to_string())
        } else {
            show_menu(app, None, None)
        }
    }
}

fn register_menu_shortcut(app: &tauri::App) {
    let result = app.global_shortcut().register(menu_shortcut());
    let state = app.state::<MenuState>();
    let mut status = state
        .shortcut
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    status.label = SHORTCUT_LABEL.to_string();
    match result {
        Ok(()) => {
            status.registered = true;
            status.error = None;
            eprintln!("quicklet: registered {SHORTCUT_LABEL}");
        }
        Err(error) => {
            status.registered = false;
            status.error = Some(error.to_string());
            eprintln!("quicklet: failed to register {SHORTCUT_LABEL}: {error}");
        }
    }
}

#[cfg(target_os = "macos")]
fn setup_menu_panel(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_nspanel::{CollectionBehavior, PanelLevel, StyleMask, WebviewWindowExt};

    app.handle()
        .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

    let window = app
        .get_webview_window(MENU_WINDOW_LABEL)
        .expect("menu window should be created from tauri.conf.json");
    let panel = window.to_panel::<QuickletPanel>()?;

    panel.set_level(PanelLevel::Floating.value());
    panel.add_style_mask(StyleMask::empty().nonactivating_panel().into())?;
    panel.set_collection_behavior(
        CollectionBehavior::new()
            .full_screen_auxiliary()
            .move_to_active_space()
            .into(),
    );
    panel.set_hides_on_deactivate(false);

    Ok(())
}

fn setup_menu_window(app: &tauri::App) -> tauri::Result<()> {
    let window = app
        .get_webview_window(MENU_WINDOW_LABEL)
        .expect("menu window should be created from tauri.conf.json");
    let handle = app.handle().clone();

    window.on_window_event(move |event| match event {
        WindowEvent::Focused(false) => {
            let state = handle.state::<MenuState>();
            let within_grace_period = state
                .last_shown_at
                .lock()
                .ok()
                .map(|last_shown_at| {
                    last_shown_at
                        .map(|shown_at| shown_at.elapsed() < BLUR_GRACE_PERIOD)
                        .unwrap_or(true)
                })
                .unwrap_or(true);
            let input_state = handle.state::<InputState>();
            let input_busy = input_state.dialog_open.load(Ordering::SeqCst)
                || input_state.drop_mode.load(Ordering::SeqCst)
                || input_state.conversion_in_progress.load(Ordering::SeqCst);

            if !input_busy && !within_grace_period {
                input_state.drop_mode.store(false, Ordering::SeqCst);
                clear_input_state(&input_state);

                #[cfg(target_os = "macos")]
                if let Ok(panel) = menu_panel(&handle) {
                    panel.hide();
                }

                #[cfg(not(target_os = "macos"))]
                if let Some(window) = handle.get_webview_window(MENU_WINDOW_LABEL) {
                    let _ = window.hide();
                }
            }
        }
        WindowEvent::DragDrop(drag_event) => {
            if !handle
                .state::<InputState>()
                .drop_mode
                .load(Ordering::SeqCst)
            {
                return;
            }

            match drag_event {
                tauri::DragDropEvent::Enter { paths, .. } => {
                    if let Some(window) = handle.get_webview_window(MENU_WINDOW_LABEL) {
                        let _ = window.emit(
                            "input-drag",
                            DragState {
                                active: true,
                                count: paths.len(),
                            },
                        );
                    }
                }
                tauri::DragDropEvent::Leave => {
                    if let Some(window) = handle.get_webview_window(MENU_WINDOW_LABEL) {
                        let _ = window.emit(
                            "input-drag",
                            DragState {
                                active: false,
                                count: 0,
                            },
                        );
                    }
                }
                tauri::DragDropEvent::Drop { paths, .. } => {
                    handle_file_drop(&handle, paths.to_vec());
                }
                _ => {}
            }
        }
        _ => {}
    });

    Ok(())
}

#[cfg(debug_assertions)]
fn create_dev_launcher(app: &tauri::App) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    WebviewWindowBuilder::new(app, "dev-launcher", WebviewUrl::App("index.html".into()))
        .title("Quicklet Dev")
        .inner_size(360.0, 330.0)
        .resizable(false)
        .build()?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .manage(MenuState::default())
        .manage(InputState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        eprintln!("quicklet: shortcut {SHORTCUT_LABEL} pressed");
                        if let Err(error) = toggle_menu(app) {
                            eprintln!("quicklet: failed to toggle menu: {error}");
                        }
                    }
                })
                .build(),
        );

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());

    builder
        .invoke_handler(tauri::generate_handler![
            open_menu,
            dismiss_menu,
            get_menu_status,
            pick_input_files,
            remove_input_file,
            clear_input,
            set_drop_mode,
            get_category_actions,
            start_action,
            get_input,
            reorder_input,
            choose_output,
            choose_output_directory,
            convert_to_pdf,
            merge_pdfs,
            inspect_split_source,
            plan_split,
            split_pdf,
            inspect_compress_source,
            compress_pdf,
            inspect_page_source,
            plan_page_selection,
            extract_pages,
            delete_pages,
            reveal_output,
            open_output,
            open_output_directory
        ])
        .setup(|app| {
            #[cfg(debug_assertions)]
            create_dev_launcher(app)?;

            #[cfg(target_os = "macos")]
            if let Err(error) = setup_menu_panel(app) {
                eprintln!("quicklet: failed to configure macOS panel: {error}");
            }

            setup_menu_window(app)?;
            register_menu_shortcut(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{InputState, compute_menu_position, ensure_not_converting};
    use std::sync::atomic::Ordering;

    const WINDOW: (u32, u32) = (440, 440);
    const MARGIN: i32 = 8;

    #[test]
    fn centers_on_cursor_when_space_is_available() {
        let position =
            compute_menu_position((1920.0, 1080.0), WINDOW, (0, 0), (3840, 2160), MARGIN);

        assert_eq!(position, (1700, 860));
    }

    #[test]
    fn clamps_near_top_left_work_area_edge() {
        let position = compute_menu_position((20.0, 20.0), WINDOW, (0, 0), (1920, 1080), MARGIN);

        assert_eq!(position, (8, 8));
    }

    #[test]
    fn clamps_near_bottom_right_work_area_edge() {
        let position =
            compute_menu_position((1910.0, 1070.0), WINDOW, (0, 0), (1920, 1080), MARGIN);

        assert_eq!(position, (1472, 632));
    }

    #[test]
    fn supports_negative_coordinates_on_secondary_monitors() {
        let position =
            compute_menu_position((-960.0, 540.0), WINDOW, (-1920, 0), (1920, 1080), MARGIN);

        assert_eq!(position, (-1180, 320));
    }

    #[test]
    fn clamps_to_work_area_origin_when_area_is_smaller_than_window() {
        let position = compute_menu_position((100.0, 100.0), WINDOW, (0, 0), (300, 300), MARGIN);

        assert_eq!(position, (8, 8));
    }

    #[test]
    fn applies_category_filters_only_where_defined() {
        let (images_name, images_extensions) =
            super::filter_for_action("images.convert_to_pdf").expect("images should have filters");
        assert_eq!(images_name, "Images");
        assert!(images_extensions.contains(&"png"));
        assert!(images_extensions.contains(&"webp"));

        let (pdf_name, pdf_extensions) =
            super::filter_for_action("pdf.merge").expect("PDF merge should have filters");
        assert_eq!(pdf_name, "PDF documents");
        assert_eq!(pdf_extensions, &["pdf"]);

        let (split_name, split_extensions) =
            super::filter_for_action("pdf.split").expect("PDF split should have filters");
        assert_eq!(split_name, "PDF documents");
        assert_eq!(split_extensions, &["pdf"]);

        let (compress_name, compress_extensions) =
            super::filter_for_action("pdf.compress").expect("PDF compress should have filters");
        assert_eq!(compress_name, "PDF documents");
        assert_eq!(compress_extensions, &["pdf"]);

        for action in ["pdf.extract_pages", "pdf.delete_pages"] {
            let (name, extensions) = super::filter_for_action(action).expect("PDF page action");
            assert_eq!(name, "PDF documents");
            assert_eq!(extensions, &["pdf"]);
        }

        assert!(super::filter_for_action("compress.files").is_none());
        assert!(super::filter_for_action("settings.preferences").is_none());
    }

    #[test]
    fn describes_common_file_kinds() {
        assert_eq!(super::describe_kind(Some("png")), "PNG image");
        assert_eq!(super::describe_kind(Some("jpeg")), "JPEG image");
        assert_eq!(super::describe_kind(Some("pdf")), "PDF document");
        assert_eq!(super::describe_kind(Some("xyz")), "XYZ file");
        assert_eq!(super::describe_kind(None), "File");
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "quicklet-test-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("test directory should be created");
        dir
    }

    #[test]
    fn enforces_extension_filters_for_actions() {
        assert!(super::extension_is_allowed(
            "images.convert_to_pdf",
            Some("png")
        ));
        assert!(!super::extension_is_allowed(
            "images.convert_to_pdf",
            Some("pdf")
        ));
        assert!(!super::extension_is_allowed("images.convert_to_pdf", None));
        assert!(super::filter_for_action("compress.files").is_none());
    }

    #[test]
    fn ingestion_rejects_unsupported_files_and_directories() {
        let dir = test_dir("reject");
        let image = dir.join("photo.png");
        let document = dir.join("document.pdf");
        let folder = dir.join("folder");
        std::fs::write(&image, b"x").expect("image should be written");
        std::fs::write(&document, b"x").expect("document should be written");
        std::fs::create_dir(&folder).expect("folder should be created");

        let state = super::InputState::default();
        let outcome = super::ingest_paths(
            &state,
            "images.convert_to_pdf",
            vec![image.clone(), document, folder],
            false,
        );

        assert_eq!(outcome.accepted, 1);
        assert_eq!(outcome.selection.rejected, 2);
        assert_eq!(outcome.selection.files.len(), 1);
        assert_eq!(outcome.selection.files[0].name, "photo.png");
        assert_eq!(outcome.selection.files[0].kind, "PNG image");

        std::fs::remove_dir_all(dir).expect("test directory should be removed");
    }

    #[test]
    fn busy_guard_rejects_duplicate_conversion_work() {
        let state = InputState::default();
        assert!(ensure_not_converting(&state).is_ok());

        state.conversion_in_progress.store(true, Ordering::SeqCst);
        assert!(matches!(
            ensure_not_converting(&state),
            Err(message) if message == "A conversion is already running."
        ));

        state.conversion_in_progress.store(false, Ordering::SeqCst);
        assert!(ensure_not_converting(&state).is_ok());
    }

    #[test]
    fn action_catalog_exposes_one_enabled_image_action() {
        let actions = super::category_actions("images");
        let enabled: Vec<_> = actions.iter().filter(|action| action.enabled).collect();

        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, "images.convert_to_pdf");
        assert!(actions.iter().any(|action| !action.enabled));
        assert!(super::find_action("images.convert_to_pdf").is_some());
        assert!(super::find_action("images.convert_format").is_some());

        let pdf_actions = super::category_actions("pdf");
        assert!(
            pdf_actions
                .iter()
                .any(|action| action.id == "pdf.merge" && action.enabled)
        );
        assert!(
            pdf_actions
                .iter()
                .any(|action| action.id == "pdf.split" && action.enabled)
        );
        assert!(
            pdf_actions
                .iter()
                .any(|action| action.id == "pdf.compress" && action.enabled)
        );
        for id in ["pdf.extract_pages", "pdf.delete_pages"] {
            assert!(
                pdf_actions
                    .iter()
                    .any(|action| action.id == id && action.enabled)
            );
        }
    }

    #[test]
    fn split_ingestion_allows_only_one_pdf() {
        let dir = test_dir("split-limit");
        let first = dir.join("first.pdf");
        let second = dir.join("second.pdf");
        std::fs::write(&first, b"x").expect("first PDF should be written");
        std::fs::write(&second, b"x").expect("second PDF should be written");

        let state = super::InputState::default();
        let outcome = super::ingest_paths(
            &state,
            "pdf.split",
            vec![first.clone(), second.clone()],
            false,
        );

        assert_eq!(outcome.accepted, 1);
        assert_eq!(outcome.selection.files.len(), 1);
        assert_eq!(outcome.selection.limit, 1);
        assert_eq!(outcome.selection.limit_skipped, 1);

        let compress_state = super::InputState::default();
        let compress_outcome =
            super::ingest_paths(
                &compress_state,
                "pdf.compress",
                vec![first.clone(), second.clone()],
                false,
            );
        assert_eq!(compress_outcome.accepted, 1);
        assert_eq!(compress_outcome.selection.limit, 1);
        assert_eq!(compress_outcome.selection.limit_skipped, 1);

        for action in ["pdf.extract_pages", "pdf.delete_pages"] {
            let state = super::InputState::default();
            let outcome = super::ingest_paths(
                &state,
                action,
                vec![first.clone(), second.clone()],
                false,
            );
            assert_eq!(outcome.accepted, 1);
            assert_eq!(outcome.selection.limit, 1);
            assert_eq!(outcome.selection.limit_skipped, 1);
        }

        std::fs::remove_dir_all(dir).expect("test directory should be removed");
    }

    #[test]
    fn normalizes_pdf_output_paths() {
        assert_eq!(
            super::ensure_pdf_extension(std::path::PathBuf::from("/tmp/out")),
            std::path::PathBuf::from("/tmp/out.pdf")
        );
        assert_eq!(
            super::ensure_pdf_extension(std::path::PathBuf::from("/tmp/out.PDF")),
            std::path::PathBuf::from("/tmp/out.PDF")
        );
    }

    #[test]
    fn ingestion_enforces_limit_and_deduplicates() {
        let dir = test_dir("limit");
        let mut paths = Vec::new();
        for index in 0..10 {
            let path = dir.join(format!("image-{index}.png"));
            std::fs::write(&path, b"x").expect("image should be written");
            paths.push(path);
        }

        let state = super::InputState::default();
        let outcome = super::ingest_paths(&state, "images.convert_to_pdf", paths.clone(), false);

        assert_eq!(outcome.accepted, super::MAX_INPUT_FILES);
        assert_eq!(outcome.selection.files.len(), super::MAX_INPUT_FILES);
        assert_eq!(outcome.selection.limit_skipped, 2);

        let dedupe_state = super::InputState::default();
        let first = super::ingest_paths(
            &dedupe_state,
            "images.convert_to_pdf",
            vec![paths[0].clone(), paths[1].clone()],
            false,
        );
        assert_eq!(first.accepted, 2);

        let second = super::ingest_paths(
            &dedupe_state,
            "images.convert_to_pdf",
            vec![paths[1].clone(), paths[2].clone()],
            true,
        );
        assert_eq!(second.selection.files.len(), 3);
        assert_eq!(second.selection.duplicates, 1);

        std::fs::remove_dir_all(dir).expect("test directory should be removed");
    }
}
