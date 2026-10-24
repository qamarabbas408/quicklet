import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ActionList } from "./ActionList";
import { CompressionSettings } from "./CompressionSettings";
import { FileConfirmation } from "./FileConfirmation";
import { OutputPanel } from "./OutputPanel";
import { SplitSettings } from "./SplitSettings";
import { QuickletMark } from "./icons";
import { RadialMenu } from "./RadialMenu";
import type {
  ActionInfo,
  CompressionPreset,
  CompressionSourceInfo,
  ConversionResult,
  OutputDirectoryInfo,
  OutputInfo,
  RadialAction,
  SplitMode,
  SplitPlan,
  SplitSourceInfo,
} from "../lib/actions";
import { APP_NAME, IS_TAURI } from "../lib/constants";
import type { InputSelection } from "../lib/input";

type Stage =
  | "radial"
  | "actions"
  | "input"
  | "review"
  | "configure"
  | "compression"
  | "output";

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
  const [isConverting, setIsConverting] = useState(false);
  const [conversionResult, setConversionResult] =
    useState<ConversionResult | null>(null);
  const [conversionError, setConversionError] = useState<string | null>(null);
  const [resultActionError, setResultActionError] = useState<string | null>(
    null,
  );
  const [isPicking, setIsPicking] = useState(false);
  const [isInspectingSplit, setIsInspectingSplit] = useState(false);
  const [splitSource, setSplitSource] = useState<SplitSourceInfo | null>(null);
  const [splitMode, setSplitMode] = useState<SplitMode>("every");
  const [splitRanges, setSplitRanges] = useState("");
  const [splitPlan, setSplitPlan] = useState<SplitPlan | null>(null);
  const [splitPlanError, setSplitPlanError] = useState<string | null>(null);
  const [isPlanning, setIsPlanning] = useState(false);
  const [outputDirectory, setOutputDirectory] =
    useState<OutputDirectoryInfo | null>(null);
  const [isInspectingCompress, setIsInspectingCompress] = useState(false);
  const [compressSource, setCompressSource] =
    useState<CompressionSourceInfo | null>(null);
  const [compressionPreset, setCompressionPreset] =
    useState<CompressionPreset>("balanced");
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
    setIsConverting(false);
    setConversionResult(null);
    setConversionError(null);
    setResultActionError(null);
    setPickerError(null);
    setIsPicking(false);
    setIsInspectingSplit(false);
    setSplitSource(null);
    setSplitMode("every");
    setSplitRanges("");
    setSplitPlan(null);
    setSplitPlanError(null);
    setIsPlanning(false);
    setOutputDirectory(null);
    setIsInspectingCompress(false);
    setCompressSource(null);
    setCompressionPreset("balanced");
    setIsDropMode(false);
    setIsDragActive(false);
    setDragCount(0);
  }, []);

  const dismiss = useCallback(() => {
    if (isConverting) return;

    resetSession();

    if (IS_TAURI) {
      void invoke("dismiss_menu").catch(() => setIsOpen(false));
      return;
    }

    setIsOpen(false);
  }, [isConverting, resetSession]);

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
        setOutputDirectory(null);
        setCompressSource(null);
        setConversionResult(null);
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

  const beginSplitSetup = useCallback(async () => {
    if (isInspectingSplit) return;

    setIsInspectingSplit(true);
    setPickerError(null);
    setSplitPlan(null);
    setSplitPlanError(null);

    try {
      const before = await invoke<InputSelection>("get_input");
      const source = await invoke<SplitSourceInfo>("inspect_split_source");
      const after = await invoke<InputSelection>("get_input");

      if (
        after.files.length !== 1 ||
        before.files[0]?.id !== after.files[0]?.id
      ) {
        setInputSelection(after);
        setStage("input");
        return;
      }

      setSplitSource(source);
      setSplitMode("every");
      setSplitRanges("");
      setOutputDirectory(null);
      setConversionResult(null);
      setConversionError(null);
      setResultActionError(null);
      setStage("configure");
    } catch (error) {
      setPickerError(String(error));
    } finally {
      setIsInspectingSplit(false);
    }
  }, [isInspectingSplit]);

  const beginCompressionSetup = useCallback(async () => {
    if (isInspectingCompress) return;

    setIsInspectingCompress(true);
    setPickerError(null);
    setCompressSource(null);

    try {
      const before = await invoke<InputSelection>("get_input");
      const source = await invoke<CompressionSourceInfo>(
        "inspect_compress_source",
      );
      const after = await invoke<InputSelection>("get_input");

      if (
        after.files.length !== 1 ||
        before.files[0]?.id !== after.files[0]?.id
      ) {
        setInputSelection(after);
        setStage("input");
        return;
      }

      setCompressSource(source);
      setCompressionPreset("balanced");
      setOutputInfo(null);
      setOutputError(null);
      setConversionResult(null);
      setConversionError(null);
      setResultActionError(null);
      setStage("compression");
    } catch (error) {
      setPickerError(String(error));
    } finally {
      setIsInspectingCompress(false);
    }
  }, [isInspectingCompress]);

  useEffect(() => {
    if (stage !== "configure" || splitSource === null) return;

    let disposed = false;
    setIsPlanning(true);

    const timer = window.setTimeout(() => {
      void invoke<SplitPlan>("plan_split", {
        pageCount: splitSource.pages,
        mode: splitMode,
        ranges: splitMode === "ranges" ? splitRanges : "",
      })
        .then((plan) => {
          if (disposed) return;
          setSplitPlan(plan);
          setSplitPlanError(null);
        })
        .catch((error) => {
          if (disposed) return;
          setSplitPlan(null);
          setSplitPlanError(String(error));
        })
        .finally(() => {
          if (!disposed) setIsPlanning(false);
        });
    }, 180);

    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [stage, splitSource, splitMode, splitRanges]);

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
    setConversionResult(null);
    setConversionError(null);
    setResultActionError(null);
    setSplitSource(null);
    setSplitMode("every");
    setSplitRanges("");
    setSplitPlan(null);
    setSplitPlanError(null);
    setOutputDirectory(null);
    setCompressSource(null);
    setCompressionPreset("balanced");

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
        setOutputDirectory(null);
        setSplitSource(null);
        setSplitPlan(null);
        setSplitPlanError(null);
        setCompressSource(null);
        setConversionResult(null);
        setConversionError(null);
        setResultActionError(null);
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
      setOutputDirectory(null);
      setSplitSource(null);
      setSplitPlan(null);
      setSplitPlanError(null);
      setCompressSource(null);
      setConversionResult(null);
      setConversionError(null);
      setResultActionError(null);
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
      setConversionResult(null);
      setConversionError(null);
      setResultActionError(null);
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
      setOutputDirectory(null);
      setSplitSource(null);
      setSplitPlan(null);
      setSplitPlanError(null);
      setCompressSource(null);
      setOutputError(null);
      setConversionResult(null);
      setConversionError(null);
      setResultActionError(null);
      setStage("input");
    } catch (error) {
      setPickerError(String(error));
    }
  };

  const chooseOutput = async () => {
    setIsChoosingOutput(true);
    setOutputError(null);
    setConversionResult(null);
    setConversionError(null);
    setResultActionError(null);

    try {
      if (selectedOperation?.id === "pdf.split") {
        const info = await invoke<OutputDirectoryInfo | null>(
          "choose_output_directory",
        );
        if (info) {
          setOutputDirectory(info);
        }
      } else {
        const info = await invoke<OutputInfo | null>("choose_output", {
          preset:
            selectedOperation?.id === "pdf.compress"
              ? compressionPreset
              : null,
        });
        if (info) {
          setOutputInfo(info);
        }
      }
    } catch (error) {
      setOutputError(String(error));
    } finally {
      setIsChoosingOutput(false);
    }
  };

  const convert = async () => {
    if (isConverting) return;

    setIsConverting(true);
    setConversionError(null);
    setConversionResult(null);
    setResultActionError(null);

    try {
      let result: ConversionResult;
      if (selectedOperation?.id === "pdf.split") {
        result = await invoke<ConversionResult>("split_pdf", {
          mode: splitMode,
          ranges: splitMode === "ranges" ? splitRanges : "",
        });
      } else if (selectedOperation?.id === "pdf.compress") {
        result = await invoke<ConversionResult>("compress_pdf", {
          preset: compressionPreset,
        });
      } else {
        const command =
          selectedOperation?.id === "pdf.merge"
            ? "merge_pdfs"
            : "convert_to_pdf";
        result = await invoke<ConversionResult>(command);
      }
      setConversionResult(result);
    } catch (error) {
      setConversionError(String(error));
    } finally {
      setIsConverting(false);
    }
  };

  const runResultAction = async (
    command: "reveal_output" | "open_output" | "open_output_directory",
  ) => {
    setResultActionError(null);

    try {
      await invoke(command);
    } catch (error) {
      setResultActionError(String(error));
    }
  };

  const backToReview = () => {
    if (isConverting) return;
    setConversionResult(null);
    setConversionError(null);
    setResultActionError(null);
    if (selectedOperation?.id === "pdf.split") {
      setStage("configure");
    } else if (selectedOperation?.id === "pdf.compress") {
      setStage("compression");
    } else {
      setStage("review");
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
  const showConfigure =
    stage === "configure" &&
    selectedOperation !== null &&
    splitSource !== null &&
    inputSelection !== null;
  const showCompression =
    stage === "compression" &&
    selectedOperation !== null &&
    compressSource !== null &&
    inputSelection !== null;
  const showOutput =
    stage === "output" && selectedOperation !== null && inputSelection !== null;
  const isSplit = selectedOperation?.id === "pdf.split";
  const isMerge = selectedOperation?.id === "pdf.merge";
  const isCompress = selectedOperation?.id === "pdf.compress";
  const minFiles = isSplit || isCompress ? 1 : isMerge ? 2 : 1;
  const fileNoun = isSplit || isCompress ? "PDF" : isMerge ? "PDFs" : "images";
  const continueLabel = isSplit
    ? "Configure split"
    : isCompress
      ? "Choose preset"
      : undefined;

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
          minFiles={minFiles}
          fileNoun={fileNoun}
          continueLabel={continueLabel}
          isContinuing={
            isSplit ? isInspectingSplit : isCompress ? isInspectingCompress : false
          }
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
          onContinue={() => {
            if (isSplit) {
              void beginSplitSetup();
            } else if (isCompress) {
              void beginCompressionSetup();
            } else {
              setStage("review");
            }
          }}
          onReorder={(ids) => void reorderFiles(ids)}
          onDismiss={dismiss}
        />
      ) : showReview ? (
        <FileConfirmation
          selection={inputSelection}
          actionLabel={selectedOperation.label}
          categoryIcon={selectedAction?.icon}
          mode="review"
          minFiles={minFiles}
          fileNoun={fileNoun}
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
      ) : showConfigure ? (
        <SplitSettings
          source={splitSource}
          mode={splitMode}
          ranges={splitRanges}
          plan={splitPlan}
          isPlanning={isPlanning}
          error={splitPlanError}
          categoryIcon={selectedAction?.icon}
          onModeChange={(mode) => {
            setSplitMode(mode);
            setIsPlanning(true);
            setSplitPlanError(null);
          }}
          onRangesChange={(value) => {
            setSplitRanges(value);
            setIsPlanning(true);
            setSplitPlanError(null);
          }}
          onBack={() => setStage("input")}
          onContinue={() => setStage("output")}
          onDismiss={dismiss}
        />
      ) : showCompression ? (
        <CompressionSettings
          source={compressSource}
          preset={compressionPreset}
          categoryIcon={selectedAction?.icon}
          onPresetChange={(preset) => {
            setCompressionPreset(preset);
            setOutputInfo(null);
            setOutputError(null);
          }}
          onBack={() => setStage("input")}
          onContinue={() => setStage("output")}
          onDismiss={dismiss}
        />
      ) : showOutput ? (
        <OutputPanel
          category={selectedAction}
          actionLabel={selectedOperation.label}
          selection={inputSelection}
          output={outputInfo}
          outputDirectory={outputDirectory}
          isChoosing={isChoosingOutput}
          isConverting={isConverting}
          isMerge={isMerge}
          isSplit={isSplit}
          isCompress={isCompress}
          compressionPreset={compressionPreset}
          compressSource={compressSource}
          splitMode={splitMode}
          splitPlan={splitPlan}
          result={conversionResult}
          error={conversionError ?? outputError}
          resultError={resultActionError}
          onChooseOutput={() => void chooseOutput()}
          onConvert={() => void convert()}
          onReveal={() => void runResultAction("reveal_output")}
          onOpen={() => void runResultAction("open_output")}
          onOpenFolder={() => void runResultAction("open_output_directory")}
          onBack={backToReview}
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
                className="max-h-20 max-w-full overflow-y-auto rounded-2xl border border-red-300/20 bg-red-950/70 px-3 py-1.5 text-[11px] break-words text-red-200 backdrop-blur [overflow-wrap:anywhere]"
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
