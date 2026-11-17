import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ImageFormatSettings } from "./ImageFormatSettings";
import type { ImageSourceInfo } from "../lib/actions";

const transparentPng: ImageSourceInfo = {
  name: "logo.png",
  format: "png",
  formatLabel: "PNG",
  width: 4000,
  height: 3000,
  bytes: 2_500_000,
  hasAlpha: true,
  orientation: 1,
  destinations: [
    {
      id: "jpeg",
      label: "JPEG",
      extension: "jpg",
      lossy: true,
      supportsAlpha: false,
    },
    {
      id: "webp",
      label: "WebP (Lossless)",
      extension: "webp",
      lossy: false,
      supportsAlpha: true,
    },
  ],
  defaultJpegQuality: 85,
  maxInputBytes: 100 * 1024 * 1024,
  maxDimension: 16_384,
  maxPixels: 50_000_000,
};

function renderSettings(
  overrides: Partial<React.ComponentProps<typeof ImageFormatSettings>> = {},
) {
  const props = {
    source: transparentPng,
    format: null,
    quality: 85,
    background: "white" as const,
    onFormatChange: vi.fn(),
    onQualityChange: vi.fn(),
    onBackgroundChange: vi.fn(),
    onBack: vi.fn(),
    onContinue: vi.fn(),
    onDismiss: vi.fn(),
    ...overrides,
  };
  render(<ImageFormatSettings {...props} />);
  return props;
}

describe("ImageFormatSettings", () => {
  it("offers every format except the source's own", () => {
    const props = renderSettings();

    expect(screen.getByRole("button", { name: /^JPEG/ })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^WebP \(Lossless\)/ }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^PNG/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Choose output" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: /^JPEG/ }));
    expect(props.onFormatChange).toHaveBeenCalledWith("jpeg");
  });

  it("shows quality and background controls for JPEG with a transparent source", () => {
    const props = renderSettings({ format: "jpeg" });

    const slider = screen.getByRole("slider", { name: "JPEG quality" });
    expect(slider).toHaveValue("85");
    fireEvent.change(slider, { target: { value: "60" } });
    expect(props.onQualityChange).toHaveBeenCalledWith(60);

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Transparent areas will be filled with white",
    );
    fireEvent.click(screen.getByRole("button", { name: "Black" }));
    expect(props.onBackgroundChange).toHaveBeenCalledWith("black");
    expect(screen.getByRole("button", { name: "Choose output" })).toBeEnabled();
  });

  it("hides lossy controls for lossless output and continues", () => {
    const props = renderSettings({ format: "webp" });

    expect(screen.queryByRole("slider")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(
      screen.getByText("Transparency is preserved in WebP (Lossless)."),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Choose output" }));
    expect(props.onContinue).toHaveBeenCalledTimes(1);
  });

  it("does not mention a background when the source is opaque", () => {
    renderSettings({
      source: { ...transparentPng, hasAlpha: false, orientation: 6 },
      format: "jpeg",
    });

    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText(/EXIF rotation/)).toBeInTheDocument();
  });
});
