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

export function formatPageRanges(pages: number[]): string {
  if (pages.length === 0) return "None";

  const ranges: Array<[number, number]> = [];
  for (const page of pages) {
    const last = ranges[ranges.length - 1];
    if (last && page === last[1] + 1) {
      last[1] = page;
    } else {
      ranges.push([page, page]);
    }
  }

  return ranges
    .map(([start, end]) => (start === end ? `${start}` : `${start}–${end}`))
    .join(", ");
}
