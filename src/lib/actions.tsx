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
};
