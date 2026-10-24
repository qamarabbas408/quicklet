import { SlidersIcon } from "./icons";

export type SettingsState = {
  defaultOutputDirectory: string | null;
  valid: boolean;
  error: string | null;
};

type SettingsPanelProps = {
  settings: SettingsState | null;
  loading: boolean;
  onBack: () => void;
  onDismiss: () => void;
  onChooseDirectory: () => void;
  onClearDirectory: () => void;
  onReset: () => void;
};

export function SettingsPanel({
  settings,
  loading,
  onBack,
  onDismiss,
  onChooseDirectory,
  onClearDirectory,
  onReset,
}: SettingsPanelProps) {
  const directory = settings?.defaultOutputDirectory ?? null;
  const hasDirectory = Boolean(directory);
  const stateLabel = !settings
    ? "Loading…"
    : settings.valid
      ? "Ready"
      : "Needs attention";
  const stateClass = !settings
    ? "text-white/45"
    : settings.valid
      ? "text-emerald-200/85"
      : "text-amber-200/85";

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
        aria-labelledby="settings-title"
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
            <SlidersIcon className="h-5 w-5" />
          </span>
          <div className="pointer-events-none min-w-0 flex-1">
            <h2
              id="settings-title"
              className="truncate text-sm font-semibold text-white"
            >
              Preferences
            </h2>
            <p className="mt-0.5 text-[11px] text-white/45">
              Default output folder
            </p>
          </div>
        </header>

        {loading && (
          <p className="py-6 text-center text-xs text-white/40">
            Loading preferences…
          </p>
        )}

        {!loading && settings && (
          <div className="space-y-3">
            <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-3">
              <div className="flex items-center justify-between gap-3">
                <span className="text-[10px] uppercase tracking-[0.14em] text-white/45">
                  Status
                </span>
                <span className={`text-[10px] font-medium ${stateClass}`}>
                  {stateLabel}
                </span>
              </div>

              <div className="mt-3 rounded-lg border border-white/8 bg-black/10 p-2.5">
                <p className="text-[10px] uppercase tracking-[0.12em] text-white/40">
                  Preferred output folder
                </p>
                <p
                  className={
                    hasDirectory
                      ? "mt-2 line-clamp-3 text-[11px] break-words text-white/80"
                      : "mt-2 text-[11px] text-white/45"
                  }
                  title={directory ?? undefined}
                >
                  {hasDirectory ? directory : "No folder selected"}
                </p>
              </div>

              {settings.error && !settings.valid && (
                <p className="mt-3 rounded-lg border border-amber-300/15 bg-amber-500/5 px-2.5 py-2 text-[10px] leading-relaxed text-amber-100/85">
                  {settings.error}
                </p>
              )}
            </div>

            <div className="grid gap-2">
              <button
                type="button"
                className="rounded-xl border border-white/10 bg-white/[0.04] px-3 py-2 text-left text-[11px] font-medium text-white/85 transition-colors hover:border-white/20 hover:bg-white/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
                onClick={onChooseDirectory}
              >
                {hasDirectory ? "Choose another folder" : "Choose output folder"}
              </button>

              <button
                type="button"
                className="rounded-xl border border-white/10 bg-white/[0.04] px-3 py-2 text-left text-[11px] font-medium text-white/85 transition-colors hover:border-white/20 hover:bg-white/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
                onClick={onClearDirectory}
                disabled={!hasDirectory}
              >
                Clear preferred folder
              </button>

              <button
                type="button"
                className="rounded-xl border border-red-300/25 bg-red-500/5 px-3 py-2 text-left text-[11px] font-medium text-red-100/85 transition-colors hover:border-red-300/35 hover:bg-red-500/8 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-300/50"
                onClick={onReset}
              >
                Reset all settings
              </button>
            </div>
          </div>
        )}

        {!loading && !settings && (
          <p className="py-6 text-center text-xs text-white/40">
            Unable to load preferences.
          </p>
        )}

        <div className="mt-4 flex items-center justify-between text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            onClick={onBack}
          >
            &larr; Actions
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
