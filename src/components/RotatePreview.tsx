import {
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import type { ComponentType } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  RotatePreviewBatch,
  RotateSourceInfo,
} from "../lib/actions";
import { formatBytes } from "../lib/format";

type RotatePreviewProps = {
  source: RotateSourceInfo;
  selectedPages: Set<number>;
  rotations: Record<number, number>;
  categoryIcon?: ComponentType<{ className?: string }>;
  onTogglePage: (page: number) => void;
  onSelectAll: () => void;
  onClearSelection: () => void;
  onRotateSelected: () => void;
  onBack: () => void;
  onReview: () => void;
  onDismiss: () => void;
};

type CachedThumbnail = {
  url: string;
  width: number;
  height: number;
  turns: number;
};

type QueueItem = {
  page: number;
  turns: number;
  generation: number;
};

const THUMBNAIL_WIDTH = 240;
const THUMBNAIL_HEIGHT = 320;
const MAX_CONCURRENT_RENDERS = 2;
const MAX_CACHE_ENTRIES = 60;
const COLUMNS = 3;

function base64PngToObjectUrl(base64: string): string {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
}

export function RotatePreview({
  source,
  selectedPages,
  rotations,
  categoryIcon,
  onTogglePage,
  onSelectAll,
  onClearSelection,
  onRotateSelected,
  onBack,
  onReview,
  onDismiss,
}: RotatePreviewProps) {
  const CategoryIcon = categoryIcon;
  const scrollRef = useRef<HTMLDivElement>(null);
  const cacheRef = useRef(new Map<string, CachedThumbnail>());
  const pendingRef = useRef(new Set<string>());
  const queueRef = useRef<QueueItem[]>([]);
  const activeRef = useRef(0);
  const generationRef = useRef(0);
  const visibleRef = useRef(new Set<number>());
  const rotationsRef = useRef(rotations);
  const pumpRef = useRef<() => void>(() => {});
  const [, setVersion] = useState(0);
  const [errors, setErrors] = useState<Record<number, string>>({});
  const [focusPage, setFocusPage] = useState(1);

  rotationsRef.current = rotations;

  const clearCache = useCallback(() => {
    for (const entry of cacheRef.current.values()) {
      URL.revokeObjectURL(entry.url);
    }
    cacheRef.current.clear();
    pendingRef.current.clear();
    queueRef.current = [];
    visibleRef.current.clear();
  }, []);

  const trimCache = useCallback(() => {
    if (cacheRef.current.size <= MAX_CACHE_ENTRIES) return;
    for (const [key, entry] of cacheRef.current) {
      const page = Number(key.split(":")[0]);
      if (visibleRef.current.has(page)) continue;
      URL.revokeObjectURL(entry.url);
      cacheRef.current.delete(key);
      if (cacheRef.current.size <= MAX_CACHE_ENTRIES) break;
    }
  }, []);

  const requestThumbnail = useCallback((page: number, force = false) => {
    const turns = rotationsRef.current[page] ?? 0;
    const key = `${page}:${turns}`;
    const generation = generationRef.current;
    const pendingKey = `${generation}:${key}`;
    if (
      !force &&
      (cacheRef.current.has(key) || pendingRef.current.has(pendingKey))
    ) {
      return;
    }
    if (force) {
      const existing = cacheRef.current.get(key);
      if (existing) {
        URL.revokeObjectURL(existing.url);
        cacheRef.current.delete(key);
      }
    }

    setErrors((current) => {
      if (!(page in current)) return current;
      const next = { ...current };
      delete next[page];
      return next;
    });

    pendingRef.current.add(pendingKey);
    queueRef.current.push({
      page,
      turns,
      generation,
    });
    pumpRef.current();
  }, []);

  pumpRef.current = () => {
    while (
      activeRef.current < MAX_CONCURRENT_RENDERS &&
      queueRef.current.length > 0
    ) {
      const item = queueRef.current.shift();
      if (!item) return;
      const key = `${item.page}:${item.turns}`;
      const pendingKey = `${item.generation}:${key}`;

      if (item.generation !== generationRef.current) {
        pendingRef.current.delete(pendingKey);
        continue;
      }

      activeRef.current += 1;
      void invoke<RotatePreviewBatch>("render_pdf_thumbnails", {
        requests: [{ page: item.page, turns: item.turns }],
        targetWidth: THUMBNAIL_WIDTH,
        targetHeight: THUMBNAIL_HEIGHT,
      })
        .then((batch) => {
          if (item.generation !== generationRef.current) return;
          const thumbnail = batch.thumbnails[0];
          if (!thumbnail) {
            throw new Error("The page preview was empty.");
          }
          const url = base64PngToObjectUrl(thumbnail.pngBase64);
          cacheRef.current.set(key, {
            url,
            width: thumbnail.width,
            height: thumbnail.height,
            turns: item.turns,
          });
          trimCache();
          setVersion((value) => value + 1);
        })
        .catch((error) => {
          if (item.generation !== generationRef.current) return;
          setErrors((current) => ({
            ...current,
            [item.page]: String(error),
          }));
        })
        .finally(() => {
          pendingRef.current.delete(pendingKey);
          activeRef.current = Math.max(0, activeRef.current - 1);
          pumpRef.current();
        });
    }
  };

  useEffect(() => {
    generationRef.current += 1;
    clearCache();
    setErrors({});
    setFocusPage(1);
    setVersion((value) => value + 1);

    return () => {
      generationRef.current += 1;
      clearCache();
    };
  }, [source, clearCache]);

  useEffect(() => {
    for (const page of visibleRef.current) {
      requestThumbnail(page);
    }
  }, [rotations, requestThumbnail]);

  useEffect(() => {
    const root = scrollRef.current;
    if (!root) return;

    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const element = entry.target as HTMLElement;
          const page = Number(element.dataset.page);
          if (!Number.isInteger(page) || page < 1) continue;
          if (entry.isIntersecting) {
            visibleRef.current.add(page);
            requestThumbnail(page);
          } else {
            visibleRef.current.delete(page);
          }
        }
      },
      { root, rootMargin: "160px", threshold: 0.01 },
    );

    const tiles = root.querySelectorAll<HTMLElement>("[data-page]");
    for (const tile of tiles) {
      observer.observe(tile);
    }

    return () => observer.disconnect();
  }, [requestThumbnail, source.pages]);

  const rotatedCount = Object.keys(rotations).length;
  const selectedCount = selectedPages.size;
  const canReview = rotatedCount > 0;

  const focusPageButton = (page: number) => {
    const next = Math.min(Math.max(1, page), source.pages);
    setFocusPage(next);
    scrollRef.current
      ?.querySelector<HTMLElement>(`[data-page="${next}"]`)
      ?.focus();
  };

  const handleGridKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const page = Number(target.closest<HTMLElement>("[data-page]")?.dataset.page);
    if (!Number.isInteger(page) || page < 1) return;

    let next = page;
    if (event.key === "ArrowLeft") next = page - 1;
    else if (event.key === "ArrowRight") next = page + 1;
    else if (event.key === "ArrowUp") next = page - COLUMNS;
    else if (event.key === "ArrowDown") next = page + COLUMNS;
    else if (event.key === "Home") next = 1;
    else if (event.key === "End") next = source.pages;
    else return;

    event.preventDefault();
    focusPageButton(next);
  };

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
        className="confirmation-panel rotate-preview-panel"
        aria-labelledby="rotate-preview-title"
      >
        <div
          className="flex h-4 shrink-0 cursor-grab items-center justify-center active:cursor-grabbing"
          data-tauri-drag-region
          title="Drag to move Quicklet"
        >
          <span className="pointer-events-none h-1 w-12 rounded-full bg-white/25" />
        </div>

        <header
          className="flex shrink-0 cursor-grab items-start gap-3 active:cursor-grabbing"
          data-tauri-drag-region
          title="Drag to move Quicklet"
        >
          <span className="confirmation-icon pointer-events-none">
            {CategoryIcon ? <CategoryIcon className="h-5 w-5" /> : null}
          </span>
          <div className="pointer-events-none min-w-0 flex-1">
            <div className="flex items-baseline justify-between gap-2">
              <h2
                id="rotate-preview-title"
                className="truncate text-sm font-semibold text-white"
              >
                Preview pages
              </h2>
              <span className="shrink-0 text-[11px] text-white/40">
                Rotate Pages
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
            className="shrink-0 rounded-lg border border-amber-300/20 bg-amber-300/[0.08] px-2.5 py-1.5 text-[10px] text-amber-100/90"
          >
            Saving this rotated PDF will invalidate its existing digital
            signature.
          </p>
        )}

        <div className="flex shrink-0 flex-wrap items-center gap-1.5">
          <button
            type="button"
            className="rounded-lg border border-white/10 bg-white/[0.04] px-2 py-1.5 text-[10px] text-white/70 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40"
            onClick={onSelectAll}
          >
            Select all
          </button>
          <button
            type="button"
            className="rounded-lg border border-white/10 bg-white/[0.04] px-2 py-1.5 text-[10px] text-white/70 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:opacity-40"
            disabled={selectedCount === 0}
            onClick={onClearSelection}
          >
            Clear
          </button>
          <button
            type="button"
            className="rounded-lg border border-white/15 bg-white/10 px-2 py-1.5 text-[10px] font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-40"
            disabled={selectedCount === 0}
            onClick={onRotateSelected}
          >
            Rotate 90° clockwise
          </button>
          <span className="ml-auto text-[10px] text-white/35">
            {selectedCount} selected · {rotatedCount} rotated
          </span>
        </div>

        {source.pages === 0 ? (
          <div className="flex flex-1 items-center justify-center text-xs text-white/45">
            This PDF has no pages.
          </div>
        ) : (
          <div
            ref={scrollRef}
            className="rotate-preview-grid"
            aria-label="PDF page thumbnails"
            onKeyDown={handleGridKeyDown}
          >
            {Array.from({ length: source.pages }, (_, index) => index + 1).map(
              (page) => {
                const turns = rotations[page] ?? 0;
                const sourceRotation = source.rotations[page - 1] ?? 0;
                const effectiveRotation = (sourceRotation + turns * 90) % 360;
                const key = `${page}:${turns}`;
                const cached = cacheRef.current.get(key);
                const error = errors[page];
                const isSelected = selectedPages.has(page);

                return (
                  <div
                    key={page}
                    className={`rotate-page${isSelected ? " is-selected" : ""}`}
                    data-page={page}
                  >
                    <button
                      type="button"
                      tabIndex={focusPage === page ? 0 : -1}
                      aria-label={`Page ${page}, ${effectiveRotation} degrees${isSelected ? ", selected" : ""}`}
                      aria-pressed={isSelected}
                      className="rotate-page-select"
                      onFocus={() => setFocusPage(page)}
                      onClick={() => onTogglePage(page)}
                    >
                      <span className="rotate-page-image">
                        {cached ? (
                          <img
                            src={cached.url}
                            alt=""
                            draggable={false}
                            width={cached.width}
                            height={cached.height}
                          />
                        ) : error ? (
                          <span className="text-[10px] text-red-200/80">
                            Preview failed
                          </span>
                        ) : (
                          <span className="conversion-spinner" />
                        )}
                      </span>
                      <span className="rotate-page-footer">
                        <span>Page {page}</span>
                        <span className="rotate-page-badge">
                          {effectiveRotation}°
                        </span>
                      </span>
                    </button>
                    {error && (
                      <button
                        type="button"
                        className="rotate-page-retry"
                        onClick={() => requestThumbnail(page, true)}
                      >
                        Retry
                      </button>
                    )}
                  </div>
                );
              },
            )}
          </div>
        )}

        <div className="flex shrink-0 items-center justify-between gap-2 text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            onClick={onBack}
          >
            &larr; Back to files
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>

        <button
          type="button"
          className="w-full shrink-0 rounded-lg border border-white/15 bg-white/10 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 disabled:cursor-not-allowed disabled:opacity-40"
          disabled={!canReview}
          onClick={onReview}
        >
          {canReview ? "Review and save" : "Rotate at least one page"}
        </button>
      </section>
    </div>
  );
}
