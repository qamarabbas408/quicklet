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
