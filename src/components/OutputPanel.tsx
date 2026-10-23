import type {
  ConversionResult,
  OutputDirectoryInfo,
  OutputInfo,
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
  splitMode: SplitMode;
  splitPlan: SplitPlan | null;
  result: ConversionResult | null;
  error: string | null;
  resultError: string | null;
  onChooseOutput: () => void;
  onConvert: () => void;
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
  splitMode,
  splitPlan,
  result,
  error,
  resultError,
  onChooseOutput,
  onConvert,
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
  const canConvert = isSplit
    ? outputDirectory !== null && splitPlan !== null && !isChoosing && !isConverting
    : output !== null && !isChoosing && !isConverting;
  const splitExcluded = splitPlan?.excludedPages ?? [];
  const splitIncluded = splitPlan
    ? splitPlan.outputs.reduce((sum, item) => sum + item.pages.length, 0)
    : 0;
  const busyLabel = isSplit
    ? "Splitting PDF…"
    : isMerge
      ? "Merging PDFs…"
      : "Creating PDF…";
  const actionLabelForButton = isSplit
    ? "Split PDF"
    : isMerge
      ? "Merge PDFs"
      : "Convert to PDF";
  const retryLabel = isSplit
    ? "Retry split"
    : isMerge
      ? "Retry merge"
      : "Retry conversion";

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
                ? hasFileList
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
            {isSplit ? (
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

            {isConverting && (
              <div className="flex items-center gap-2 rounded-lg border border-white/10 bg-white/[0.04] px-3 py-2 text-[11px] text-white/60">
                <span className="conversion-spinner" />
                {busyLabel}
              </div>
            )}

            <button
              type="button"
              className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-50"
              disabled={isChoosing || isConverting}
              onClick={onChooseOutput}
            >
              {isChoosing
                ? "Choosing folder…"
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
            &larr; {isSplit ? "Split settings" : "Review files"}
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
