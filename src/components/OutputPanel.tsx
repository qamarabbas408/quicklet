import type { OutputInfo, RadialAction } from "../lib/actions";
import type { InputSelection } from "../lib/input";
import { formatFileCount } from "../lib/format";

type OutputPanelProps = {
  category: RadialAction | null;
  actionLabel: string;
  selection: InputSelection;
  output: OutputInfo | null;
  isChoosing: boolean;
  error: string | null;
  onChooseOutput: () => void;
  onBack: () => void;
  onDismiss: () => void;
};

export function OutputPanel({
  category,
  actionLabel,
  selection,
  output,
  isChoosing,
  error,
  onChooseOutput,
  onBack,
  onDismiss,
}: OutputPanelProps) {
  const CategoryIcon = category?.icon;

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
              Output
            </h2>
            <p className="mt-0.5 text-[11px] text-white/45">{actionLabel}</p>
          </div>
        </header>

        <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
          <div className="flex items-center justify-between text-[11px]">
            <span className="text-white/45">Format</span>
            <span className="text-white/85">PDF</span>
          </div>
          <div className="mt-1 flex items-center justify-between text-[11px]">
            <span className="text-white/45">Images</span>
            <span className="text-white/85">
              {formatFileCount(selection.files.length)}
            </span>
          </div>
          <div className="mt-1 flex items-center justify-between text-[11px]">
            <span className="text-white/45">Layout</span>
            <span className="text-white/85">
              A4 portrait · fit with margins
            </span>
          </div>
        </div>

        <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
          <p className="text-[10px] uppercase tracking-wide text-white/35">
            Save to
          </p>
          {output ? (
            <>
              <p className="mt-1 truncate text-xs font-medium text-white/85">
                {output.name}
              </p>
              <p className="mt-0.5 truncate text-[10px] text-white/35">
                {output.directory}
              </p>
            </>
          ) : (
            <p className="mt-1 text-xs text-white/45">No destination selected</p>
          )}
        </div>

        {error && (
          <p role="alert" className="text-[11px] text-red-300/90">
            {error}
          </p>
        )}

        <button
          type="button"
          className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:opacity-50"
          disabled={isChoosing}
          onClick={onChooseOutput}
        >
          {isChoosing
            ? "Choosing destination…"
            : output
              ? "Change destination"
              : "Choose destination"}
        </button>

        <button
          type="button"
          disabled
          className="w-full cursor-not-allowed rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2 text-xs text-white/35"
        >
          Convert · coming in M4.1
        </button>

        <div className="flex items-center justify-between text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            onClick={onBack}
          >
            &larr; Review files
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
