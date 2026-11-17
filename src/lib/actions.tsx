import type { ComponentType } from "react";
import {
  CompressIcon,
  FileTextIcon,
  FolderIcon,
  ImagesIcon,
  SlidersIcon,
  WrenchIcon,
} from "../components/icons";

export type ActionInput = "images" | "pdf" | "documents" | "any" | "none";

export type RadialAction = {
  id: string;
  label: string;
  icon: ComponentType<{ className?: string }>;
  input: ActionInput;
};

export const actions: RadialAction[] = [
  { id: "images", label: "Images", icon: ImagesIcon, input: "images" },
  { id: "pdf", label: "PDF", icon: FileTextIcon, input: "pdf" },
  { id: "compress", label: "Compress", icon: CompressIcon, input: "any" },
  { id: "documents", label: "Documents", icon: FolderIcon, input: "documents" },
  { id: "tools", label: "Tools", icon: WrenchIcon, input: "any" },
  { id: "settings", label: "Settings", icon: SlidersIcon, input: "none" },
];

export type ActionInfo = {
  id: string;
  label: string;
  description: string;
  enabled: boolean;
  input: string | null;
};

export type OutputInfo = {
  name: string;
  directory: string;
};

export type OutputDirectoryInfo = {
  directory: string;
};

export type SplitMode = "every" | "ranges";

export type SplitSourceInfo = {
  name: string;
  pages: number;
  bytes: number;
  maxPages: number;
  maxOutputs: number;
  maxInputBytes: number;
};

export type SplitOutputPlan = {
  name: string;
  pages: number[];
};

export type SplitPlan = {
  pageCount: number;
  outputs: SplitOutputPlan[];
  excludedPages: number[];
};

export type ConversionFile = {
  name: string;
  pages: number;
  bytesWritten: number;
};

export type CompressionPreset = "light" | "balanced" | "strong";

export type CompressionSourceInfo = {
  name: string;
  pages: number;
  bytes: number;
  hasSignature: boolean;
  images: number;
  maxPages: number;
  maxImages: number;
  maxInputBytes: number;
};

export type PageSelectionMode = "extract" | "delete";

export type PageSourceInfo = {
  name: string;
  pages: number;
  bytes: number;
  maxPages: number;
  maxRanges: number;
  maxInputBytes: number;
};

export type PageSelectionPlan = {
  mode: PageSelectionMode;
  pageCount: number;
  selectedPages: number[];
  outputPages: number[];
  outputCount: number;
};

export type RotateSourceInfo = {
  name: string;
  pages: number;
  bytes: number;
  rotations: number[];
  hasSignature: boolean;
  maxPages: number;
  maxInputBytes: number;
};

export type RotateThumbnailRequest = {
  page: number;
  turns: number;
};

export type RotateThumbnail = {
  page: number;
  sourceRotation: number;
  effectiveRotation: number;
  width: number;
  height: number;
  pngBase64: string;
};

export type RotatePreviewBatch = {
  pageCount: number;
  thumbnails: RotateThumbnail[];
};

export type RotateChange = {
  page: number;
  turns: number;
};

export type CompressionReport = {
  originalBytes: number;
  outputBytes: number;
  savedBytes: number;
  reductionPercent: number;
  preset: string;
  status: "reduced" | "no-reduction";
  imagesRecompressed: number;
  imagesSkipped: number;
};

export type ConversionResult = {
  pages: number;
  bytesWritten: number;
  name: string;
  directory: string;
  files?: ConversionFile[];
  compression?: CompressionReport;
  image?: ConversionImage;
  imageCompression?: ImageCompressionReport;
};

export type OperationProgressStatus =
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled";

export type OperationProgressOutput = {
  name?: string;
  directory?: string;
  path?: string;
};

export type OperationProgress = {
  operationId: string;
  status: OperationProgressStatus;
  stage: string;
  message: string;
  determinate: boolean;
  completed?: number;
  total?: number;
  unit?: "pages" | "files" | "images" | "bytes" | string;
  etaSeconds?: number;
  error?: string;
  output?: OperationProgressOutput;
};

export type ImageFormat = "png" | "jpeg" | "webp";

export type ImageFormatOption = {
  id: ImageFormat;
  label: string;
  extension: string;
  lossy: boolean;
  supportsAlpha: boolean;
};

export type ImageSourceInfo = {
  name: string;
  format: ImageFormat;
  formatLabel: string;
  width: number;
  height: number;
  bytes: number;
  hasAlpha: boolean;
  /** EXIF orientation 1–8 as stored in the file. */
  orientation: number;
  /** Every supported format except the source's own. */
  destinations: ImageFormatOption[];
  defaultJpegQuality: number;
  maxInputBytes: number;
  maxDimension: number;
  maxPixels: number;
};

/** Fill for transparent areas when converting to JPEG. */
export type ImageBackground = "white" | "black";

export const IMAGE_BACKGROUNDS: Record<
  ImageBackground,
  { label: string; hex: string }
> = {
  white: { label: "White", hex: "#ffffff" },
  black: { label: "Black", hex: "#000000" },
};

export type ConversionImage = {
  format: ImageFormat;
  formatLabel: string;
  width: number;
  height: number;
  hadAlpha: boolean;
  flattened: boolean;
};

export type ImageMetadataSummary = {
  hasIcc: boolean;
  hasExif: boolean;
  hasXmp: boolean;
  hasIptc: boolean;
  preservedBytes: number;
};

export type ImageCompressionSourceInfo = {
  name: string;
  format: ImageFormat;
  formatLabel: string;
  width: number;
  height: number;
  bytes: number;
  hasAlpha: boolean;
  orientation: number;
  metadata: ImageMetadataSummary;
  defaultJpegQuality: number;
  maxInputBytes: number;
  maxDimension: number;
  maxPixels: number;
};

export type ImageCompressionReport = {
  format: ImageFormat;
  formatLabel: string;
  width: number;
  height: number;
  originalBytes: number;
  outputBytes: number;
  savedBytes: number;
  reductionPercent: number;
  status: "reduced" | "no-reduction";
  hadAlpha: boolean;
  jpegQuality: number | null;
  metadata: ImageMetadataSummary;
};
