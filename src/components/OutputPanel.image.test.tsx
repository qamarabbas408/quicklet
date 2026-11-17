import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { OutputPanel } from "./OutputPanel";
import type {
  ConversionResult,
  ImageSourceInfo,
  OperationProgress,
} from "../lib/actions";

const source: ImageSourceInfo = {
  name: "photo.png",
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

function renderPanel(
  overrides: Partial<React.ComponentProps<typeof OutputPanel>> = {},
) {
  const props = {
    category: null,
    actionLabel: "Convert Image",
    selection: {
      actionId: "images.convert_format",
      files: [
        { id: 1, name: "photo.png", extension: "png", kind: "PNG image", size: 2_500_000 },
      ],
      limit: 1,
      duplicates: 0,
      limitSkipped: 0,
      rejected: 0,
    },
    output: { name: "photo.jpg", directory: "/Users/me/Pictures" },
    outputDirectory: null,
    isChoosing: false,
    isConverting: false,
    isMerge: false,
    isSplit: false,
    isCompress: false,
    isImageConvert: true,
    isImageCompress: false,
    imageSource: source,
    imageFormat: "jpeg" as const,
    imageQuality: 85,
    imageBackground: "white" as const,
    imageCompressionSource: null,
    imageCompressionQuality: 85,
    compressionPreset: "balanced" as const,
    compressSource: null,
    pageSelectionMode: null,
    pagePlan: null,
    isRotate: false,
    rotateSource: null,
    rotateTurns: {},
    splitMode: "every" as const,
    splitPlan: null,
    progress: null,
    result: null,
    error: null,
    resultError: null,
    isCancellingOperation: false,
    onChooseOutput: vi.fn(),
    onConvert: vi.fn(),
    onCancelOperation: vi.fn(),
    onReveal: vi.fn(),
    onOpen: vi.fn(),
    onOpenFolder: vi.fn(),
    onBack: vi.fn(),
    onDismiss: vi.fn(),
    ...overrides,
  };
  render(<OutputPanel {...props} />);
  return props;
}

const running: OperationProgress = {
  operationId: "images.convert_format:1",
  status: "running",
  stage: "encoding",
  message: "Encoding image…",
  determinate: false,
};

describe("OutputPanel for image conversion", () => {
  it("summarises the conversion and starts it", () => {
    const props = renderPanel();

    expect(screen.getByText("4000×3000 (unchanged)")).toBeInTheDocument();
    expect(screen.getByText("Filled with white")).toBeInTheDocument();
    expect(screen.getByText("85")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Convert Image" }));
    expect(props.onConvert).toHaveBeenCalledTimes(1);
  });

  it("shows dimensions after EXIF rotation and returns to conversion settings", () => {
    renderPanel({
      imageSource: { ...source, orientation: 6 },
    });

    expect(
      screen.getByText("3000×4000 after EXIF rotation"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Conversion settings/ }),
    ).toBeInTheDocument();
  });

  it("shows live progress without a percentage and lets the user cancel", () => {
    const props = renderPanel({ isConverting: true, progress: running });

    expect(screen.getByText("Encoding image…")).toBeInTheDocument();
    expect(screen.queryByText(/%/)).toBeNull();
    expect(screen.queryByText(/remaining/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(props.onCancelOperation).toHaveBeenCalledTimes(1);
  });

  it("reports a cancelled conversion without an error", () => {
    renderPanel({
      progress: {
        ...running,
        status: "cancelled",
        stage: "cancelled",
        message: "Image conversion cancelled",
      },
    });

    expect(screen.getByText("Image conversion cancelled")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    expect(screen.getByRole("button", { name: "Convert Image" })).toBeEnabled();
  });

  it("shows the converted image and opens it", () => {
    const result: ConversionResult = {
      pages: 1,
      bytesWritten: 640_000,
      name: "photo.jpg",
      directory: "/Users/me/Pictures",
      image: {
        format: "jpeg",
        formatLabel: "JPEG",
        width: 4000,
        height: 3000,
        hadAlpha: true,
        flattened: true,
      },
    };
    const props = renderPanel({ result });

    expect(screen.getByText("Image converted")).toBeInTheDocument();
    expect(screen.getByText(/4000×3000 · JPEG/)).toBeInTheDocument();
    expect(
      screen.getByText("Transparent areas were filled with white."),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open image" }));
    expect(props.onOpen).toHaveBeenCalledTimes(1);
  });
});
