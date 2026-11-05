import { type ReactNode, useCallback, useEffect, useRef } from "react";

export function SidebarItem({
  icon,
  label,
  active,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-sm font-medium transition ${
        active
          ? "bg-sidebar-active text-heading"
          : "text-muted hover:bg-sidebar-active hover:text-heading"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}

export function SidebarSubItem({
  icon,
  label,
  active,
  onClick,
}: {
  icon?: ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex w-full items-center gap-2 rounded-lg py-1.5 pl-10 pr-3 text-left text-sm font-medium transition ${
        active
          ? "bg-sidebar-active text-heading"
          : "text-muted hover:bg-sidebar-active hover:text-heading"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}

export function FieldGroup({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-sm font-medium text-heading">{label}</span>
      {hint && <span className="mb-2 block text-xs text-muted">{hint}</span>}
      {children}
    </label>
  );
}

export function EmptyState({ message }: { message: string }) {
  return (
    <div className="flex h-full items-center justify-center p-8">
      <p className="text-center text-sm text-muted">{message}</p>
    </div>
  );
}

export function SmallButton({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button
      onClick={onClick}
      className="flex items-center gap-1.5 rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
    >
      {children}
    </button>
  );
}

export function SmallDangerButton({
  onClick,
  children,
}: {
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className="flex items-center gap-1.5 rounded-lg border border-danger/30 px-3 py-1.5 text-xs font-medium text-danger transition hover:bg-danger-bg hover:border-danger/50"
    >
      {children}
    </button>
  );
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    },
    [onCancel],
  );

  useEffect(() => {
    if (!open) return;
    document.addEventListener("keydown", handleKeyDown);
    cancelRef.current?.focus();
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [open, handleKeyDown]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-dim-overlay"
      onClick={onCancel}
    >
      <div
        className="mx-4 w-full max-w-sm rounded-xl border border-rule-strong bg-page p-6 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-base font-semibold text-heading">{title}</h3>
        <p className="mt-2 text-sm leading-relaxed text-body">{description}</p>
        <div className="mt-6 flex justify-end gap-3">
          <button
            ref={cancelRef}
            onClick={onCancel}
            className="rounded-lg border border-rule-strong px-4 py-2 text-sm font-medium text-body transition hover:bg-sidebar-active"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            className="rounded-lg border border-danger/40 bg-danger-bg px-4 py-2 text-sm font-medium text-danger transition hover:bg-danger/15"
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
