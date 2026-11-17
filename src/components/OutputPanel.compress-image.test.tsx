import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { OutputPanel } from "./OutputPanel";
import type {
  ConversionResult,
  ImageCompressionSourceInfo,
  OperationProgress,
} from "../lib/actions";

const source: ImageCompressionSourceInfo = {
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
    hasXmp: false,
    hasIptc: false,
    preservedBytes: 12_000,
  },
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
    actionLabel: "Compress Image",
    selection: {
      actionId: "images.compress",
      files: [
        {
          id: 1,
          name: "photo.jpg",
          extension: "jpg",
          kind: "JPEG image",
          size: 3_500_000,
        },
      ],
      limit: 1,
      duplicates: 0,
      limitSkipped: 0,
      rejected: 0,
    },
    output: {
      name: "Quicklet-Compress-photo-20261024-153045.jpg",
      directory: "/Users/me/Pictures",
    },
    outputDirectory: null,
    isChoosing: false,
    isConverting: false,
    isMerge: false,
    isSplit: false,
    isCompress: false,
    isImageConvert: false,
    isImageCompress: true,
    imageSource: null,
    imageFormat: null,
    imageQuality: 85,
    imageBackground: "white" as const,
    imageCompressionSource: source,
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
  operationId: "images.compress:1",
  status: "running",
  stage: "encoding",
  message: "Encoding image…",
  determinate: false,
};

describe("OutputPanel for image compression", () => {
  it("summarises settings and starts compression", () => {
    const props = renderPanel();

    expect(screen.getByText("JPEG (unchanged)")).toBeInTheDocument();
    expect(screen.getByText("JPEG quality 85")).toBeInTheDocument();
    expect(screen.getByText(/ICC\/EXIF preserved/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Compress Image" }));
    expect(props.onConvert).toHaveBeenCalledTimes(1);
  });

  it("shows stage-based progress without percentage or ETA and cancels", () => {
    const props = renderPanel({ isConverting: true, progress: running });

    expect(screen.getByText("Encoding image…")).toBeInTheDocument();
    expect(screen.queryByText(/%/)).toBeNull();
    expect(screen.queryByText(/remaining/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(props.onCancelOperation).toHaveBeenCalledTimes(1);
  });

  it("shows a retry state after an error", () => {
    renderPanel({
      error: "Could not encode the JPEG image.",
      isConverting: false,
    });

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Could not encode the JPEG image.",
    );
    expect(
      screen.getByRole("button", { name: "Retry compression" }),
    ).toBeInTheDocument();
  });

  it("reports a reduced result with original, output and savings", () => {
    const result: ConversionResult = {
      pages: 1,
      bytesWritten: 1_400_000,
      name: "Quicklet-Compress-photo-20261024-153045.jpg",
      directory: "/Users/me/Pictures",
      imageCompression: {
        format: "jpeg",
        formatLabel: "JPEG",
        width: 4000,
        height: 3000,
        originalBytes: 3_500_000,
        outputBytes: 1_400_000,
        savedBytes: 2_100_000,
        reductionPercent: 60,
        status: "reduced",
        hadAlpha: false,
        jpegQuality: 85,
        metadata: source.metadata,
      },
    };
    const props = renderPanel({ result });

    expect(screen.getByText("Image compressed")).toBeInTheDocument();
    expect(screen.getByText(/Original 3.3 MB/)).toBeInTheDocument();
    expect(screen.getByText(/Saved 2 MB/)).toBeInTheDocument();
    expect(screen.getByText(/60.0%/)).toBeInTheDocument();
    expect(
      screen.getByText(/ICC\/EXIF preserved where present/),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open image" }));
    expect(props.onOpen).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: /folder/i }));
    expect(props.onReveal).toHaveBeenCalledTimes(1);
    const back = screen.getByRole("button", { name: /Compression settings/ });
    expect(back).toBeInTheDocument();
    fireEvent.click(back);
    expect(props.onBack).toHaveBeenCalledTimes(1);
  });

  it("reports no reduction without claiming a saving", () => {
    const result: ConversionResult = {
      pages: 1,
      bytesWritten: 3_500_000,
      name: "Quicklet-Compress-photo-20261024-153045.jpg",
      directory: "/Users/me/Pictures",
      imageCompression: {
        format: "jpeg",
        formatLabel: "JPEG",
        width: 4000,
        height: 3000,
        originalBytes: 3_500_000,
        outputBytes: 3_500_000,
        savedBytes: 0,
        reductionPercent: 0,
        status: "no-reduction",
        hadAlpha: false,
        jpegQuality: 85,
        metadata: source.metadata,
      },
    };
    renderPanel({ result });

    expect(screen.getByText("No reduction")).toBeInTheDocument();
    expect(screen.getByText(/Saved 0 B \(0.0%\)/)).toBeInTheDocument();
    expect(
      screen.getByText(/the exact original bytes were copied/),
    ).toBeInTheDocument();
  });
});
