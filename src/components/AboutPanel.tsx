import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  APP_COMPANY,
  APP_DEVELOPER,
  APP_NAME,
  APP_VERSION,
  APP_WEBSITE,
  IS_TAURI,
} from "../lib/constants";
import { InfoIcon, QuickletMark } from "./icons";

type AboutPanelProps = {
  onBack: () => void;
  onDismiss: () => void;
};

export function AboutPanel({ onBack, onDismiss }: AboutPanelProps) {
  const [linkError, setLinkError] = useState<string | null>(null);

  const openWebsite = async () => {
    setLinkError(null);
    try {
      if (IS_TAURI) {
        await openUrl(APP_WEBSITE);
      } else {
        window.open(APP_WEBSITE, "_blank", "noopener,noreferrer");
      }
    } catch (error) {
      setLinkError(String(error));
    }
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
        className="confirmation-panel relative"
        aria-labelledby="about-title"
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
            <InfoIcon className="h-5 w-5" />
          </span>
          <div className="pointer-events-none min-w-0 flex-1">
            <h2 id="about-title" className="text-sm font-semibold text-white">
              About
            </h2>
            <p className="mt-0.5 text-[11px] text-white/45">
              Product information
            </p>
          </div>
        </header>

        <div className="about-hero">
          <span className="about-logo">
            <QuickletMark className="h-14 w-14" />
          </span>
          <h3 className="mt-3 text-lg font-semibold tracking-tight text-white">
            {APP_NAME}
          </h3>
          <p className="mt-0.5 text-[11px] font-medium text-white/50">
            Version {APP_VERSION}
          </p>
          <p className="mt-2 max-w-[280px] text-[11px] leading-relaxed text-white/45">
            Desktop utility for file conversion and PDF operations.
          </p>
        </div>

        <dl className="about-details">
          <div className="about-detail-row">
            <dt>Developed by</dt>
            <dd>{APP_DEVELOPER}</dd>
          </div>
          <div className="about-detail-row">
            <dt>Product of</dt>
            <dd>{APP_COMPANY}</dd>
          </div>
          <div className="about-detail-row">
            <dt>Website</dt>
            <dd>
              <button
                type="button"
                className="about-link"
                onClick={() => void openWebsite()}
              >
                {APP_WEBSITE.replace(/^https?:\/\//, "")}
              </button>
            </dd>
          </div>
        </dl>

        {linkError && (
          <p
            role="alert"
            className="max-h-16 overflow-y-auto text-center text-[10px] break-words text-red-300/90 [overflow-wrap:anywhere]"
          >
            {linkError}
          </p>
        )}

        <div className="flex items-center justify-between text-[10px]">
          <button
            type="button"
            className="text-white/45 transition-colors hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            onClick={onBack}
          >
            &larr; Settings
          </button>
          <span className="text-white/30">Esc to dismiss</span>
        </div>
      </section>
    </div>
  );
}
