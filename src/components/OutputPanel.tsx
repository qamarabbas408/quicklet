import type {
  CompressionPreset,
  CompressionSourceInfo,
  ConversionResult,
  OperationProgress,
  OutputDirectoryInfo,
  OutputInfo,
  PageSelectionMode,
  PageSelectionPlan,
  RadialAction,
  SplitMode,
  SplitPlan,
} from "../lib/actions";
import type { InputSelection } from "../lib/input";
import { formatBytes, formatFileCount, formatPageRanges } from "../lib/format";

type OutputPanelProps = {
  category: RadialAction | null;
  actionLabel: string;
  selection: InputSelection;
  output: OutputInfo | null;
  outputDirectory: OutputDirectoryInfo | null;
  isChoosing: boolean;
  isConverting: boolean;
  isMerge: boolean;
  isSplit: boolean;
  isCompress: boolean;
  compressionPreset: CompressionPreset;
  compressSource: CompressionSourceInfo | null;
  pageSelectionMode: PageSelectionMode | null;
  pagePlan: PageSelectionPlan | null;
  splitMode: SplitMode;
  splitPlan: SplitPlan | null;
  progress: OperationProgress | null;
  result: ConversionResult | null;
  error: string | null;
  resultError: string | null;
  isCancellingOperation: boolean;
  onChooseOutput: () => void;
  onConvert: () => void;
  onCancelOperation: () => void;
  onReveal: () => void;
  onOpen: () => void;
  onOpenFolder: () => void;
  onBack: () => void;
  onDismiss: () => void;
};

