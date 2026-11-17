import type { ComponentType } from "react";
import {
  IMAGE_BACKGROUNDS,
  type ImageBackground,
  type ImageFormat,
  type ImageFormatOption,
  type ImageSourceInfo,
} from "../lib/actions";
import { formatBytes } from "../lib/format";

type ImageFormatSettingsProps = {
  source: ImageSourceInfo;
  format: ImageFormat | null;
  quality: number;
  background: ImageBackground;
  categoryIcon?: ComponentType<{ className?: string }>;
  onFormatChange: (format: ImageFormat) => void;
  onQualityChange: (quality: number) => void;
  onBackgroundChange: (background: ImageBackground) => void;
  onBack: () => void;
  onContinue: () => void;
  onDismiss: () => void;
};

function describeFormat(option: ImageFormatOption): string {
  if (option.lossy) {
    return "Smaller files. Quality is adjustable. No transparency.";
  }
  return option.id === "webp"
    ? "Lossless. Keeps transparency. Can be larger than JPEG."
    : "Lossless. Keeps transparency.";
}

export function ImageFormatSettings({
  source,
  format,
  quality,
  background,
  categoryIcon,
  onFormatChange,
  onQualityChange,
  onBackgroundChange,
  onBack,
  onContinue,
  onDismiss,
}: ImageFormatSettingsProps) {
  const CategoryIcon = categoryIcon;
  const selected = source.destinations.find((option) => option.id === format);
  const isJpeg = selected?.lossy === true;
  const flattens = isJpeg && source.hasAlpha;

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
        aria-labelledby="image-format-settings-title"
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
                id="image-format-settings-title"
                className="truncate text-sm font-semibold text-white"
              >
                Conversion settings
              </h2>
              <span className="shrink-0 text-[11px] text-white/40">
                Convert Image
              </span>
            </div>
            <p className="mt-0.5 truncate text-[11px] text-white/45">
              {source.name} · {source.formatLabel} · {source.width}×
              {source.height} · {formatBytes(source.bytes)}
              {source.hasAlpha ? " · transparent" : ""}
            </p>
          </div>
        </header>

        <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
          <p className="text-[10px] uppercase tracking-wide text-white/35">
            Convert to
          </p>
          <div className="mt-1.5 flex flex-col gap-1.5">
            {source.destinations.map((option) => (
              <button
                key={option.id}
                type="button"
                aria-pressed={format === option.id}
                className={`rounded-lg border px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 ${
                  format === option.id
                    ? "border-white/25 bg-white/15 text-white"
                    : "border-white/10 bg-white/[0.03] text-white/60 hover:bg-white/10 hover:text-white"
                }`}
                onClick={() => onFormatChange(option.id)}
              >
                <span className="text-xs font-medium">{option.label}</span>
                <span className="mt-0.5 block text-[10px] text-white/40">
                  {describeFormat(option)}
                </span>
              </button>
            ))}
          </div>
        </div>

        {isJpeg && (
          <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
            <div className="flex items-center justify-between">
              <label
                htmlFor="image-jpeg-quality"
                className="text-[10px] uppercase tracking-wide text-white/35"
              >
                JPEG quality
              </label>
              <span className="text-[11px] text-white/85">{quality}</span>
            </div>
            <input
              id="image-jpeg-quality"
              type="range"
              min={1}
              max={100}
              step={1}
              value={quality}
              className="mt-2 w-full accent-emerald-300"
              onChange={(event) => onQualityChange(Number(event.target.value))}
            />
            <p className="mt-1 text-[10px] text-white/35">
              Higher keeps more detail and produces a larger file. JPEG is
              lossy at every setting.
            </p>
          </div>
        )}

        {flattens && (
          <div className="rounded-xl border border-amber-300/20 bg-amber-300/[0.08] px-3 py-2.5">
            <p className="text-[10px] uppercase tracking-wide text-amber-100/70">
              Background
            </p>
            <div
              className="mt-1.5 flex gap-1.5"
              role="group"
              aria-label="Background for transparent areas"
            >
              {(
                Object.entries(IMAGE_BACKGROUNDS) as Array<
                  [ImageBackground, { label: string; hex: string }]
                >
              ).map(([id, option]) => (
                <button
                  key={id}
                  type="button"
                  aria-pressed={background === id}
                  className={`flex-1 rounded-lg border px-2.5 py-1.5 text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 ${
                    background === id
                      ? "border-white/25 bg-white/15 text-white"
                      : "border-white/10 bg-white/[0.03] text-white/60 hover:bg-white/10 hover:text-white"
                  }`}
                  onClick={() => onBackgroundChange(id)}
                >
                  {option.label}
                </button>
              ))}
            </div>
            <p role="alert" className="mt-2 text-[11px] text-amber-100/90">
              JPEG cannot store transparency. Transparent areas will be filled
              with {IMAGE_BACKGROUNDS[background].label.toLowerCase()}.
            </p>
          </div>
        )}

        {selected && !isJpeg && source.hasAlpha && (
          <p className="rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2 text-[10px] text-white/45">
            Transparency is preserved in {selected.label}.
          </p>
        )}

        {source.orientation !== 1 && (
          <p className="rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2 text-[10px] text-white/45">
            This image carries an EXIF rotation. It will be applied to the
            pixels, so the result displays the right way up everywhere.
          </p>
        )}

        <p className="text-[10px] text-white/35">
          The image is converted at its original size. EXIF data and colour
          profiles are not carried over; wide-gamut images may look slightly
          different.
        </p>

        <p className="text-[10px] text-white/30">
          Limits: {formatBytes(source.maxInputBytes)} input ·{" "}
          {source.maxDimension.toLocaleString()} px per side ·{" "}
          {Math.round(source.maxPixels / 1_000_000)} megapixels.
        </p>

        <button
          type="button"
          className="w-full rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-50"
          disabled={!selected}
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
