pub mod compress;
pub mod image_compression;
pub mod image_conversion;
pub mod merge;
pub mod naming;
pub mod operation;
pub mod organize;
pub mod pdf;
pub mod pdf_preview;
pub mod progress;
pub mod rotate;
pub mod split;
#[cfg(test)]
pub(crate) mod test_images;

use chrono::Local;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use operation::{OperationTarget, run_operation};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<ConversionImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_compression: Option<ImageCompressionReport>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageCompressionReport {
    format: String,
    format_label: String,
    width: u32,
    height: u32,
    original_bytes: u64,
    output_bytes: u64,
    saved_bytes: u64,
    reduction_percent: f64,
    status: String,
    had_alpha: bool,
    jpeg_quality: Option<u8>,
    metadata: image_compression::ImageMetadataSummary,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionImage {
    format: String,
    format_label: String,
    width: u32,
    height: u32,
    had_alpha: bool,
    flattened: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageFormatOption {
    id: String,
    label: String,
    extension: String,
    lossy: bool,
    supports_alpha: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageCompressionSourceInfo {
    name: String,
    format: String,
    format_label: String,
    width: u32,
    height: u32,
    bytes: u64,
    has_alpha: bool,
    orientation: u8,
    metadata: image_compression::ImageMetadataSummary,
    default_jpeg_quality: u8,
    max_input_bytes: u64,
    max_dimension: u32,
    max_pixels: u64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageSourceInfo {
    name: String,
    format: String,
    format_label: String,
    width: u32,
    height: u32,
    bytes: u64,
    has_alpha: bool,
    orientation: u8,
    destinations: Vec<ImageFormatOption>,
    default_jpeg_quality: u8,
    max_input_bytes: u64,
    max_dimension: u32,
    max_pixels: u64,
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

const UNAVAILABLE_ACTION: &str = "This action is not available yet.";

fn selected_action(state: &InputState) -> Result<String, String> {
    state
        .action_id
        .lock()
        .map(|action_id| action_id.clone())
        .unwrap_or(None)
        .ok_or_else(|| "Choose an action first.".to_string())
}

fn selected_files(state: &InputState) -> Vec<InputFile> {
    state
        .files
        .lock()
        .map(|files| files.clone())
        .unwrap_or_default()
}

fn selected_output(state: &InputState) -> Result<PathBuf, String> {
    state
        .output_path
        .lock()
        .map(|output| output.clone())
        .unwrap_or(None)
        .ok_or_else(|| "Choose an output destination first.".to_string())
}

/// Returns the selected files when `action_id` is the active action and no
/// conversion is running.
fn action_files(app: &AppHandle, action_id: &str) -> Result<Vec<InputFile>, String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;
    if selected_action(&state)? != action_id {
        return Err(UNAVAILABLE_ACTION.to_string());
    }
    Ok(selected_files(&state))
}

fn single_input_path(app: &AppHandle, action_id: &str, count_error: &str) -> Result<PathBuf, String> {
    match action_files(app, action_id)?.as_slice() {
        [file] => Ok(file.path.clone()),
        _ => Err(count_error.to_string()),
    }
}

fn input_paths(files: &[InputFile]) -> Vec<PathBuf> {
    files.iter().map(|file| file.path.clone()).collect()
}

fn file_name_and_directory(path: &Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output.pdf".to_string());
    let directory = path
        .parent()
        .map(|directory| directory.display().to_string())
        .unwrap_or_default();
    (name, directory)
}

fn file_result(output: &Path, pages: usize, bytes_written: u64) -> ConversionResult {
    let (name, directory) = file_name_and_directory(output);
    ConversionResult {
        pages,
        bytes_written,
        name,
        directory,
        files: None,
        compression: None,
        image: None,
        image_compression: None,
    }
}

/// Forgets the chosen destination and the last result.
fn clear_outputs(state: &InputState) {
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
struct RotateSourceInfo {
    name: String,
    pages: usize,
    bytes: u64,
    rotations: Vec<u16>,
    has_signature: bool,
    max_pages: usize,
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
fn keep_menu_alive(app: AppHandle) -> Result<(), String> {
    note_menu_shown(&app);
    Ok(())
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
    format: Option<String>,
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

    let (title, filter_label, filter_extensions, extension, default_name): (
        &str,
        &str,
        Vec<&str>,
        String,
        String,
    ) = if action_id == "images.convert_format" {
        let format = format
            .as_deref()
            .and_then(image_conversion::ImageFormat::parse)
            .ok_or_else(|| "Choose an output format first.".to_string())?;
        let extension = format.extension();
        (
            "Save image",
            format.label(),
            vec![extension],
            extension.to_string(),
            naming::image_output_name(&files[0].path, extension, Local::now()),
        )
    } else if action_id == "images.compress" {
        let (label, extensions, extension) =
            compression_output_spec(files[0].extension.as_deref()).ok_or_else(|| {
                "Compression supports PNG, JPEG, and WebP images only.".to_string()
            })?;
        (
            "Save compressed image",
            label,
            extensions,
            extension.to_string(),
            naming::image_compression_output_name(&files[0].path, extension, Local::now()),
        )
    } else {
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
        let default_name =
            naming::default_output_name(&action_id, compression_preset, Local::now())
                .ok_or_else(|| "This action does not use a single output file.".to_string())?;
        ("Save PDF", "PDF", vec!["pdf"], "pdf".to_string(), default_name)
    };

    state.dialog_open.store(true, Ordering::SeqCst);

    let dialog = app
        .dialog()
        .file()
        .set_title(title)
        .set_parent(&window)
        .add_filter(filter_label, &filter_extensions)
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

    let path = ensure_output_path(
        file_path.into_path().map_err(|error| error.to_string())?,
        &extension,
    )?;
    let (name, directory) = file_name_and_directory(&path);

    clear_outputs(&state);
    if let Ok(mut output_path) = state.output_path.lock() {
        *output_path = Some(path);
    }

    Ok(Some(OutputInfo { name, directory }))
}

#[tauri::command]
async fn convert_to_pdf(app: AppHandle) -> Result<ConversionResult, String> {
    let files = action_files(&app, "images.convert_to_pdf")?;
    if files.is_empty() {
        return Err("Add images before converting.".to_string());
    }
    let output = selected_output(&app.state::<InputState>())?;

    let paths = input_paths(&files);
    let task_output = output.clone();
    let outcome = run_operation(
        &app,
        "images.convert_to_pdf",
        "Conversion",
        OperationTarget::File(output.clone()),
        move |progress| pdf::convert_images_to_pdf_with_progress(&paths, &task_output, true, progress),
    )
    .await?;

    Ok(file_result(&output, outcome.pages, outcome.bytes_written))
}

#[tauri::command]
async fn merge_pdfs(app: AppHandle) -> Result<ConversionResult, String> {
    let files = action_files(&app, "pdf.merge")?;
    if files.len() < 2 {
        return Err("Select at least two PDF files to merge.".to_string());
    }
    let output = selected_output(&app.state::<InputState>())?;

    let paths = input_paths(&files);
    let task_output = output.clone();
    let outcome = run_operation(
        &app,
        "pdf.merge",
        "Merge",
        OperationTarget::File(output.clone()),
        move |progress| merge::merge_pdfs_with_overwrite_and_progress(&paths, &task_output, progress),
    )
    .await?;

    Ok(file_result(&output, outcome.pages, outcome.bytes_written))
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

    clear_outputs(&state);
    if let Ok(mut split_output_dir) = state.split_output_dir.lock() {
        *split_output_dir = Some(path.clone());
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
    let path = split_input_path(&app)?;
    let output_directory = app
        .state::<InputState>()
        .split_output_dir
        .lock()
        .map(|directory| directory.clone())
        .unwrap_or(None)
        .ok_or_else(|| "Choose an output folder first.".to_string())?;
    let mode =
        split::SplitMode::parse(&mode).ok_or_else(|| "Choose a valid split mode.".to_string())?;

    let task_directory = output_directory.clone();
    let outcome = run_operation(
        &app,
        "pdf.split",
        "Split",
        OperationTarget::Directory(output_directory.clone()),
        move |progress| split::split_pdf_with_progress(&path, &task_directory, mode, &ranges, progress),
    )
    .await?;

    let Some(first_file) = outcome.files.first() else {
        return Err("No output files were created.".to_string());
    };
    if let Ok(mut last_output) = app.state::<InputState>().last_output.lock() {
        *last_output = Some(output_directory.join(&first_file.name));
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
        image: None,
        image_compression: None,
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
    let path = compress_input_path(&app)?;
    let output = selected_output(&app.state::<InputState>())?;
    let preset = compress::CompressionPreset::parse(&preset)
        .ok_or_else(|| "Choose a valid compression preset.".to_string())?;

    let task_output = output.clone();
    let outcome = run_operation(
        &app,
        "pdf.compress",
        "Compression",
        OperationTarget::File(output.clone()),
        move |reporter| {
            let cancel = reporter.cancel_token();
            compress::compress_pdf_with_overwrite_and_progress_cancelable(
                &path,
                &task_output,
                preset,
                &cancel,
                |stage, completed, total| {
                    // The final stages report a single step each, so a bar
                    // would jump straight to 100%.
                    if matches!(stage, "saving" | "validating" | "finalizing") {
                        reporter.report(stage, 0, 0);
                    } else {
                        reporter.report(stage, completed, total);
                    }
                },
            )
        },
    )
    .await?;

    let status = match outcome.status {
        compress::CompressionStatus::Reduced => "reduced",
        compress::CompressionStatus::NoReduction => "no-reduction",
    };

    Ok(ConversionResult {
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
        ..file_result(&output, outcome.pages, outcome.output_bytes)
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
        return Err(UNAVAILABLE_ACTION.to_string());
    }
    let output = selected_output(&app.state::<InputState>())?;
    let (kind, title) = match mode {
        organize::PageSelectionMode::Extract => ("pdf.extract_pages", "Page extraction"),
        organize::PageSelectionMode::Delete => ("pdf.delete_pages", "Page deletion"),
    };

    let task_output = output.clone();
    let outcome = run_operation(
        &app,
        kind,
        title,
        OperationTarget::File(output.clone()),
        move |progress| {
            organize::write_selection(&path, &task_output, &selection, mode, true, progress)
        },
    )
    .await?;

    Ok(file_result(&output, outcome.pages, outcome.bytes_written))
}

fn page_selection_input_path(app: &AppHandle) -> Result<(String, PathBuf), String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;

    let action_id = selected_action(&state)?;
    if organize::PageSelectionMode::from_action_id(&action_id).is_none() {
        return Err(UNAVAILABLE_ACTION.to_string());
    }

    match selected_files(&state).as_slice() {
        [file] => Ok((action_id, file.path.clone())),
        _ => Err("Select exactly one PDF.".to_string()),
    }
}

#[tauri::command]
async fn render_pdf_thumbnails(
    app: AppHandle,
    requests: Vec<pdf_preview::ThumbnailRequest>,
    target_width: u32,
    target_height: u32,
) -> Result<pdf_preview::PreviewBatch, String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;
    let files = state
        .files
        .lock()
        .map(|files| files.clone())
        .unwrap_or_default();

    if files.len() != 1 {
        return Err("Select exactly one PDF to preview.".to_string());
    }

    let path = files[0].path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        pdf_preview::render_thumbnails(&path, &requests, target_width, target_height)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn inspect_image_source(app: AppHandle) -> Result<ImageSourceInfo, String> {
    let path = single_input_path(&app, "images.convert_format", "Select exactly one image.")?;
    let task_path = path.clone();

    let source =
        tauri::async_runtime::spawn_blocking(move || image_conversion::inspect_image(&task_path))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;

    Ok(ImageSourceInfo {
        name: file_name_and_directory(&path).0,
        format: source.format.id().to_string(),
        format_label: source.format.label().to_string(),
        width: source.width,
        height: source.height,
        bytes: source.bytes,
        has_alpha: source.has_alpha,
        orientation: source.orientation,
        destinations: source
            .format
            .destinations()
            .into_iter()
            .map(|format| ImageFormatOption {
                id: format.id().to_string(),
                label: format.label().to_string(),
                extension: format.extension().to_string(),
                lossy: format.is_lossy(),
                supports_alpha: format.supports_alpha(),
            })
            .collect(),
        default_jpeg_quality: image_conversion::DEFAULT_JPEG_QUALITY,
        max_input_bytes: image_conversion::MAX_INPUT_BYTES,
        max_dimension: image_conversion::MAX_DIMENSION,
        max_pixels: image_conversion::MAX_PIXELS,
    })
}

#[tauri::command]
async fn convert_image(
    app: AppHandle,
    format: String,
    quality: Option<u8>,
    background: Option<String>,
) -> Result<ConversionResult, String> {
    let path = single_input_path(&app, "images.convert_format", "Select exactly one image.")?;
    let output = selected_output(&app.state::<InputState>())?;
    let format = image_conversion::ImageFormat::parse(&format)
        .ok_or_else(|| "Choose a valid output format.".to_string())?;
    let mut options = image_conversion::ConversionOptions::new(format);
    if let Some(quality) = quality {
        options.jpeg_quality = quality;
    }
    if let Some(background) = background {
        options.background = image_conversion::parse_background(&background)
            .ok_or_else(|| "Choose a valid background colour.".to_string())?;
    }

    let task_output = output.clone();
    let outcome = run_operation(
        &app,
        "images.convert_format",
        "Image conversion",
        OperationTarget::File(output.clone()),
        move |progress| {
            image_conversion::convert_image_with_progress(
                &path,
                &task_output,
                options,
                true,
                progress,
            )
        },
    )
    .await?;

    Ok(ConversionResult {
        image: Some(ConversionImage {
            format: outcome.format.id().to_string(),
            format_label: outcome.format.label().to_string(),
            width: outcome.width,
            height: outcome.height,
            had_alpha: outcome.had_alpha,
            flattened: outcome.flattened,
        }),
        ..file_result(&output, 1, outcome.bytes_written)
    })
}

#[tauri::command]
async fn inspect_image_compression_source(
    app: AppHandle,
) -> Result<ImageCompressionSourceInfo, String> {
    let path = single_input_path(&app, "images.compress", "Select exactly one image.")?;
    let task_path = path.clone();

    let source =
        tauri::async_runtime::spawn_blocking(move || image_compression::inspect_image(&task_path))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;

    Ok(ImageCompressionSourceInfo {
        name: file_name_and_directory(&path).0,
        format: source.format.id().to_string(),
        format_label: source.format.label().to_string(),
        width: source.width,
        height: source.height,
        bytes: source.bytes,
        has_alpha: source.has_alpha,
        orientation: source.orientation,
        metadata: source.metadata,
        default_jpeg_quality: image_compression::DEFAULT_COMPRESSION_JPEG_QUALITY,
        max_input_bytes: image_conversion::MAX_INPUT_BYTES,
        max_dimension: image_conversion::MAX_DIMENSION,
        max_pixels: image_conversion::MAX_PIXELS,
    })
}

#[tauri::command]
async fn compress_image(app: AppHandle, quality: Option<u8>) -> Result<ConversionResult, String> {
    let path = single_input_path(&app, "images.compress", "Select exactly one image.")?;
    let output = selected_output(&app.state::<InputState>())?;
    let mut options = image_compression::CompressionOptions::new();
    if let Some(quality) = quality {
        options.jpeg_quality = quality;
    }
    let jpeg_quality = options.jpeg_quality;

    let task_output = output.clone();
    let outcome = run_operation(
        &app,
        "images.compress",
        "Image compression",
        OperationTarget::File(output.clone()),
        move |progress| {
            image_compression::compress_image_with_progress(
                &path,
                &task_output,
                options,
                true,
                progress,
            )
        },
    )
    .await?;

    let status = match outcome.status {
        image_compression::CompressionStatus::Reduced => "reduced",
        image_compression::CompressionStatus::NoReduction => "no-reduction",
    };

    Ok(ConversionResult {
        image_compression: Some(ImageCompressionReport {
            format: outcome.format.id().to_string(),
            format_label: outcome.format.label().to_string(),
            width: outcome.width,
            height: outcome.height,
            original_bytes: outcome.original_bytes,
            output_bytes: outcome.output_bytes,
            saved_bytes: outcome.saved_bytes,
            reduction_percent: outcome.reduction_percent,
            status: status.to_string(),
            had_alpha: outcome.had_alpha,
            jpeg_quality: (outcome.format == image_conversion::ImageFormat::Jpeg)
                .then_some(jpeg_quality),
            metadata: outcome.metadata,
        }),
        ..file_result(&output, 1, outcome.output_bytes)
    })
}

#[tauri::command]
async fn inspect_rotate_source(app: AppHandle) -> Result<RotateSourceInfo, String> {
    let path = single_input_path(&app, "pdf.rotate_pages", "Select exactly one PDF.")?;
    let task_path = path.clone();

    let source = tauri::async_runtime::spawn_blocking(move || rotate::inspect_pdf(&task_path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.pdf".to_string());

    Ok(RotateSourceInfo {
        name,
        pages: source.pages,
        bytes: source.bytes,
        rotations: source.rotations,
        has_signature: source.has_signature,
        max_pages: rotate::MAX_ROTATE_PAGES,
        max_input_bytes: rotate::MAX_ROTATE_INPUT_BYTES,
    })
}

#[tauri::command]
async fn rotate_pages(
    app: AppHandle,
    rotations: Vec<rotate::PageRotation>,
) -> Result<ConversionResult, String> {
    let path = single_input_path(&app, "pdf.rotate_pages", "Select exactly one PDF to rotate.")?;
    let output = selected_output(&app.state::<InputState>())?;

    let task_output = output.clone();
    let outcome = run_operation(
        &app,
        "pdf.rotate_pages",
        "Rotation",
        OperationTarget::File(output.clone()),
        move |progress| {
            rotate::rotate_pdf_with_overwrite_and_progress(&path, &task_output, &rotations, progress)
        },
    )
    .await?;

    Ok(file_result(&output, outcome.pages, outcome.bytes_written))
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

/// Maps a validated source extension to the compression save-dialog label,
/// filter extensions, and canonical output extension.
fn compression_output_spec(
    source_extension: Option<&str>,
) -> Option<(&'static str, Vec<&'static str>, &'static str)> {
    match source_extension {
        Some("jpg") => Some(("JPEG", vec!["jpg", "jpeg"], "jpg")),
        Some("jpeg") => Some(("JPEG", vec!["jpg", "jpeg"], "jpeg")),
        Some("png") => Some(("PNG", vec!["png"], "png")),
        Some("webp") => Some(("WebP", vec!["webp"], "webp")),
        _ => None,
    }
}

/// Normalises the save-dialog path and prevents a corrected extension from
/// silently overwriting a different existing file that the dialog did not ask
/// about.
fn ensure_output_path(path: PathBuf, extension: &str) -> Result<PathBuf, String> {
    let normalized = ensure_extension(path.clone(), extension);
    if normalized != path && normalized.exists() {
        let name = normalized
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| normalized.display().to_string());
        return Err(format!(
            "A file named \"{name}\" already exists. Choose a different name or folder."
        ));
    }
    Ok(normalized)
}

/// Adds `extension` unless the path already has it (`jpg` also accepts `jpeg`).
fn ensure_extension(path: PathBuf, extension: &str) -> PathBuf {
    let current = path
        .extension()
        .and_then(|current| current.to_str())
        .map(str::to_ascii_lowercase);
    let matches = current.as_deref().is_some_and(|current| {
        current == extension || (extension == "jpg" && current == "jpeg")
    });

    if matches {
        path
    } else {
        path.with_extension(extension)
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
        clear_outputs(state);
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
    clear_outputs(&state);

    Ok(input_snapshot(&state))
}

#[tauri::command]
fn clear_input(app: AppHandle) -> Result<(), String> {
    let state = app.state::<InputState>();
    ensure_not_converting(&state)?;

    if let Ok(mut files) = state.files.lock() {
        files.clear();
    }
    clear_outputs(&state);

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
    clear_outputs(state);
}

fn split_input_path(app: &AppHandle) -> Result<PathBuf, String> {
    single_input_path(app, "pdf.split", "Select exactly one PDF to split.")
}

fn compress_input_path(app: &AppHandle) -> Result<PathBuf, String> {
    single_input_path(app, "pdf.compress", "Select exactly one PDF to compress.")
}

fn max_files_for_action(action_id: &str) -> usize {
    if matches!(
        action_id,
        "images.convert_format"
            | "images.compress"
            | "pdf.split"
            | "pdf.compress"
            | "pdf.extract_pages"
            | "pdf.delete_pages"
            | "pdf.rotate_pages"
    ) {
        1
    } else {
        MAX_INPUT_FILES
    }
}

fn action_input_kind(action_id: &str) -> Option<&'static str> {
    match action_id {
        "images.convert_to_pdf" => Some("images"),
        "images.convert_format" | "images.compress" => Some("image"),
        "pdf.merge" | "pdf.split" | "pdf.compress" | "pdf.extract_pages" | "pdf.delete_pages"
        | "pdf.rotate_pages" => Some("pdf"),
        _ => None,
    }
}

fn filter_for_action(action_id: &str) -> Option<(&'static str, &'static [&'static str])> {
    match action_input_kind(action_id) {
        Some("images") => Some((
            "Images",
            &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"],
        )),
        Some("image") => Some(("Images", &["png", "jpg", "jpeg", "webp"])),
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
                "Convert Image",
                "Convert between PNG, JPEG, and WebP",
                true,
                Some("image"),
            ),
            action(
                "images.compress",
                "Compress Image",
                "Reduce file size without changing format or dimensions",
                true,
                Some("image"),
            ),
            action(
                "images.resize_compress",
                "Resize Image",
                "Change image dimensions",
                false,
                Some("image"),
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
            action(
                "pdf.rotate_pages",
                "Rotate Pages",
                "Preview and rotate selected pages",
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
                "settings.about",
                "About Quicklet",
                "Version and product information",
                true,
                None,
            ),
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

fn should_hide_on_blur(last_shown_at: Option<Instant>, input_busy: bool) -> bool {
    !input_busy && last_shown_at.is_none_or(|shown_at| shown_at.elapsed() >= BLUR_GRACE_PERIOD)
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
            let last_shown_at = state
                .last_shown_at
                .lock()
                .ok()
                .and_then(|last_shown_at| *last_shown_at);
            let input_state = handle.state::<InputState>();
            let input_busy = input_state.dialog_open.load(Ordering::SeqCst)
                || input_state.drop_mode.load(Ordering::SeqCst)
                || input_state.conversion_in_progress.load(Ordering::SeqCst);

            if should_hide_on_blur(last_shown_at, input_busy) {
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
        .manage(operation::OperationProgressState::default())
        .manage(operation::OperationCancellationState::default())
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
            keep_menu_alive,
            operation::get_operation_progress,
            operation::cancel_operation,
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
            inspect_image_source,
            convert_image,
            inspect_image_compression_source,
            compress_image,
            inspect_rotate_source,
            rotate_pages,
            render_pdf_thumbnails,
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
    use super::{
        BLUR_GRACE_PERIOD, InputState, compute_menu_position, ensure_not_converting,
        should_hide_on_blur,
    };
    use std::sync::atomic::Ordering;
    use std::time::Instant;

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
    fn keeps_menu_open_during_interaction_grace_period() {
        let last_shown_at = Some(Instant::now());

        assert!(!should_hide_on_blur(last_shown_at, false));
    }

    #[test]
    fn hides_menu_after_grace_period_expires() {
        let last_shown_at = Some(
            Instant::now() - (BLUR_GRACE_PERIOD + std::time::Duration::from_millis(1)),
        );

        assert!(should_hide_on_blur(last_shown_at, false));
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

        for action in ["pdf.extract_pages", "pdf.delete_pages", "pdf.rotate_pages"] {
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
    fn ensure_extension_adds_only_when_missing() {
        use std::path::PathBuf;

        let fix = |name: &str, extension: &str| {
            super::ensure_extension(PathBuf::from(name), extension)
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(fix("photo", "jpg"), "photo.jpg");
        assert_eq!(fix("photo.JPEG", "jpg"), "photo.JPEG");
        assert_eq!(fix("photo.png", "jpg"), "photo.jpg");
        assert_eq!(fix("scan.PDF", "pdf"), "scan.PDF");
        assert_eq!(fix("scan.txt", "pdf"), "scan.pdf");
    }

    #[test]
    fn compression_output_spec_preserves_format_and_extension() {
        assert_eq!(
            super::compression_output_spec(Some("jpg")),
            Some(("JPEG", vec!["jpg", "jpeg"], "jpg"))
        );
        assert_eq!(
            super::compression_output_spec(Some("jpeg")),
            Some(("JPEG", vec!["jpg", "jpeg"], "jpeg"))
        );
        assert_eq!(
            super::compression_output_spec(Some("png")),
            Some(("PNG", vec!["png"], "png"))
        );
        assert_eq!(
            super::compression_output_spec(Some("webp")),
            Some(("WebP", vec!["webp"], "webp"))
        );
        assert_eq!(super::compression_output_spec(Some("gif")), None);
        assert_eq!(super::compression_output_spec(None), None);
    }

    #[test]
    fn corrected_output_extension_does_not_overwrite_an_unconfirmed_file() {
        let dir = test_dir("output-extension-collision");
        let picked = dir.join("photo.png");
        let normalized_existing = dir.join("photo.jpg");
        std::fs::write(&normalized_existing, b"existing").expect("existing output");

        assert!(super::ensure_output_path(picked, "jpg").is_err());

        let fresh = super::ensure_output_path(dir.join("fresh.png"), "jpg")
            .expect("fresh output should be accepted");
        assert_eq!(fresh, dir.join("fresh.jpg"));

        std::fs::remove_dir_all(dir).expect("test directory should be removed");
    }

    #[test]
    fn action_catalog_exposes_enabled_image_actions() {
        let actions = super::category_actions("images");
        let enabled: Vec<_> = actions
            .iter()
            .filter(|action| action.enabled)
            .map(|action| action.id.as_str())
            .collect();

        assert_eq!(
            enabled,
            [
                "images.convert_to_pdf",
                "images.convert_format",
                "images.compress"
            ]
        );
        assert!(actions.iter().any(|action| !action.enabled));
        for id in ["images.convert_format", "images.compress"] {
            assert_eq!(super::max_files_for_action(id), 1);
            assert_eq!(
                super::filter_for_action(id),
                Some(("Images", &["png", "jpg", "jpeg", "webp"][..]))
            );
        }

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
        for id in ["pdf.extract_pages", "pdf.delete_pages", "pdf.rotate_pages"] {
            assert!(
                pdf_actions
                    .iter()
                    .any(|action| action.id == id && action.enabled)
            );
        }

        let settings_actions = super::category_actions("settings");
        assert!(
            settings_actions
                .iter()
                .any(|action| action.id == "settings.about" && action.enabled)
        );
        assert!(
            settings_actions
                .iter()
                .any(|action| action.id == "settings.preferences" && !action.enabled)
        );
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

        for action in ["pdf.extract_pages", "pdf.delete_pages", "pdf.rotate_pages"] {
            let state = super::InputState::default();
            let outcome =
                super::ingest_paths(&state, action, vec![first.clone(), second.clone()], false);
            assert_eq!(outcome.accepted, 1);
            assert_eq!(outcome.selection.limit, 1);
            assert_eq!(outcome.selection.limit_skipped, 1);
        }

        std::fs::remove_dir_all(dir).expect("test directory should be removed");
    }

    #[test]
    fn normalizes_pdf_output_paths() {
        assert_eq!(
            super::ensure_extension(std::path::PathBuf::from("/tmp/out"), "pdf"),
            std::path::PathBuf::from("/tmp/out.pdf")
        );
        assert_eq!(
            super::ensure_extension(std::path::PathBuf::from("/tmp/out.PDF"), "pdf"),
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
