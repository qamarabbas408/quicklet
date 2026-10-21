import { actions } from "../lib/actions";
import { formatBytes, formatFileCount } from "../lib/format";
import type { InputSelection } from "../lib/input";
import { CloseIcon } from "./icons";

type FileConfirmationProps = {
  selection: InputSelection;
  isPicking: boolean;
  error: string | null;
  onRemove: (id: number) => void;
  onAddMore: () => void;
  onReplace: () => void;
  onBack: () => void;
  onDismiss: () => void;
};

export function FileConfirmation({
  selection,
  isPicking,
  error,
  onRemove,
  onAddMore,
  onReplace,
  onBack,
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
        className="confirmation-panel"
        aria-labelledby="confirmation-title"
      >
        <header className="flex items-start gap-3">
          <span className="confirmation-icon">
            {ActionIcon ? <ActionIcon className="h-5 w-5" /> : null}
          </span>
          <div className="min-w-0 flex-1">
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

        {error && (
          <p role="alert" className="text-[11px] text-red-300/90">
            {error}
          </p>
        )}

        <div className="flex gap-2">
          <button
            type="button"
            className="flex-1 rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={isPicking || atLimit}
            onClick={onAddMore}
          >
            {isPicking
              ? "Opening picker…"
              : atLimit
                ? `Limit reached (${selection.limit})`
                : "Add more files"}
          </button>
          <button
            type="button"
            className="rounded-lg border border-white/10 bg-white/[0.04] px-3 py-2 text-xs text-white/70 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:opacity-50"
            disabled={isPicking}
            onClick={onReplace}
          >
            Replace files
          </button>
        </div>

        <div className="flex items-center justify-between text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            disabled={isPicking}
            onClick={onBack}
          >
            &larr; Back to menu
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
