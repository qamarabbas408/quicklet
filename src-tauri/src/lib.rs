use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

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
    picker_open: AtomicBool,
    drop_mode: AtomicBool,
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
    skipped: usize,
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
    action_id: String,
    append: bool,
) -> Result<Option<InputSelection>, String> {
    if action_id == "settings" {
        return Err("Settings does not accept file input.".to_string());
    }

    if append {
        let current_count = app
            .state::<InputState>()
            .files
            .lock()
            .map(|files| files.len())
            .unwrap_or(0);
        if current_count >= MAX_INPUT_FILES {
            return Err(format!(
                "Maximum of {MAX_INPUT_FILES} files reached. Remove a file first."
            ));
        }
    }

    app.state::<InputState>()
        .picker_open
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

    let received = tauri::async_runtime::spawn_blocking(move || receiver.recv().ok().flatten()).await;

    app.state::<InputState>()
        .picker_open
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
    app.state::<InputState>()
        .drop_mode
        .store(active, Ordering::SeqCst);

    if !active {
        note_menu_shown(&app);
    }

    Ok(())
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
    let capacity = if append {
        MAX_INPUT_FILES.saturating_sub(existing.len())
    } else {
        MAX_INPUT_FILES
    };

    let mut next_id = state.next_id.lock().map(|id| *id).unwrap_or(0);
    let mut new_files: Vec<InputFile> = Vec::new();
    let mut skipped: usize = 0;
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
            skipped += 1;
            continue;
        }

        if new_files.len() >= capacity {
            skipped += 1;
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
    }

    let mut selection = input_snapshot(state);
    selection.skipped = skipped;
    selection.rejected = rejected;

    IngestOutcome { selection, accepted }
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
fn remove_input_file(app: AppHandle, id: u64) -> InputSelection {
    let state = app.state::<InputState>();

    if let Ok(mut files) = state.files.lock() {
        files.retain(|file| file.id != id);
    }

    input_snapshot(&state)
}

#[tauri::command]
fn clear_input(app: AppHandle) -> Result<(), String> {
    clear_input_state(&app.state::<InputState>());
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

    InputSelection {
        action_id,
        files,
        limit: MAX_INPUT_FILES,
        skipped: 0,
        rejected: 0,
    }
}

fn clear_input_state(state: &InputState) {
    if let Ok(mut files) = state.files.lock() {
        files.clear();
    }
    if let Ok(mut action_id) = state.action_id.lock() {
        *action_id = None;
    }
}

fn filter_for_action(action_id: &str) -> Option<(&'static str, &'static [&'static str])> {
    match action_id {
        "images" => Some((
            "Images",
            &[
                "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "heic", "avif",
            ],
        )),
        "pdf" => Some(("PDF documents", &["pdf"])),
        "documents" => Some((
            "Documents",
            &["pdf", "doc", "docx", "txt", "md", "markdown", "rtf", "odt"],
        )),
        _ => None,
    }
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
    let max_x =
        (area_position.0 + area_size.0 as i32 - window_size.0 as i32 - margin).max(min_x);
    let max_y =
        (area_position.1 + area_size.1 as i32 - window_size.1 as i32 - margin).max(min_y);

    (target_x.clamp(min_x, max_x), target_y.clamp(min_y, max_y))
}

fn toggle_menu(app: &AppHandle) -> Result<(), String> {
    if app
        .state::<InputState>()
        .picker_open
        .load(Ordering::SeqCst)
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
            let input_busy = input_state.picker_open.load(Ordering::SeqCst)
                || input_state.drop_mode.load(Ordering::SeqCst);

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

    WebviewWindowBuilder::new(
        app,
        "dev-launcher",
        WebviewUrl::App("index.html".into()),
    )
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
            set_drop_mode
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
    use super::compute_menu_position;

    const WINDOW: (u32, u32) = (440, 440);
    const MARGIN: i32 = 8;

    #[test]
    fn centers_on_cursor_when_space_is_available() {
        let position = compute_menu_position(
            (1920.0, 1080.0),
            WINDOW,
            (0, 0),
            (3840, 2160),
            MARGIN,
        );

        assert_eq!(position, (1700, 860));
    }

    #[test]
    fn clamps_near_top_left_work_area_edge() {
        let position =
            compute_menu_position((20.0, 20.0), WINDOW, (0, 0), (1920, 1080), MARGIN);

        assert_eq!(position, (8, 8));
    }

    #[test]
    fn clamps_near_bottom_right_work_area_edge() {
        let position = compute_menu_position(
            (1910.0, 1070.0),
            WINDOW,
            (0, 0),
            (1920, 1080),
            MARGIN,
        );

        assert_eq!(position, (1472, 632));
    }

    #[test]
    fn supports_negative_coordinates_on_secondary_monitors() {
        let position = compute_menu_position(
            (-960.0, 540.0),
            WINDOW,
            (-1920, 0),
            (1920, 1080),
            MARGIN,
        );

        assert_eq!(position, (-1180, 320));
    }

    #[test]
    fn clamps_to_work_area_origin_when_area_is_smaller_than_window() {
        let position =
            compute_menu_position((100.0, 100.0), WINDOW, (0, 0), (300, 300), MARGIN);

        assert_eq!(position, (8, 8));
    }

    #[test]
    fn applies_category_filters_only_where_defined() {
        let (images_name, images_extensions) =
            super::filter_for_action("images").expect("images should have filters");
        assert_eq!(images_name, "Images");
        assert!(images_extensions.contains(&"png"));
        assert!(images_extensions.contains(&"webp"));

        let (pdf_name, pdf_extensions) =
            super::filter_for_action("pdf").expect("pdf should have filters");
        assert_eq!(pdf_name, "PDF documents");
        assert_eq!(pdf_extensions, &["pdf"]);

        assert!(super::filter_for_action("compress").is_none());
        assert!(super::filter_for_action("settings").is_none());
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
        assert!(super::extension_is_allowed("images", Some("png")));
        assert!(!super::extension_is_allowed("images", Some("pdf")));
        assert!(!super::extension_is_allowed("images", None));
        assert!(super::extension_is_allowed("compress", Some("anything")));
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
            "images",
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
    fn ingestion_enforces_limit_and_deduplicates() {
        let dir = test_dir("limit");
        let mut paths = Vec::new();
        for index in 0..10 {
            let path = dir.join(format!("image-{index}.png"));
            std::fs::write(&path, b"x").expect("image should be written");
            paths.push(path);
        }

        let state = super::InputState::default();
        let outcome = super::ingest_paths(&state, "images", paths.clone(), false);

        assert_eq!(outcome.accepted, super::MAX_INPUT_FILES);
        assert_eq!(outcome.selection.files.len(), super::MAX_INPUT_FILES);
        assert_eq!(outcome.selection.skipped, 2);

        let dedupe_state = super::InputState::default();
        let first = super::ingest_paths(
            &dedupe_state,
            "images",
            vec![paths[0].clone(), paths[1].clone()],
            false,
        );
        assert_eq!(first.accepted, 2);

        let second = super::ingest_paths(
            &dedupe_state,
            "images",
            vec![paths[1].clone(), paths[2].clone()],
            true,
        );
        assert_eq!(second.selection.files.len(), 3);
        assert_eq!(second.selection.skipped, 1);

        std::fs::remove_dir_all(dir).expect("test directory should be removed");
    }
}
