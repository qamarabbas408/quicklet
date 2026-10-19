import type { ComponentType } from "react";
import {
  CompressIcon,
  FileTextIcon,
  FolderIcon,
  ImagesIcon,
  SlidersIcon,
  WrenchIcon,
} from "../components/icons";

export type RadialAction = {
  id: string;
  label: string;
  icon: ComponentType<{ className?: string }>;
};

export const actions: RadialAction[] = [
  { id: "images", label: "Images", icon: ImagesIcon },
  { id: "pdf", label: "PDF", icon: FileTextIcon },
  { id: "compress", label: "Compress", icon: CompressIcon },
  { id: "documents", label: "Documents", icon: FolderIcon },
  { id: "tools", label: "Tools", icon: WrenchIcon },
  { id: "settings", label: "Settings", icon: SlidersIcon },
];
