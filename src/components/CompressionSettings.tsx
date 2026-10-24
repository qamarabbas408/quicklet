import type { ComponentType } from "react";
import type {
  CompressionPreset,
  CompressionSourceInfo,
} from "../lib/actions";
import { formatBytes } from "../lib/format";

type CompressionSettingsProps = {
  source: CompressionSourceInfo;
  preset: CompressionPreset;
  categoryIcon?: ComponentType<{ className?: string }>;
  onPresetChange: (preset: CompressionPreset) => void;
  onBack: () => void;
  onContinue: () => void;
  onDismiss: () => void;
};

const presets: Array<{
  id: CompressionPreset;
  label: string;
  description: string;
}> = [
  {
    id: "light",
    label: "Light",
    description:
      "Lossless optimization only. No image recompression. May produce little or no reduction.",
  },
  {
    id: "balanced",
    label: "Balanced",
    description:
      "Lossless optimization plus moderate JPEG recompression for compatible images.",
  },
  {
    id: "strong",
    label: "Strong",
    description:
      "Lossless optimization plus stronger JPEG recompression for compatible images.",
  },
];

export function CompressionSettings({
  source,
  preset,
  categoryIcon,
  onPresetChange,
  onBack,
  onContinue,
  onDismiss,
}: CompressionSettingsProps) {
  const CategoryIcon = categoryIcon;
  const selected = presets.find((item) => item.id === preset) ?? presets[1];

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
        aria-labelledby="compression-settings-title"
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
                id="compression-settings-title"
                className="truncate text-sm font-semibold text-white"
              >
                Compression settings
              </h2>
              <span className="shrink-0 text-[11px] text-white/40">
                Compress PDF
              </span>
            </div>
            <p className="mt-0.5 truncate text-[11px] text-white/45">
              {source.name} · {source.pages}{" "}
              {source.pages === 1 ? "page" : "pages"} ·{" "}
              {formatBytes(source.bytes)}
            </p>
          </div>
        </header>

        {source.hasSignature && (
          <p
            role="alert"
            className="rounded-lg border border-amber-300/20 bg-amber-300/[0.08] px-3 py-2 text-[11px] text-amber-100/90"
          >
            This PDF contains a digital signature. Compressing it rewrites the
            file and will invalidate the existing signature.
          </p>
        )}

        <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
          <p className="text-[10px] uppercase tracking-wide text-white/35">
            Preset
          </p>
          <div className="mt-1.5 flex flex-col gap-1.5">
            {presets.map((item) => (
              <button
                key={item.id}
                type="button"
                aria-pressed={preset === item.id}
                className={`rounded-lg border px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 ${
                  preset === item.id
                    ? "border-white/25 bg-white/15 text-white"
                    : "border-white/10 bg-white/[0.03] text-white/60 hover:bg-white/10 hover:text-white"
                }`}
                onClick={() => onPresetChange(item.id)}
              >
                <span className="flex items-center justify-between gap-2">
                  <span className="text-xs font-medium">{item.label}</span>
                  {item.id === "balanced" && (
                    <span className="rounded-full border border-white/10 px-1.5 py-0.5 text-[9px] uppercase tracking-wide text-white/45">
                      Recommended
                    </span>
                  )}
                </span>
                <span className="mt-0.5 block text-[10px] text-white/40">
                  {item.description}
                </span>
              </button>
            ))}
          </div>
        </div>

        {source.images === 0 && (
          <p className="rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2 text-[10px] text-white/45">
            No compatible image objects were detected. Balanced and Strong will
            produce the same result as Light for this PDF.
          </p>
        )}

        {preset === "strong" && (
          <p
            role="status"
            className="rounded-lg border border-red-300/15 bg-red-300/[0.06] px-3 py-2 text-[11px] text-red-100/85"
          >
            Strong may cause visible image-quality loss. Use Balanced if image
            fidelity matters.
          </p>
        )}

        <p className="text-[10px] text-white/35">
          {selected.label} recompresses only compatible 8-bit RGB/grayscale
          JPEG or Flate images. Masks and unsupported images are left unchanged.
          No size reduction is guaranteed; if the result is not smaller, the
          original bytes are retained.
        </p>

        <p className="text-[10px] text-white/30">
          Limits: {source.maxPages.toLocaleString()} pages ·{" "}
          {source.maxImages.toLocaleString()} image objects ·{" "}
          {formatBytes(source.maxInputBytes)} input.
        </p>

        <button
          type="button"
          className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
          onClick={onContinue}
        >
          Choose output
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
