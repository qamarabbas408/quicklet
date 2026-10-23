import type { ComponentType } from "react";
import type { SplitMode, SplitPlan, SplitSourceInfo } from "../lib/actions";
import { formatBytes, formatFileCount, formatPageRanges } from "../lib/format";

type SplitSettingsProps = {
  source: SplitSourceInfo;
  mode: SplitMode;
  ranges: string;
  plan: SplitPlan | null;
  isPlanning: boolean;
  error: string | null;
  categoryIcon?: ComponentType<{ className?: string }>;
  onModeChange: (mode: SplitMode) => void;
  onRangesChange: (ranges: string) => void;
  onBack: () => void;
  onContinue: () => void;
  onDismiss: () => void;
};


export function SplitSettings({
  source,
  mode,
  ranges,
  plan,
  isPlanning,
  error,
  categoryIcon,
  onModeChange,
  onRangesChange,
  onBack,
  onContinue,
  onDismiss,
}: SplitSettingsProps) {
  const CategoryIcon = categoryIcon;
  const included = plan
    ? plan.outputs.reduce((sum, output) => sum + output.pages.length, 0)
    : 0;
  const excluded = plan?.excludedPages ?? [];
  const outputCount = plan?.outputs.length ?? 0;
  const canContinue = plan !== null && !isPlanning && error === null;
  const modeButton = (value: SplitMode, label: string, detail: string) => (
    <button
      type="button"
      aria-pressed={mode === value}
      className={`flex-1 rounded-lg border px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 ${
        mode === value
          ? "border-white/25 bg-white/15 text-white"
          : "border-white/10 bg-white/[0.03] text-white/60 hover:bg-white/10 hover:text-white"
      }`}
      onClick={() => onModeChange(value)}
    >
      <span className="block text-xs font-medium">{label}</span>
      <span className="mt-0.5 block text-[10px] text-white/40">{detail}</span>
    </button>
  );

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
        aria-labelledby="split-settings-title"
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
            <div className="flex items-baseline justify-between gap-2">
              <h2
                id="split-settings-title"
                className="truncate text-sm font-semibold text-white"
              >
                Split settings
              </h2>
              <span className="shrink-0 text-[11px] text-white/40">
                Split PDF
              </span>
            </div>
            <p className="mt-0.5 truncate text-[11px] text-white/45">
              {source.name} · {source.pages}{" "}
              {source.pages === 1 ? "page" : "pages"} ·{" "}
              {formatBytes(source.bytes)}
            </p>
          </div>
        </header>

        <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
          <p className="text-[10px] uppercase tracking-wide text-white/35">
            Split mode
          </p>
          <div className="mt-1.5 flex gap-2">
            {modeButton("every", "Every page", "One PDF per page")}
            {modeButton("ranges", "Custom ranges", "Choose page groups")}
          </div>
        </div>

        {mode === "ranges" ? (
          <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
            <label
              htmlFor="split-ranges"
              className="text-[10px] uppercase tracking-wide text-white/35"
            >
              Page ranges
            </label>
            <input
              id="split-ranges"
              value={ranges}
              onChange={(event) => onRangesChange(event.target.value)}
              placeholder="1-3, 4-7, 8-10"
              spellCheck={false}
              autoComplete="off"
              className="mt-1.5 w-full rounded-lg border border-white/15 bg-black/20 px-2.5 py-2 text-xs text-white placeholder:text-white/25 focus:border-white/30 focus:outline-none focus:ring-2 focus:ring-white/20"
            />
            <p className="mt-1.5 text-[10px] text-white/35">
              Ascending, non-overlapping ranges. Omitted pages are excluded.
            </p>
          </div>
        ) : (
          <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
            <p className="text-xs text-white/80">
              Creates {formatFileCount(source.pages)} with one page each.
            </p>
            <p className="mt-1 text-[10px] text-white/35">
              Original page order is preserved.
            </p>
          </div>
        )}

        <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
          {plan ? (
            <>
              <div className="flex items-center justify-between text-[11px]">
                <span className="text-white/45">Output files</span>
                <span className="text-white/85">
                  {formatFileCount(outputCount)}
                </span>
              </div>
              <div className="mt-1 flex items-center justify-between text-[11px]">
                <span className="text-white/45">Included pages</span>
                <span className="text-white/85">
                  {included} of {source.pages}
                </span>
              </div>
              <div className="mt-1 flex items-start justify-between gap-3 text-[11px]">
                <span className="shrink-0 text-white/45">Excluded</span>
                <span
                  className={`text-right ${
                    excluded.length > 0 ? "text-amber-200/85" : "text-emerald-300/80"
                  }`}
                >
                  {excluded.length > 0
                    ? `Pages ${formatPageRanges(excluded)}`
                    : "All pages included"}
                </span>
              </div>
            </>
          ) : (
            <p className="text-[11px] text-white/45">
              {isPlanning ? "Checking ranges…" : "Enter valid page ranges."}
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

        <p className="text-[10px] text-white/30">
          Limits: {source.maxPages.toLocaleString()} pages ·{" "}
          {source.maxOutputs.toLocaleString()} output files ·{" "}
          {formatBytes(source.maxInputBytes)} input. Existing files are never
          overwritten.
        </p>

        <button
          type="button"
          className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-40"
          disabled={!canContinue}
          onClick={onContinue}
        >
          {isPlanning ? "Checking ranges…" : "Choose output folder"}
        </button>

        <div className="flex items-center justify-between text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            onClick={onBack}
          >
            &larr; Back to files
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
