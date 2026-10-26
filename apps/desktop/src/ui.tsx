// Petits composants d'interface partagés.

import clsx from "clsx";
import type { ComponentPropsWithRef, ReactNode } from "react";

export function formatBytes(b: number | null | undefined): string {
  if (b == null) return "—";
  // Unités binaires, comme l'Explorateur Windows.
  if (b >= 1024 ** 3) return `${(b / 1024 ** 3).toFixed(1).replace(".", ",")} Go`;
  if (b >= 1024 ** 2) return `${Math.round(b / 1024 ** 2)} Mo`;
  return `${Math.round(b / 1024)} Ko`;
}

type Variant = "primary" | "secondary" | "ghost" | "danger";

export function Button({
  variant = "secondary",
  className,
  ...props
}: ComponentPropsWithRef<"button"> & { variant?: Variant }) {
  return (
    <button
      {...props}
      className={clsx(
        "inline-flex items-center justify-center gap-2 rounded-lg px-3.5 py-2 text-sm font-medium transition-colors",
        "disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]",
        variant === "primary" && "bg-[var(--accent)] text-[var(--accent-text)] hover:brightness-110",
        variant === "secondary" && "border border-[var(--border)] bg-[var(--panel)] hover:bg-[var(--panel-2)]",
        variant === "ghost" && "hover:bg-[var(--panel-2)]",
        variant === "danger" && "text-[var(--danger)] hover:bg-[var(--panel-2)]",
        className,
      )}
    />
  );
}

export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div className={clsx("rounded-xl border border-[var(--border)] bg-[var(--panel)] p-5", className)}>{children}</div>
  );
}

export function Badge({ children, tone = "neutral" }: { children: ReactNode; tone?: "neutral" | "accent" | "ok" }) {
  return (
    <span
      className={clsx(
        "inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium",
        tone === "neutral" && "bg-[var(--panel-2)] text-[var(--muted)]",
        tone === "accent" && "bg-[var(--accent-soft)] text-[var(--accent)]",
        tone === "ok" && "bg-[var(--panel-2)] text-[var(--ok)]",
      )}
    >
      {children}
    </span>
  );
}

export function ProgressBar({ value, total }: { value: number; total: number | null }) {
  const pct = total ? Math.min(100, (value / total) * 100) : 0;
  return (
    <div className="h-2 w-full overflow-hidden rounded-full bg-[var(--panel-2)]">
      <div className="h-full rounded-full bg-[var(--accent)] transition-[width] duration-200" style={{ width: `${pct}%` }} />
    </div>
  );
}

export function Spinner({ className }: { className?: string }) {
  return (
    <span
      className={clsx(
        "inline-block size-4 animate-spin rounded-full border-2 border-current border-t-transparent",
        className,
      )}
      aria-hidden
    />
  );
}

export function ErrorNote({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <p className="rounded-lg border border-[var(--danger)]/40 bg-[var(--danger)]/10 px-3 py-2 text-sm whitespace-pre-wrap text-[var(--danger)]">
      {message}
    </p>
  );
}

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: ReactNode }) {
  return (
    <div className="mb-6 flex items-start justify-between gap-4">
      <div>
        <h1 className="text-xl font-semibold">{title}</h1>
        {subtitle && <p className="mt-1 text-[var(--muted)]">{subtitle}</p>}
      </div>
      {actions}
    </div>
  );
}
