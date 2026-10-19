import { useCallback, useEffect, useState } from "react";
import { QuickletMark } from "./components/icons";
import { RadialMenu } from "./components/RadialMenu";
import type { RadialAction } from "./lib/actions";
import "./App.css";

function App() {
  const [isOpen, setIsOpen] = useState(true);
  const [selectedAction, setSelectedAction] = useState<RadialAction | null>(
    null,
  );

  const dismiss = useCallback(() => {
    setIsOpen(false);
    setSelectedAction(null);
  }, []);

  useEffect(() => {
    if (!isOpen) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        dismiss();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, dismiss]);

  return (
    <main className="app-shell">
      {isOpen ? (
        <RadialMenu
          selectedAction={selectedAction}
          onSelect={setSelectedAction}
          onDismiss={dismiss}
        />
      ) : (
        <div className="flex h-full items-center justify-center px-6">
          <button
            type="button"
            className="group flex items-center gap-3 rounded-full border border-white/10 bg-white/[0.06] px-5 py-3 text-sm font-medium text-white/80 shadow-2xl backdrop-blur-xl transition-colors hover:border-white/20 hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
            onClick={() => setIsOpen(true)}
          >
            <span className="flex h-8 w-8 items-center justify-center rounded-full bg-white/10">
              <QuickletMark className="h-4 w-4" />
            </span>
            <span className="flex flex-col items-start leading-tight">
              <span>Open Quicklet menu</span>
              <span className="text-[11px] font-normal text-white/40">
                M1 radial menu prototype
              </span>
            </span>
          </button>
        </div>
      )}
    </main>
  );
}

export default App;
