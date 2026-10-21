import { useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DevLauncher } from "./components/DevLauncher";
import { MenuWindow } from "./components/MenuWindow";
import {
  BROWSER_PREVIEW_CLASS,
  DEV_LAUNCHER_WINDOW_LABEL,
  IS_TAURI,
  MENU_WINDOW_LABEL,
} from "./lib/constants";
import "./App.css";

if (!IS_TAURI) {
  document.documentElement.classList.add(BROWSER_PREVIEW_CLASS);
}

function App() {
  const [windowLabel] = useState(() =>
    IS_TAURI ? getCurrentWindow().label : MENU_WINDOW_LABEL,
  );

  return windowLabel === DEV_LAUNCHER_WINDOW_LABEL ? (
    <DevLauncher />
  ) : (
    <MenuWindow />
  );
}

export default App;
