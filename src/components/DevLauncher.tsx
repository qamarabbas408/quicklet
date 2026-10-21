import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { APP_NAME } from "../lib/constants";
import { QuickletMark } from "./icons";

type MenuStatus = {
  registered: boolean;
  label: string;
  error: string | null;
};

export function DevLauncher() {
  const [x, setX] = useState("");
  const [y, setY] = useState("");
  const [menuStatus, setMenuStatus] = useState<MenuStatus | null>(null);
  const [message, setMessage] = useState(
    "Use the global shortcut or open the menu manually below.",
  );
  const [isError, setIsError] = useState(false);

  useEffect(() => {
    let active = true;

    void invoke<MenuStatus>("get_menu_status")
      .then((status) => {
        if (active) setMenuStatus(status);
      })
      .catch((error) => {
        if (active) {
          setIsError(true);
          setMessage(`Could not read shortcut status: ${String(error)}`);
        }
      });

    return () => {
      active = false;
    };
  }, []);

  const openMenu = async (px: number | null, py: number | null) => {
    try {
      await invoke("open_menu", { x: px, y: py });
      setIsError(false);
      setMessage(
        px === null ? "Opened at the cursor." : `Opened at ${px}, ${py}.`,
      );
    } catch (error) {
      setIsError(true);
      setMessage(`Could not open menu: ${String(error)}`);
    }
  };

  const openAtCoordinates = () => {
    const parsedX = Number.parseFloat(x);
    const parsedY = Number.parseFloat(y);

    if (!Number.isFinite(parsedX) || !Number.isFinite(parsedY)) {
      setIsError(true);
      setMessage("Enter valid physical screen coordinates.");
      return;
    }

    void openMenu(parsedX, parsedY);
  };

  return (
    <main className="dev-launcher-shell">
      <header className="flex items-center gap-2">
        <span className="flex h-8 w-8 items-center justify-center rounded-full bg-white/10 text-white">
          <QuickletMark className="h-4 w-4" />
        </span>
        <div className="leading-tight">
          <h1 className="text-sm font-semibold text-white">{APP_NAME} Dev</h1>
          <p className="text-[11px] text-white/40">M2.3 shortcut harness</p>
        </div>
      </header>

      <div className="flex items-center justify-between gap-2 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 text-[11px]">
        <span className="text-white/50">Shortcut</span>
        <span className="flex items-center gap-2">
          <kbd className="rounded border border-white/15 bg-white/10 px-1.5 py-0.5 font-sans text-[10px] text-white/70">
            {menuStatus?.label ?? "…"}
          </kbd>
          {menuStatus ? (
            menuStatus.registered ? (
              <span className="text-emerald-300/90">registered</span>
            ) : (
              <span className="text-red-300/90">unavailable</span>
            )
          ) : (
            <span className="text-white/30">checking</span>
          )}
        </span>
      </div>

      {menuStatus?.error && (
        <p className="text-[10px] leading-relaxed text-red-300/80">
          {menuStatus.error}
        </p>
      )}

      <button
        type="button"
        className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-sm font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
        onClick={() => void openMenu(null, null)}
      >
        Open menu at cursor
      </button>

      <div className="flex items-end gap-2">
        <label className="flex-1 text-[11px] text-white/50">
          X (physical)
          <input
            className="mt-1 w-full rounded-md border border-white/10 bg-black/30 px-2 py-1.5 text-xs text-white outline-none focus:border-white/30"
            inputMode="numeric"
            value={x}
            onChange={(event) => setX(event.currentTarget.value)}
            placeholder="-1920"
          />
        </label>
        <label className="flex-1 text-[11px] text-white/50">
          Y (physical)
          <input
            className="mt-1 w-full rounded-md border border-white/10 bg-black/30 px-2 py-1.5 text-xs text-white outline-none focus:border-white/30"
            inputMode="numeric"
            value={y}
            onChange={(event) => setY(event.currentTarget.value)}
            placeholder="1080"
          />
        </label>
        <button
          type="button"
          className="rounded-md border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-white/80 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
          onClick={openAtCoordinates}
        >
          Open
        </button>
      </div>

      <p
        className={`mt-auto text-[11px] leading-relaxed ${
          isError ? "text-red-300/90" : "text-white/40"
        }`}
        role="status"
      >
        {message}
      </p>
    </main>
  );
}
