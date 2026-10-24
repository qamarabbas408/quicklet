import type { ComponentType } from "react";
import type {
  PageSelectionMode,
  PageSelectionPlan,
  PageSourceInfo,
} from "../lib/actions";
import { formatBytes, formatPageRanges } from "../lib/format";

type PageSelectionSettingsProps = {
  source: PageSourceInfo;
  mode: PageSelectionMode;
  selection: string;
  plan: PageSelectionPlan | null;
  isPlanning: boolean;
  error: string | null;
  categoryIcon?: ComponentType<{ className?: string }>;
  onSelectionChange: (value: string) => void;
  onBack: () => void;
  onContinue: () => void;
  onDismiss: () => void;
};

export function PageSelectionSettings({
  source,
  mode,
  selection,
  plan,
  isPlanning,
  error,
  categoryIcon,
  onSelectionChange,
  onBack,
  onContinue,
  onDismiss,
}: PageSelectionSettingsProps) {
  const CategoryIcon = categoryIcon;
  const isExtract = mode === "extract";
  const actionLabel = isExtract ? "Extract Pages" : "Delete Pages";
  const inputLabel = isExtract ? "Pages to extract" : "Pages to remove";
  const canContinue = plan !== null && !isPlanning && error === null;

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
        aria-labelledby="page-selection-title"
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
                id="page-selection-title"
                className="truncate text-sm font-semibold text-white"
              >
                Page selection
              </h2>
              <span className="shrink-0 text-[11px] text-white/40">
                {actionLabel}
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
          <label
            htmlFor="page-selection"
            className="text-[10px] uppercase tracking-wide text-white/35"
          >
            {inputLabel}
          </label>
          <input
            id="page-selection"
            value={selection}
            onChange={(event) => onSelectionChange(event.target.value)}
            placeholder={isExtract ? "1, 3-5, 8" : "2, 5-7"}
            spellCheck={false}
            autoComplete="off"
            className="mt-1.5 w-full rounded-lg border border-white/15 bg-black/20 px-2.5 py-2 text-xs text-white placeholder:text-white/25 focus:border-white/30 focus:outline-none focus:ring-2 focus:ring-white/20"
          />
          <p className="mt-1.5 text-[10px] text-white/35">
            Comma-separated pages and ranges. Ranges must not overlap or repeat
            pages.
          </p>
        </div>

        <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
          {plan ? (
            <>
              <div className="flex items-start justify-between gap-3 text-[11px]">
                <span className="shrink-0 text-white/45">
                  {isExtract ? "Selected pages" : "Remove pages"}
                </span>
                <span className="text-right text-white/85">
                  {formatPageRanges(plan.selectedPages)}
                </span>
              </div>
              <div className="mt-1 flex items-center justify-between text-[11px]">
                <span className="text-white/45">
                  {isExtract ? "Output" : "Remaining"}
                </span>
                <span className="text-white/85">
                  {plan.outputCount}{" "}
                  {plan.outputCount === 1 ? "page" : "pages"}
                </span>
              </div>
              <div className="mt-1 flex items-start justify-between gap-3 text-[11px]">
                <span className="shrink-0 text-white/45">Order</span>
                <span className="text-right text-white/85">
                  {isExtract ? "As entered" : "Original document order"}
                </span>
              </div>
            </>
          ) : (
            <p className="text-[11px] text-white/45">
              {isPlanning
                ? "Checking selection…"
                : isExtract
                  ? "Select the pages to keep."
                  : "Select the pages to remove."}
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

        <p className="text-[10px] text-white/35">
          Direct links are preserved when both pages remain in the output.
          Links to removed or unselected pages are dropped. Encrypted PDFs are
          not supported.
        </p>

        <p className="text-[10px] text-white/30">
          Limits: {source.maxPages.toLocaleString()} pages ·{" "}
          {source.maxRanges.toLocaleString()} ranges ·{" "}
          {formatBytes(source.maxInputBytes)} input.
        </p>

        <button
          type="button"
          className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-40"
          disabled={!canContinue}
          onClick={onContinue}
        >
          {isPlanning ? "Checking selection…" : "Choose output"}
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
