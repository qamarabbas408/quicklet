import { isTauri } from "@tauri-apps/api/core";

export const APP_NAME = "Quicklet";

export const MENU_WINDOW_LABEL = "main";
export const DEV_LAUNCHER_WINDOW_LABEL = "dev-launcher";
export const BROWSER_PREVIEW_CLASS = "browser-preview";

export const IS_DEV = import.meta.env.DEV;
export const IS_PROD = import.meta.env.PROD;

export const IS_TAURI = isTauri();

// The authoritative input limit lives in Rust and is returned with each
// selection (`selection.limit`), so the UI never drifts from the backend.
