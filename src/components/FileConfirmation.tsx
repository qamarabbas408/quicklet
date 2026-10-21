import { actions } from "../lib/actions";
import { formatBytes, formatFileCount } from "../lib/format";
import type { InputSelection } from "../lib/input";
import { CloseIcon } from "./icons";

type FileConfirmationProps = {
  selection: InputSelection;
  isPicking: boolean;
  isDropMode: boolean;
  isDragActive: boolean;
  dragCount: number;
  error: string | null;
  onRemove: (id: number) => void;
  onAddMore: () => void;
  onReplace: () => void;
  onClearAll: () => void;
  onBack: () => void;
  onEnterDrop: () => void;
  onCancelDrop: () => void;
  onDismiss: () => void;
};

export function FileConfirmation({
  selection,
  isPicking,
  isDropMode,
  isDragActive,
  dragCount,
  error,
  onRemove,
  onAddMore,
  onReplace,
  onClearAll,
  onBack,
  onEnterDrop,
  onCancelDrop,
  onDismiss,
}: FileConfirmationProps) {
  const action = actions.find((item) => item.id === selection.actionId);
  const ActionIcon = action?.icon;
  const totalSize = selection.files.reduce(
    (sum, file) => sum + (file.size ?? 0),
    0,
  );
  const hasKnownSize = selection.files.some((file) => file.size !== null);
  const atLimit = selection.files.length >= selection.limit;

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
        aria-labelledby="confirmation-title"
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
            {ActionIcon ? <ActionIcon className="h-5 w-5" /> : null}
          </span>
          <div className="pointer-events-none min-w-0 flex-1">
            <div className="flex items-baseline justify-between gap-2">
              <h2
                id="confirmation-title"
                className="truncate text-sm font-semibold text-white"
              >
                Selected files
              </h2>
              <span className="shrink-0 text-[11px] text-white/40">
                {action?.label ?? "Input"}
              </span>
            </div>
            <p className="mt-0.5 text-[11px] text-white/45">
              {formatFileCount(selection.files.length)}
              {hasKnownSize ? ` · ${formatBytes(totalSize)}` : ""}
              {` · max ${selection.limit}`}
            </p>
          </div>
        </header>

        <ul className="confirmation-list" aria-label="Selected files">
          {selection.files.map((file) => (
            <li
              key={file.id}
              className="flex items-center gap-2 rounded-lg border border-transparent px-2 py-1.5 transition-colors hover:border-white/10 hover:bg-white/[0.04]"
            >
              <div className="min-w-0 flex-1">
                <p
                  className="truncate text-xs font-medium text-white/85"
                  title={file.name}
                >
                  {file.name}
                </p>
                <p className="truncate text-[10px] text-white/40">
                  {file.kind}
                  {file.size !== null ? ` · ${formatBytes(file.size)}` : ""}
                </p>
              </div>
              <button
                type="button"
                aria-label={`Remove ${file.name}`}
                className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-white/35 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:opacity-40"
                disabled={isPicking}
                onClick={() => onRemove(file.id)}
              >
                <CloseIcon className="h-3.5 w-3.5" />
              </button>
            </li>
          ))}
        </ul>

        <p className="text-[10px] text-emerald-300/70">
          Input ready · No conversion started
        </p>

        {selection.skipped > 0 && (
          <p role="status" className="text-[10px] text-amber-200/80">
            {selection.skipped}{" "}
            {selection.skipped === 1 ? "file was" : "files were"} not added.
          </p>
        )}

        {selection.rejected > 0 && (
          <p role="status" className="text-[10px] text-amber-200/80">
            {selection.rejected}{" "}
            {selection.rejected === 1 ? "file was" : "files were"} rejected for
            this action.
          </p>
        )}

        {error && (
          <p role="alert" className="text-[11px] text-red-300/90">
            {error}
          </p>
        )}

        <div className="flex gap-2">
          <button
            type="button"
            className="flex-1 whitespace-nowrap rounded-lg border border-white/15 bg-white/10 px-2 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={isPicking || atLimit}
            onClick={onAddMore}
          >
            {isPicking ? "Opening picker…" : "Add more files"}
          </button>
          <button
            type="button"
            className="flex-1 whitespace-nowrap rounded-lg border border-white/10 bg-white/[0.04] px-2 py-2 text-xs text-white/70 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={isPicking || atLimit}
            onClick={onEnterDrop}
          >
            Drop files
          </button>
          <button
            type="button"
            className="flex-1 whitespace-nowrap rounded-lg border border-white/10 bg-white/[0.04] px-2 py-2 text-xs text-white/70 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:opacity-50"
            disabled={isPicking}
            onClick={onReplace}
          >
            Replace files
          </button>
        </div>

        {atLimit && (
          <p className="text-[10px] text-white/35">
            Limit reached ({selection.limit}). Remove a file to add more.
          </p>
        )}

        <div className="flex items-center justify-between text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            disabled={isPicking}
            onClick={onBack}
          >
            &larr; Back to menu
          </button>
          <div className="flex items-center gap-3">
            <button
              type="button"
              className="text-white/45 transition-colors hover:text-red-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-200/40 disabled:opacity-40"
              disabled={isPicking}
              onClick={onClearAll}
            >
              Clear all
            </button>
            <span className="text-white/30">Esc to dismiss</span>
          </div>
        </div>

        {isDropMode && (
          <div
            className={`absolute inset-0 z-20 flex cursor-grab flex-col items-center justify-center gap-2 rounded-[22px] border-2 border-dashed px-6 text-center backdrop-blur-md transition-colors active:cursor-grabbing ${
              isDragActive
                ? "border-emerald-300/70 bg-emerald-300/10"
                : "border-white/25 bg-[#0c0f15]/90"
            }`}
            data-tauri-drag-region
            title="Drag to move Quicklet"
          >
            <p className="pointer-events-none text-sm font-medium text-white/90">
              {isDragActive
                ? `${dragCount} ${dragCount === 1 ? "file" : "files"} detected`
                : "Drop files here"}
            </p>
            <p className="pointer-events-none text-[11px] text-white/45">
              Files will be added to this selection · max {selection.limit}
            </p>
            <button
              type="button"
              className="mt-2 rounded-lg border border-white/15 bg-white/10 px-3 py-1.5 text-[11px] text-white/80 transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
              onClick={onCancelDrop}
            >
              Cancel drop
            </button>
          </div>
        )}
      </section>
    </div>
  );
}
