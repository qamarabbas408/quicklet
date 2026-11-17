import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ImageCompressionSettings } from "./ImageCompressionSettings";
import type { ImageCompressionSourceInfo } from "../lib/actions";

const jpegSource: ImageCompressionSourceInfo = {
  name: "photo.jpg",
  format: "jpeg",
  formatLabel: "JPEG",
  width: 4000,
  height: 3000,
  bytes: 3_500_000,
  hasAlpha: false,
  orientation: 6,
  metadata: {
    hasIcc: true,
    hasExif: true,
    hasXmp: true,
    hasIptc: false,
    preservedBytes: 20_000,
  },
  defaultJpegQuality: 85,
  maxInputBytes: 100 * 1024 * 1024,
  maxDimension: 16_384,
  maxPixels: 50_000_000,
};

function renderSettings(
  overrides: Partial<React.ComponentProps<typeof ImageCompressionSettings>> = {},
) {
  const props = {
    source: jpegSource,
    quality: 85,
    onQualityChange: vi.fn(),
    onBack: vi.fn(),
    onContinue: vi.fn(),
    onDismiss: vi.fn(),
    ...overrides,
  };
  render(<ImageCompressionSettings {...props} />);
  return props;
}

describe("ImageCompressionSettings", () => {
  it("shows JPEG quality, metadata disclosure and orientation note", () => {
    const props = renderSettings();

    expect(screen.getByText("JPEG (unchanged)")).toBeInTheDocument();
    expect(screen.getByText("4000×3000 (unchanged)")).toBeInTheDocument();
    expect(screen.getByText("JPEG quality 85")).toBeInTheDocument();
    expect(
      screen.getByText(/ICC and EXIF metadata are preserved/),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "This image contains XMP metadata",
    );
    expect(
      screen.getByText(/EXIF orientation is preserved/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Compression may not reduce every image/),
    ).toBeInTheDocument();

    const slider = screen.getByRole("slider", { name: "JPEG quality" });
    fireEvent.change(slider, { target: { value: "60" } });
    expect(props.onQualityChange).toHaveBeenCalledWith(60);
  });

  it("discloses both XMP and IPTC metadata before compression", () => {
    renderSettings({
      source: {
        ...jpegSource,
        metadata: {
          ...jpegSource.metadata,
          hasIptc: true,
        },
      },
    });

    expect(screen.getByRole("alert")).toHaveTextContent("XMP and IPTC");
    expect(screen.getByRole("alert")).toHaveTextContent(
      "cannot be preserved if a smaller file is produced",
    );
  });

  it("shows lossless and indexed-PNG guidance without a quality slider", () => {
    const props = renderSettings({
      source: {
        ...jpegSource,
        name: "logo.png",
        format: "png",
        formatLabel: "PNG",
        hasAlpha: true,
        orientation: 1,
        metadata: {
          hasIcc: false,
          hasExif: false,
          hasXmp: false,
          hasIptc: false,
          preservedBytes: 0,
        },
      },
    });

    expect(screen.queryByRole("slider")).toBeNull();
    expect(screen.getAllByText("PNG lossless").length).toBeGreaterThan(0);
    expect(screen.getByText(/Indexed\/palette PNGs/)).toBeInTheDocument();
    expect(screen.getByText("Transparency")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Choose output" }));
    expect(props.onContinue).toHaveBeenCalledTimes(1);
  });

  it("shows WebP lossless without JPEG controls", () => {
    renderSettings({
      source: {
        ...jpegSource,
        name: "photo.webp",
        format: "webp",
        formatLabel: "WebP",
        metadata: {
          hasIcc: false,
          hasExif: false,
          hasXmp: false,
          hasIptc: false,
          preservedBytes: 0,
        },
      },
    });

    expect(screen.queryByRole("slider")).toBeNull();
    expect(screen.getAllByText("WebP lossless").length).toBeGreaterThan(0);
    expect(screen.queryByText(/Indexed\/palette PNGs/)).toBeNull();
  });
});
