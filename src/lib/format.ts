export function formatBytes(size: number): string {
  if (!Number.isFinite(size) || size < 0) return "Unknown size";
  if (size < 1024) return `${size} B`;

  const units = ["KB", "MB", "GB", "TB"];
  let value = size / 1024;
  let unit = 0;

  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }

  return `${value.toFixed(1).replace(/\.0$/, "")} ${units[unit]}`;
}

export function formatFileCount(count: number): string {
  return `${count} ${count === 1 ? "file" : "files"}`;
}
