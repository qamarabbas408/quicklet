import type { ComponentType } from "react";
import type { ImageCompressionSourceInfo } from "../lib/actions";
import { formatBytes } from "../lib/format";

type ImageCompressionSettingsProps = {
  source: ImageCompressionSourceInfo;
  quality: number;
  categoryIcon?: ComponentType<{ className?: string }>;
  onQualityChange: (quality: number) => void;
  onBack: () => void;
  onContinue: () => void;
  onDismiss: () => void;
};

export function ImageCompressionSettings({
  source,
  quality,
  categoryIcon,
  onQualityChange,
  onBack,
  onContinue,
  onDismiss,
}: ImageCompressionSettingsProps) {
  const CategoryIcon = categoryIcon;
  const isJpeg = source.format === "jpeg";
  const isPng = source.format === "png";
  const losslessLabel = isPng ? "PNG lossless" : "WebP lossless";

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
        aria-labelledby="image-compression-settings-title"
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
                id="image-compression-settings-title"
                className="truncate text-sm font-semibold text-white"
              >
                Compression settings
              </h2>
              <span className="shrink-0 text-[11px] text-white/40">
                Compress Image
              </span>
            </div>
            <p className="mt-0.5 truncate text-[11px] text-white/45">
              {source.name} · {source.formatLabel} · {source.width}×
              {source.height} · {formatBytes(source.bytes)}
            </p>
          </div>
        </header>

        <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
          <div className="flex items-center justify-between text-[11px]">
            <span className="text-white/45">Format</span>
            <span className="text-white/85">{source.formatLabel} (unchanged)</span>
          </div>
          <div className="mt-1 flex items-center justify-between text-[11px]">
            <span className="text-white/45">Dimensions</span>
            <span className="text-white/85">
              {source.width}×{source.height} (unchanged)
            </span>
          </div>
          <div className="mt-1 flex items-center justify-between text-[11px]">
            <span className="text-white/45">Method</span>
            <span className="text-white/85">
              {isJpeg ? `JPEG quality ${quality}` : losslessLabel}
            </span>
          </div>
          <div className="mt-1 flex items-center justify-between text-[11px]">
            <span className="text-white/45">Transparency</span>
            <span className="text-white/85">
              {source.hasAlpha ? "Preserved" : "None"}
            </span>
          </div>
        </div>

        {isJpeg ? (
          <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
            <div className="flex items-center justify-between">
              <label
                htmlFor="image-compression-quality"
                className="text-[10px] uppercase tracking-wide text-white/35"
              >
                JPEG quality
              </label>
              <span className="text-[11px] text-white/85">{quality}</span>
            </div>
            <input
              id="image-compression-quality"
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
              lossy at every setting. Compression may not reduce every image.
            </p>
          </div>
        ) : (
          <div className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
            <p className="text-xs font-medium text-white/85">{losslessLabel}</p>
            <p className="mt-1 text-[10px] text-white/40">
              Pixels are preserved exactly. If re-encoding does not produce a
              smaller file, the original bytes are copied instead. Compression
              may not reduce every image.
            </p>
            {isPng && (
              <p className="mt-2 text-[10px] text-amber-100/80">
                Indexed/palette PNGs are expanded to RGB or RGBA during
                recompression, so some PNGs may not shrink.
              </p>
            )}
          </div>
        )}

        <p className="rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2 text-[10px] leading-relaxed text-white/45">
          ICC and EXIF metadata are preserved. If a smaller file is produced,
          XMP, IPTC, PNG text, gamma, chromaticity, and other unsupported
          ancillary metadata are removed. If no smaller file is produced, the
          original bytes are copied and retain all metadata.
        </p>

        {(source.metadata.hasXmp || source.metadata.hasIptc) && (
          <p
            role="alert"
            className="rounded-lg border border-amber-300/20 bg-amber-300/[0.08] px-3 py-2 text-[10px] leading-relaxed text-amber-100/90"
          >
            This image contains {[
              source.metadata.hasXmp ? "XMP" : null,
              source.metadata.hasIptc ? "IPTC" : null,
            ]
              .filter(Boolean)
              .join(" and ")}{" "}
            metadata. It cannot be preserved if a smaller file is produced;
            otherwise the original bytes are copied.
          </p>
        )}

        {source.orientation !== 1 && (
          <p className="rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2 text-[10px] text-white/45">
            EXIF orientation is preserved in metadata. Stored dimensions and
            pixel orientation are unchanged.
          </p>
        )}

        <p className="text-[10px] text-white/30">
          Limits: {formatBytes(source.maxInputBytes)} input ·{" "}
          {source.maxDimension.toLocaleString()} px per side ·{" "}
          {Math.round(source.maxPixels / 1_000_000)} megapixels.
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
            &larr; Files
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
