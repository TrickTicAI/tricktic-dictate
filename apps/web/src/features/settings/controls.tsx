import type { ReactNode } from 'react';

/**
 * A titled card of setting rows, plus an optional note attached beneath it.
 *
 * The note is part of the group on purpose. Rendering an explanation as a
 * free-standing `notice` between two cards produced a box that belonged to
 * neither, sitting flush against the card above it with no gap and no owner.
 */
export function Group({ title, lead, children, note, noteTone }: {
  title?: string;
  /** Shown between the heading and the card, such as a preview of what the rows change. */
  lead?: ReactNode;
  children: ReactNode;
  note?: ReactNode;
  noteTone?: 'alert';
}) {
  return (
    <section className="settings-group">
      {title && <h3>{title}</h3>}
      {lead}
      <div className="settings-card">{children}</div>
      {/* Only a problem announces itself. A standing explanation is prose, and
          giving it a live region would make every panel shout on mount. */}
      {note && (
        <p className={`group-note ${noteTone ?? ''}`} {...(noteTone === 'alert' ? { role: 'alert' } : {})}>
          {note}
        </p>
      )}
    </section>
  );
}

export interface RowProps {
  title: string;
  detail?: ReactNode;
  children?: ReactNode;
  /**
   * Marks a control whose backend does not exist yet. It renders greyed, with
   * an `In development` label, and stores nothing.
   *
   * This is a deliberate exception to the rule against showing scaffold
   * behaviour as working: the rule exists so the interface never lies about
   * what the product does, and a control that says `In development` on its face
   * is telling the truth. Do not make these functional-looking, and do not
   * delete them because they are unimplemented — each loses its label when its
   * backend lands.
   */
  unbuilt?: boolean;
  stacked?: boolean;
}

export function Row({ title, detail, children, unbuilt, stacked }: RowProps) {
  return (
    <div className={`setting-row ${unbuilt ? 'unbuilt' : ''} ${stacked ? 'stacked' : ''}`}>
      <div className="setting-text">
        <strong>{title}</strong>
        {detail && <p>{detail}</p>}
      </div>
      <div className="setting-control">
        {unbuilt && <span className="badge">In development</span>}
        {children}
      </div>
    </div>
  );
}

export function Toggle({ checked, onChange, label, disabled }: {
  checked: boolean;
  onChange?: (value: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className="switch"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled || !onChange}
      onClick={() => onChange?.(!checked)}
    />
  );
}

export function Segmented<T extends string>({ value, options, onChange, label, disabled }: {
  value: T;
  options: { id: T; label: string }[];
  onChange?: (value: T) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <div className={`segmented ${disabled ? 'disabled' : ''}`} role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          aria-pressed={value === option.id}
          disabled={disabled || !onChange}
          onClick={() => onChange?.(option.id)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

/** Shown where a section exists only in the desktop build. */
export function BrowserModeNote({ what }: { what: string }) {
  return (
    <p className="notice">
      {what} are part of the desktop app. This is the self-hosted browser client, so they are shown here for
      reference but have nothing to control.
    </p>
  );
}
