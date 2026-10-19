import type { RadialAction } from "../lib/actions";

type RadialMenuItemProps = {
  action: RadialAction;
  index: number;
  total: number;
  isSelected: boolean;
  onSelect: (action: RadialAction) => void;
};

const RING_RADIUS_PERCENT = 35;

export function RadialMenuItem({
  action,
  index,
  total,
  isSelected,
  onSelect,
}: RadialMenuItemProps) {
  const angle = (-90 + (360 / total) * index) * (Math.PI / 180);
  const left = 50 + Math.cos(angle) * RING_RADIUS_PERCENT;
  const top = 50 + Math.sin(angle) * RING_RADIUS_PERCENT;
  const Icon = action.icon;

  return (
    <button
      type="button"
      aria-pressed={isSelected}
      aria-label={`${action.label} (prototype action)`}
      className={`radial-item${isSelected ? " is-selected" : ""}`}
      style={{ left: `${left}%`, top: `${top}%` }}
      onClick={(event) => {
        event.stopPropagation();
        onSelect(action);
      }}
    >
      <Icon className="h-5 w-5" />
      <span className="radial-item-label">{action.label}</span>
    </button>
  );
}
