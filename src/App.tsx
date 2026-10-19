import { useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DevLauncher } from "./components/DevLauncher";
import { MenuWindow } from "./components/MenuWindow";
import "./App.css";

if (!isTauri()) {
  document.documentElement.classList.add("browser-preview");
}

function App() {
  const [windowLabel] = useState(() =>
    isTauri() ? getCurrentWindow().label : "main",
  );

  return windowLabel === "dev-launcher" ? <DevLauncher /> : <MenuWindow />;
}

export default App;
