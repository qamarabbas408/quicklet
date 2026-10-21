import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FileConfirmation } from "./FileConfirmation";
import { QuickletMark } from "./icons";
import { RadialMenu } from "./RadialMenu";
import type { RadialAction } from "../lib/actions";
import { IS_TAURI, APP_NAME } from "../lib/constants";
import type { InputSelection } from "../lib/input";

export function MenuWindow() {
  const [isOpen, setIsOpen] = useState(true);
  const [selectedAction, setSelectedAction] = useState<RadialAction | null>(
    null,
  );
  const [inputSelection, setInputSelection] = useState<InputSelection | null>(
    null,
  );
  const [isPicking, setIsPicking] = useState(false);
  const [pickerError, setPickerError] = useState<string | null>(null);

  const resetInput = useCallback(() => {
    setInputSelection(null);
    setSelectedAction(null);
    setPickerError(null);
    setIsPicking(false);
  }, []);

  const dismiss = useCallback(() => {
    resetInput();

    if (IS_TAURI) {
      void invoke("dismiss_menu").catch(() => setIsOpen(false));
      return;
    }

    setIsOpen(false);
  }, [resetInput]);

  useEffect(() => {
    if (!IS_TAURI) return;

    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listen("menu-opened", () => {
      resetInput();
      setIsOpen(true);
    }).then((stopListening) => {
      if (disposed) {
        stopListening();
      } else {
        unlisten = stopListening;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [resetInput]);

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

  const openPicker = useCallback(
    async (action: RadialAction, append: boolean) => {
      setIsPicking(true);
      setPickerError(null);

      try {
        const selection = await invoke<InputSelection | null>(
          "pick_input_files",
          { actionId: action.id, append },
        );

        if (selection && selection.files.length > 0) {
          setSelectedAction(action);
          setInputSelection(selection);
        }
      } catch (error) {
        setPickerError(String(error));
      } finally {
        setIsPicking(false);
      }
    },
    [],
  );

  const selectAction = useCallback(
    (action: RadialAction) => {
      setSelectedAction(action);
      setPickerError(null);

      if (!IS_TAURI || action.input === "none") return;
      void openPicker(action, false);
    },
    [openPicker],
  );

  const removeFile = async (id: number) => {
    try {
      const selection = await invoke<InputSelection>("remove_input_file", {
        id,
      });
      setInputSelection(selection.files.length > 0 ? selection : null);
    } catch (error) {
      setPickerError(String(error));
    }
  };

  const backToMenu = () => {
    if (IS_TAURI) {
      void invoke("clear_input").catch(() => {});
    }
    setInputSelection(null);
    setPickerError(null);
  };

  const chooseDifferentFiles = () => {
    if (selectedAction) {
      void openPicker(selectedAction, false);
    }
  };

  const addMoreFiles = () => {
    if (selectedAction) {
      void openPicker(selectedAction, true);
    }
  };

  const reopen = () => {
    resetInput();
    setIsOpen(true);
  };

  return (
    <main className="app-shell">
      {inputSelection ? (
        <FileConfirmation
          selection={inputSelection}
          isPicking={isPicking}
          error={pickerError}
          onRemove={(id) => void removeFile(id)}
          onAddMore={addMoreFiles}
          onReplace={chooseDifferentFiles}
          onBack={backToMenu}
          onDismiss={dismiss}
        />
      ) : isOpen ? (
        <>
          <RadialMenu
            selectedAction={selectedAction}
            onSelect={selectAction}
            onDismiss={dismiss}
          />
          {pickerError && (
            <div className="pointer-events-none absolute inset-x-0 top-4 z-10 flex justify-center px-4">
              <p
                role="alert"
                className="rounded-full border border-red-300/20 bg-red-950/70 px-3 py-1.5 text-[11px] text-red-200 backdrop-blur"
              >
                {pickerError}
              </p>
            </div>
          )}
        </>
      ) : (
        <div className="flex h-full items-center justify-center px-6">
          <button
            type="button"
            className="group flex items-center gap-3 rounded-full border border-white/10 bg-white/[0.06] px-5 py-3 text-sm font-medium text-white/80 shadow-2xl backdrop-blur-xl transition-colors hover:border-white/20 hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
            onClick={reopen}
          >
            <span className="flex h-8 w-8 items-center justify-center rounded-full bg-white/10">
              <QuickletMark className="h-4 w-4" />
            </span>
            <span className="flex flex-col items-start leading-tight">
              <span>Open {APP_NAME} menu</span>
              <span className="text-[11px] font-normal text-white/40">
                Browser preview
              </span>
            </span>
          </button>
        </div>
      )}
    </main>
  );
}
