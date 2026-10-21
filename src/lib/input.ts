export type InputFile = {
  id: number;
  name: string;
  extension: string | null;
  kind: string;
  size: number | null;
};

export type InputSelection = {
  actionId: string;
  files: InputFile[];
  limit: number;
  skipped: number;
};
