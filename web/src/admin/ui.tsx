type SwitchProps = { on: boolean; onToggle: () => void; label: string; describedBy?: string; disabled?: boolean };

export function Switch({ on, onToggle, label, describedBy, disabled }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      className="sw"
      aria-checked={on}
      aria-label={label}
      aria-describedby={describedBy}
      onClick={onToggle}
      disabled={disabled}
    >
      <span aria-hidden="true" />
    </button>
  );
}