export function OutputPanel({
  category,
  actionLabel,
  selection,
  output,
  outputDirectory,
  isChoosing,
  isConverting,
  isMerge,
  isSplit,
  isCompress,
  compressionPreset,
  compressSource,
  pageSelectionMode,
  pagePlan,
  splitMode,
  splitPlan,
  progress,
  result,
  error,
  resultError,
  isCancellingOperation,
  onChooseOutput,
  onConvert,
  onCancelOperation,
  onReveal,
  onOpen,
  onOpenFolder,
  onBack,
  onDismiss,
}: OutputPanelProps) {
  const CategoryIcon = category?.icon;
  const revealLabel = /Mac/i.test(navigator.userAgent)
    ? "Reveal in Finder"
    : "Show in folder";
  const files = result?.files ?? [];
  const hasFileList = files.length > 0;
  const compression = result?.compression;
  const isPageSelection = pageSelectionMode !== null;
  const isExtract = pageSelectionMode === "extract";
  const canConvert = isSplit
    ? outputDirectory !== null && splitPlan !== null && !isChoosing && !isConverting
    : isPageSelection
      ? output !== null && pagePlan !== null && !isChoosing && !isConverting
      : output !== null && !isChoosing && !isConverting;
  const splitExcluded = splitPlan?.excludedPages ?? [];
  const splitIncluded = splitPlan
    ? splitPlan.outputs.reduce((sum, item) => sum + item.pages.length, 0)
    : 0;
  const busyLabel = isSplit
    ? "Splitting PDF…"
    : isMerge
      ? "Merging PDFs…"
      : isCompress
        ? "Compressing PDF…"
        : isPageSelection
          ? isExtract
            ? "Extracting pages…"
            : "Deleting pages…"
          : "Creating PDF…";
  const progressMessage =
    progress?.status === "cancelled"
      ? "Compression cancelled"
      : progress?.message ?? busyLabel;
  const progressCompleted =
    progress && typeof progress.completed === "number" ? progress.completed : null;
  const progressTotal =
    progress && typeof progress.total === "number" ? progress.total : null;
  const progressPercent =
    progress && progress.determinate && progressTotal && progressTotal > 0
      ? Math.min(
          100,
          Math.max(0, ((progressCompleted ?? 0) / progressTotal) * 100),
        )
      : null;
  const etaText =
    typeof progress?.etaSeconds === "number"
      ? (() => {
          const totalSeconds = Math.max(0, Math.round(progress.etaSeconds));
          if (totalSeconds < 45) {
            return `About ${totalSeconds} seconds remaining`;
          }
          if (totalSeconds < 90) {
            return "About 1 minute remaining";
          }
          if (totalSeconds < 3600) {
            const minutes = Math.max(1, Math.round(totalSeconds / 60));
            return `About ${minutes} minute${minutes === 1 ? "" : "s"} remaining`;
          }
          const hours = Math.max(1, Math.round(totalSeconds / 3600));
          return `About ${hours} hour${hours === 1 ? "" : "s"} remaining`;
        })()
      : progress && progress.determinate
        ? "Estimating time remaining…"
        : null;
  const actionLabelForButton = isSplit
    ? "Split PDF"
    : isMerge
      ? "Merge PDFs"
      : isCompress
        ? "Compress PDF"
        : isPageSelection
          ? isExtract
            ? "Extract Pages"
            : "Delete Pages"
          : "Convert to PDF";
  const retryLabel = isSplit
    ? "Retry split"
    : isMerge
      ? "Retry merge"
      : isCompress
        ? "Retry compression"
        : isPageSelection
          ? isExtract
            ? "Retry extraction"
            : "Retry deletion"
          : "Retry conversion";
  const presetLabel = (preset: CompressionPreset) =>
    preset === "light"
      ? "Light"
      : preset === "strong"
        ? "Strong"
        : "Balanced";

  return (
    <div
      className="confirmation-overlay"
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onDismiss();
        }
      }}
    >
      <section
        className="confirmation-panel relative"
        aria-labelledby="output-title"
      >
        <div
          className="flex h-4 cursor-grab items-center justify-center active:cursor-grabbing"
          data-tauri-drag-region
          title="Drag to move Quicklet"
        >
          <span className="pointer-events-none h-1 w-12 rounded-full bg-white/25" />
        </div>

        <header
          className="flex cursor-grab items-start gap-3 active:cursor-grabbing"
          data-tauri-drag-region
          title="Drag to move Quicklet"
        >
          <span className="confirmation-icon pointer-events-none">
            {CategoryIcon ? <CategoryIcon className="h-5 w-5" /> : null}
          </span>
          <div className="pointer-events-none min-w-0 flex-1">
            <h2
              id="output-title"
              className="truncate text-sm font-semibold text-white"
            >
              {result
                ? isCompress
                  ? "Compression complete"
                  : isPageSelection
                    ? isExtract
                      ? "Pages extracted"
                      : "Pages deleted"
                    : hasFileList
                    ? files.length === 1
                      ? "PDF created"
                      : "PDFs created"
                    : "PDF created"
                : "Output"}
            </h2>
            <p className="mt-0.5 text-[11px] text-white/45">{actionLabel}</p>
          </div>
        </header>

        {result ? (
          <>
            {hasFileList ? (
              <div className="rounded-xl border border-emerald-300/15 bg-emerald-300/[0.06] px-3 py-3">
                <p className="text-xs font-medium text-emerald-200/90">
                  {formatFileCount(files.length)}
                </p>
                <p className="mt-1 truncate text-[10px] text-white/40">
                  {result.directory}
                </p>
                <p className="mt-2 text-[10px] text-white/45">
                  {result.pages} {result.pages === 1 ? "page" : "pages"} ·{" "}
                  {formatBytes(result.bytesWritten)}
                </p>
                <ul
                  className="confirmation-list mt-2 max-h-28"
                  aria-label="Created PDF files"
                >
                  {files.map((file) => (
                    <li
                      key={file.name}
                      className="flex items-center justify-between gap-2 rounded-lg px-1.5 py-1 text-[10px]"
                    >
                      <span
                        className="min-w-0 truncate text-white/75"
                        title={file.name}
                      >
                        {file.name}
                      </span>
                      <span className="shrink-0 text-white/35">
                        {file.pages}p · {formatBytes(file.bytesWritten)}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            ) : (
              <div className="rounded-xl border border-emerald-300/15 bg-emerald-300/[0.06] px-3 py-3">
                <p className="text-xs font-medium text-emerald-200/90">
                  {result.name}
                </p>
                <p className="mt-1 truncate text-[10px] text-white/40">
                  {result.directory}
                </p>
                <p className="mt-2 text-[10px] text-white/45">
                  {result.pages} {result.pages === 1 ? "page" : "pages"} ·{" "}
                  {formatBytes(result.bytesWritten)}
                </p>
              </div>
            )}

            {compression && (
              <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
                <div className="flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Original</span>
                  <span className="text-white/85">
                    {formatBytes(compression.originalBytes)}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Output</span>
                  <span className="text-white/85">
                    {formatBytes(compression.outputBytes)}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Saved</span>
                  <span className="text-white/85">
                    {formatBytes(compression.savedBytes)}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Reduction</span>
                  <span
                    className={
                      compression.reductionPercent > 0
                        ? "text-emerald-200/90"
                        : "text-amber-200/85"
                    }
                  >
                    {compression.reductionPercent.toFixed(1)}%
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Preset</span>
                  <span className="text-white/85">{compression.preset}</span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Images recompressed</span>
                  <span className="text-white/85">
                    {compression.imagesRecompressed}
                  </span>
                </div>
                <div className="mt-1 flex items-start justify-between gap-3 text-[11px]">
                  <span className="shrink-0 text-white/45">Status</span>
                  <span
                    className={`text-right ${
                      compression.status === "reduced"
                        ? "text-emerald-200/90"
                        : "text-amber-200/85"
                    }`}
                  >
                    {compression.status === "reduced"
                      ? "Reduced"
                      : "No reduction — original retained"}
                  </span>
                </div>
                {compression.status === "no-reduction" && (
                  <p className="mt-2 text-[10px] text-white/35">
                    The compressed candidate was not smaller, so the exact
                    original bytes were written to the destination.
                  </p>
                )}
              </div>
            )}

            {resultError && (
              <p
                role="alert"
                className="max-h-20 shrink-0 overflow-y-auto text-[11px] break-words text-red-300/90 [overflow-wrap:anywhere]"
              >
                {resultError}
              </p>
            )}

            <button
              type="button"
              className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
              onClick={onReveal}
            >
              {revealLabel}
            </button>
            <button
              type="button"
              className="w-full rounded-lg border border-white/10 bg-white/[0.04] px-3 py-2 text-xs text-white/70 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
              onClick={hasFileList ? onOpenFolder : onOpen}
            >
              {hasFileList ? "Open folder" : "Open PDF"}
            </button>
          </>
        ) : (
          <>
            {isPageSelection ? (
              <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
                <div className="flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Format</span>
                  <span className="text-white/85">PDF</span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Mode</span>
                  <span className="text-white/85">
                    {isExtract ? "Extract pages" : "Delete pages"}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">
                    {isExtract ? "Selected pages" : "Remove pages"}
                  </span>
                  <span className="text-white/85">
                    {pagePlan?.selectedPages.length ?? "—"}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">
                    {isExtract ? "Output pages" : "Remaining pages"}
                  </span>
                  <span className="text-white/85">
                    {pagePlan?.outputCount ?? "—"}
                  </span>
                </div>
              </div>
            ) : isCompress ? (
              <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
                <div className="flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Format</span>
                  <span className="text-white/85">PDF</span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Pages</span>
                  <span className="text-white/85">
                    {compressSource?.pages ?? "—"}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Preset</span>
                  <span className="text-white/85">
                    {presetLabel(compressionPreset)}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Original size</span>
                  <span className="text-white/85">
                    {compressSource ? formatBytes(compressSource.bytes) : "—"}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Images detected</span>
                  <span className="text-white/85">
                    {compressSource?.images ?? "—"}
                  </span>
                </div>
              </div>
            ) : isSplit ? (
              <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
                <div className="flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Format</span>
                  <span className="text-white/85">PDF</span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Mode</span>
                  <span className="text-white/85">
                    {splitMode === "every" ? "Every page" : "Custom ranges"}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Output files</span>
                  <span className="text-white/85">
                    {splitPlan ? formatFileCount(splitPlan.outputs.length) : "—"}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Included pages</span>
                  <span className="text-white/85">
                    {splitPlan
                      ? `${splitIncluded} of ${splitPlan.pageCount}`
                      : "—"}
                  </span>
                </div>
                <div className="mt-1 flex items-start justify-between gap-3 text-[11px]">
                  <span className="shrink-0 text-white/45">Excluded</span>
                  <span
                    className={`text-right ${
                      splitExcluded.length > 0
                        ? "text-amber-200/85"
                        : "text-emerald-300/80"
                    }`}
                  >
                    {splitPlan
                      ? splitExcluded.length > 0
                        ? `Pages ${formatPageRanges(splitExcluded)}`
                        : "All pages included"
                      : "—"}
                  </span>
                </div>
              </div>
            ) : (
              <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
                <div className="flex items-center justify-between text-[11px]">
                  <span className="text-white/45">Format</span>
                  <span className="text-white/85">PDF</span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">
                    {isMerge ? "PDFs" : "Images"}
                  </span>
                  <span className="text-white/85">
                    {formatFileCount(selection.files.length)}
                  </span>
                </div>
                <div className="mt-1 flex items-center justify-between text-[11px]">
                  <span className="text-white/45">
                    {isMerge ? "Order" : "Layout"}
                  </span>
                  <span className="text-white/85">
                    {isMerge ? "Preserved" : "A4 portrait · fit with margins"}
                  </span>
                </div>
              </div>
            )}

            <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
              <p className="text-[10px] uppercase tracking-wide text-white/35">
                Save to
              </p>
              {isSplit ? (
                outputDirectory ? (
                  <p className="mt-1 truncate text-[10px] text-white/60">
                    {outputDirectory.directory}
                  </p>
                ) : (
                  <p className="mt-1 text-xs text-white/45">
                    No folder selected
                  </p>
                )
              ) : output ? (
                <>
                  <p className="mt-1 truncate text-xs font-medium text-white/85">
                    {output.name}
                  </p>
                  <p className="mt-0.5 truncate text-[10px] text-white/35">
                    {output.directory}
                  </p>
                </>
              ) : (
                <p className="mt-1 text-xs text-white/45">
                  No destination selected
                </p>
              )}
            </div>

            {error && (
              <p
                role="alert"
                className="max-h-20 shrink-0 overflow-y-auto text-[11px] break-words text-red-300/90 [overflow-wrap:anywhere]"
              >
                {error}
              </p>
            )}

            {(isConverting || progress) && (
              <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
                <div className="mb-2 flex items-center justify-between gap-3 text-[11px]">
                  <span className="flex items-center gap-2 text-white/70">
                    <span className="conversion-spinner" />
                    {progressMessage || busyLabel}
                  </span>
                  {progressPercent !== null ? (
                    <span className="text-white/45">
                      {Math.round(progressPercent)}%
                    </span>
                  ) : null}
                  {isCompress && progress?.status === "running" && !result ? (
                    <button
                      type="button"
                      className="rounded-md border border-white/15 bg-white/5 px-2 py-1 text-[10px] font-medium text-white/75 transition-colors hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-50"
                      disabled={isCancellingOperation || isChoosing}
                      onClick={onCancelOperation}
                    >
                      {isCancellingOperation ? "Cancelling…" : "Cancel"}
                    </button>
                  ) : null}
                </div>
                {progressPercent !== null ? (
                  <div className="h-2 overflow-hidden rounded-full bg-white/10">
                    <div
                      className="h-full rounded-full bg-emerald-400 transition-[width] duration-200"
                      style={{ width: `${progressPercent}%` }}
                    />
                  </div>
                ) : (
                  <div className="h-2 overflow-hidden rounded-full bg-white/10">
                    <div className="h-full w-1/3 animate-pulse rounded-full bg-white/50" />
                  </div>
                )}
                {progress && progress.determinate && progressCompleted !== null && progressTotal !== null && (
                  <p className="mt-2 text-[10px] text-white/45">
                    {progressCompleted} / {progressTotal} {progress.unit ?? "items"}
                  </p>
                )}
                {etaText && (
                  <p className="mt-2 text-[10px] text-emerald-200/80">{etaText}</p>
                )}
              </div>
            )}

            <button
              type="button"
              className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-50"
              disabled={isChoosing || isConverting}
              onClick={onChooseOutput}
            >
              {isChoosing
                ? isSplit
                  ? "Choosing folder…"
                  : "Choosing destination…"
                : isSplit
                  ? outputDirectory
                    ? "Change output folder"
                    : "Choose output folder"
                  : output
                    ? "Change destination"
                    : "Choose destination"}
            </button>

            <button
              type="button"
              className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-40"
              disabled={!canConvert}
              onClick={onConvert}
            >
              {isConverting
                ? busyLabel
                : error
                  ? retryLabel
                  : actionLabelForButton}
            </button>
          </>
        )}

        <div className="flex items-center justify-between text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30 disabled:opacity-40"
            disabled={isConverting}
            onClick={onBack}
          >
            &larr;{" "}
            {isSplit
              ? "Split settings"
              : isCompress
                ? "Compression settings"
                : isPageSelection
                  ? "Page selection"
                  : "Review files"}
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
