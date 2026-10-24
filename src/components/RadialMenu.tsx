import { actions, type RadialAction } from "../lib/actions";
import { QuickletMark } from "./icons";
import { RadialMenuItem } from "./RadialMenuItem";

type RadialMenuProps = {
  selectedAction: RadialAction | null;
  onSelect: (action: RadialAction) => void;
  onDismiss: () => void;
};

export function RadialMenu({
  selectedAction,
  onSelect,
  onDismiss,
}: RadialMenuProps) {
  return (
    <div
      className="radial-overlay"
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onDismiss();
        }
      }}
    >
      <div className="radial-menu" role="group" aria-label="Quicklet actions">
        <div
          className="radial-center"
          data-tauri-drag-region
          title="Drag to move Quicklet"
          aria-hidden="true"
        >
          <QuickletMark className="pointer-events-none h-14 w-14" />
        </div>

        {actions.map((action, index) => (
          <RadialMenuItem
            key={action.id}
            action={action}
            index={index}
            total={actions.length}
            isSelected={selectedAction?.id === action.id}
            onSelect={onSelect}
          />
        ))}
      </div>

      <div className="radial-footer">
        {selectedAction && (
          <div className="radial-status" role="status" aria-live="polite">
            <span className="h-1.5 w-1.5 rounded-full bg-emerald-300/90" />
            <span>
              {selectedAction.label} selected
              <span className="ml-1.5 text-white/35">
                &middot; prototype only
              </span>
            </span>
          </div>
        )}
        <p className="radial-hint">
          Press <kbd>Esc</kbd> or click outside to dismiss
        </p>
      </div>
    </div>
  );
}
