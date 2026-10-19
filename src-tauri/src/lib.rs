use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const MENU_WINDOW_LABEL: &str = "main";
const DEFAULT_MENU_SIZE: f64 = 440.0;
const POSITION_MARGIN_LOGICAL: f64 = 8.0;
const BLUR_GRACE_PERIOD: Duration = Duration::from_millis(350);

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

    if let Ok(mut last_shown_at) = app.state::<MenuState>().last_shown_at.lock() {
        *last_shown_at = Some(Instant::now());
    }

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
    #[cfg(target_os = "macos")]
    {
        let panel = menu_panel(app)?;
        if panel.is_visible() {
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

    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
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

            if !within_grace_period {
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
        .plugin(tauri_plugin_opener::init())
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
            get_menu_status
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
}
