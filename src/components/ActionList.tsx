import type { ActionInfo, RadialAction } from "../lib/actions";
import { ChevronRightIcon } from "./icons";

type ActionListProps = {
  category: RadialAction;
  actions: ActionInfo[];
  loading: boolean;
  error: string | null;
  onSelect: (action: ActionInfo) => void;
  onBack: () => void;
  onDismiss: () => void;
};

export function ActionList({
  category,
  actions,
  loading,
  error,
  onSelect,
  onBack,
  onDismiss,
}: ActionListProps) {
  const CategoryIcon = category.icon;

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
        aria-labelledby="action-list-title"
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
            <CategoryIcon className="h-5 w-5" />
          </span>
          <div className="pointer-events-none min-w-0 flex-1">
            <h2
              id="action-list-title"
              className="truncate text-sm font-semibold text-white"
            >
              {category.label}
            </h2>
            <p className="mt-0.5 text-[11px] text-white/45">
              Choose an action
            </p>
          </div>
        </header>

        {loading && (
          <p className="py-6 text-center text-xs text-white/40">
            Loading actions…
          </p>
        )}

        {!loading && error && (
          <p role="alert" className="py-4 text-center text-xs text-red-300/90">
            {error}
          </p>
        )}

        {!loading && !error && (
          <div className="action-list">
            {actions.map((action) =>
              action.enabled ? (
                <button
                  key={action.id}
                  type="button"
                  className="group flex w-full items-center gap-3 rounded-xl border border-white/10 bg-white/[0.04] px-3 py-2.5 text-left transition-colors hover:border-white/20 hover:bg-white/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
                  onClick={() => onSelect(action)}
                >
                  <span className="min-w-0 flex-1">
                    <span className="block text-xs font-medium text-white/90">
                      {action.label}
                    </span>
                    <span className="mt-0.5 block text-[10px] text-white/40">
                      {action.description}
                    </span>
                  </span>
                  <ChevronRightIcon className="h-4 w-4 shrink-0 text-white/35 transition-colors group-hover:text-white/70" />
                </button>
              ) : (
                <div
                  key={action.id}
                  aria-disabled="true"
                  className="flex w-full items-center gap-3 rounded-xl border border-white/5 bg-white/[0.02] px-3 py-2.5 opacity-55"
                >
                  <span className="min-w-0 flex-1">
                    <span className="block text-xs font-medium text-white/70">
                      {action.label}
                    </span>
                    <span className="mt-0.5 block text-[10px] text-white/35">
                      {action.description}
                    </span>
                  </span>
                  <span className="shrink-0 rounded-full border border-white/10 px-2 py-0.5 text-[9px] uppercase tracking-wide text-white/40">
                    Soon
                  </span>
                </div>
              ),
            )}
          </div>
        )}

        <div className="flex items-center justify-between text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            onClick={onBack}
          >
            &larr; Launcher
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
