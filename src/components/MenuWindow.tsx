import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ActionList } from "./ActionList";
import { FileConfirmation } from "./FileConfirmation";
import { OutputPanel } from "./OutputPanel";
import { QuickletMark } from "./icons";
import { RadialMenu } from "./RadialMenu";
import type {
  ActionInfo,
  OutputInfo,
  RadialAction,
} from "../lib/actions";
import { APP_NAME, IS_TAURI } from "../lib/constants";
import type { InputSelection } from "../lib/input";

type Stage = "radial" | "actions" | "input" | "review" | "output";

type DragState = {
  active: boolean;
  count: number;
};

export function MenuWindow() {
  const [isOpen, setIsOpen] = useState(true);
  const [stage, setStage] = useState<Stage>("radial");
  const [selectedAction, setSelectedAction] = useState<RadialAction | null>(
    null,
  );
  const [actions, setActions] = useState<ActionInfo[]>([]);
  const [actionsLoading, setActionsLoading] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [selectedOperation, setSelectedOperation] =
    useState<ActionInfo | null>(null);
  const [inputSelection, setInputSelection] = useState<InputSelection | null>(
    null,
  );
  const [outputInfo, setOutputInfo] = useState<OutputInfo | null>(null);
  const [outputError, setOutputError] = useState<string | null>(null);
  const [isChoosingOutput, setIsChoosingOutput] = useState(false);
  const [isPicking, setIsPicking] = useState(false);
  const [pickerError, setPickerError] = useState<string | null>(null);
  const [isDropMode, setIsDropMode] = useState(false);
  const [isDragActive, setIsDragActive] = useState(false);
  const [dragCount, setDragCount] = useState(0);

  const resetSession = useCallback(() => {
    setStage("radial");
    setSelectedAction(null);
    setActions([]);
    setActionsLoading(false);
    setActionError(null);
    setSelectedOperation(null);
    setInputSelection(null);
    setOutputInfo(null);
    setOutputError(null);
    setIsChoosingOutput(false);
    setPickerError(null);
    setIsPicking(false);
    setIsDropMode(false);
    setIsDragActive(false);
    setDragCount(0);
  }, []);

  const dismiss = useCallback(() => {
    resetSession();

    if (IS_TAURI) {
      void invoke("dismiss_menu").catch(() => setIsOpen(false));
      return;
    }

    setIsOpen(false);
  }, [resetSession]);

  useEffect(() => {
    if (!IS_TAURI) return;

    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listen("menu-opened", () => {
      resetSession();
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
  }, [resetSession]);

  useEffect(() => {
    if (!IS_TAURI) return;

    let disposed = false;
    const unlisteners: Array<() => void> = [];

    const track = (listener: Promise<() => void>) => {
      void listener.then((stopListening) => {
        if (disposed) {
          stopListening();
        } else {
          unlisteners.push(stopListening);
        }
      });
    };

    track(
      listen<InputSelection>("input-updated", (event) => {
        setInputSelection(event.payload);
        setOutputInfo(null);
        setStage("input");
        setIsDropMode(false);
        setIsDragActive(false);
        setDragCount(0);
        setIsPicking(false);
        setPickerError(null);
      }),
    );

    track(
      listen<DragState>("input-drag", (event) => {
        setIsDragActive(event.payload.active);
        setDragCount(event.payload.count);
      }),
    );

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
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

  const openCategory = useCallback(async (category: RadialAction) => {
    setSelectedAction(category);
    setActionError(null);

    if (!IS_TAURI) return;

    setStage("actions");
    setActionsLoading(true);

    try {
      const list = await invoke<ActionInfo[]>("get_category_actions", {
        categoryId: category.id,
      });
      setActions(list);
    } catch (error) {
      setActionError(String(error));
      setActions([]);
    } finally {
      setActionsLoading(false);
    }
  }, []);

  const chooseOperation = async (action: ActionInfo) => {
    setActionError(null);

    try {
      const started = await invoke<ActionInfo>("start_action", {
        actionId: action.id,
      });
      const selection = await invoke<InputSelection>("get_input");

      if (selectedOperation?.id !== started.id) {
        setOutputInfo(null);
        setOutputError(null);
      }

      setSelectedOperation(started);
      setInputSelection(selection);
      setStage("input");
    } catch (error) {
      setActionError(String(error));
    }
  };

  const openPicker = useCallback(async (append: boolean) => {
    setIsPicking(true);
    setPickerError(null);

    try {
      const selection = await invoke<InputSelection | null>("pick_input_files", {
        append,
      });

      if (selection && selection.files.length > 0) {
        setInputSelection(selection);
        setOutputInfo(null);
        setStage("input");
      }
    } catch (error) {
      setPickerError(String(error));
    } finally {
      setIsPicking(false);
    }
  }, []);

  const removeFile = async (id: number) => {
    try {
      const selection = await invoke<InputSelection>("remove_input_file", {
        id,
      });
      setInputSelection(selection);
      setOutputInfo(null);
      if (selection.files.length === 0) {
        setStage("input");
      }
    } catch (error) {
      setPickerError(String(error));
    }
  };

  const reorderFiles = async (ids: number[]) => {
    try {
      const selection = await invoke<InputSelection>("reorder_input", { ids });
      setInputSelection(selection);
    } catch (error) {
      setPickerError(String(error));
    }
  };

  const clearAll = async () => {
    try {
      await invoke("clear_input");
      const selection = await invoke<InputSelection>("get_input");
      setInputSelection(selection);
      setOutputInfo(null);
      setOutputError(null);
      setStage("input");
    } catch (error) {
      setPickerError(String(error));
    }
  };

  const chooseOutput = async () => {
    setIsChoosingOutput(true);
    setOutputError(null);

    try {
      const info = await invoke<OutputInfo | null>("choose_output");
      if (info) {
        setOutputInfo(info);
      }
    } catch (error) {
      setOutputError(String(error));
    } finally {
      setIsChoosingOutput(false);
    }
  };

  const enterDropMode = async () => {
    if (!IS_TAURI) return;

    setIsDropMode(true);
    setIsDragActive(false);
    setDragCount(0);
    setPickerError(null);

    try {
      await invoke("set_drop_mode", { active: true });
    } catch (error) {
      setIsDropMode(false);
      setPickerError(String(error));
    }
  };

  const cancelDropMode = () => {
    setIsDropMode(false);
    setIsDragActive(false);
    setDragCount(0);

    if (IS_TAURI) {
      void invoke("set_drop_mode", { active: false }).catch(() => {});
    }
  };

  const backToLauncher = () => {
    setStage("radial");
    setSelectedAction(null);
    setActionError(null);
  };

  const backToActions = () => {
    setStage("actions");
    setActionError(null);
  };

  const reopen = () => {
    resetSession();
    setIsOpen(true);
  };

  const showActions = stage === "actions" && selectedAction !== null;
  const showInput =
    stage === "input" && selectedOperation !== null && inputSelection !== null;
  const showReview =
    stage === "review" && selectedOperation !== null && inputSelection !== null;
  const showOutput =
    stage === "output" && selectedOperation !== null && inputSelection !== null;

  return (
    <main className="app-shell">
      {showActions ? (
        <ActionList
          category={selectedAction}
          actions={actions}
          loading={actionsLoading}
          error={actionError}
          onSelect={(action) => void chooseOperation(action)}
          onBack={backToLauncher}
          onDismiss={dismiss}
        />
      ) : showInput ? (
        <FileConfirmation
          selection={inputSelection}
          actionLabel={selectedOperation.label}
          categoryIcon={selectedAction?.icon}
          mode="input"
          isPicking={isPicking}
          isDropMode={isDropMode}
          isDragActive={isDragActive}
          dragCount={dragCount}
          error={pickerError}
          onRemove={(id) => void removeFile(id)}
          onAddMore={() => void openPicker(true)}
          onReplace={() => void openPicker(false)}
          onClearAll={() => void clearAll()}
          onBack={backToActions}
          onEnterDrop={() => void enterDropMode()}
          onCancelDrop={cancelDropMode}
          onContinue={() => setStage("review")}
          onReorder={(ids) => void reorderFiles(ids)}
          onDismiss={dismiss}
        />
      ) : showReview ? (
        <FileConfirmation
          selection={inputSelection}
          actionLabel={selectedOperation.label}
          categoryIcon={selectedAction?.icon}
          mode="review"
          isPicking={isPicking}
          isDropMode={false}
          isDragActive={false}
          dragCount={0}
          error={pickerError}
          onRemove={(id) => void removeFile(id)}
          onAddMore={() => {}}
          onReplace={() => {}}
          onClearAll={() => void clearAll()}
          onBack={() => setStage("input")}
          onEnterDrop={() => {}}
          onCancelDrop={cancelDropMode}
          onContinue={() => setStage("output")}
          onReorder={(ids) => void reorderFiles(ids)}
          onDismiss={dismiss}
        />
      ) : showOutput ? (
        <OutputPanel
          category={selectedAction}
          actionLabel={selectedOperation.label}
          selection={inputSelection}
          output={outputInfo}
          isChoosing={isChoosingOutput}
          error={outputError}
          onChooseOutput={() => void chooseOutput()}
          onBack={() => setStage("review")}
          onDismiss={dismiss}
        />
      ) : isOpen ? (
        <>
          <RadialMenu
            selectedAction={selectedAction}
            onSelect={(action) => void openCategory(action)}
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
