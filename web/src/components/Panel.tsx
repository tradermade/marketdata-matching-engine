import type { ReactNode } from 'react';

/** A card: header strip (title + optional controls) above a padded body. */
export function Panel({
  title,
  icon,
  right,
  className = '',
  children,
}: {
  title?: ReactNode;
  icon?: ReactNode;
  right?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={`card flex flex-col min-h-0 overflow-hidden ${className}`}>
      {(title || right) && (
        <div className="flex items-center justify-between gap-2 px-4 py-2.5 border-b border-white/5 bg-white/[0.015]">
          <h3 className="text-[15px] font-semibold text-white flex items-center gap-2">
            {icon ?? <span className="w-1 h-3.5 rounded-full bg-amber-400" aria-hidden />}
            {title}
          </h3>
          {right}
        </div>
      )}
      <div className="flex-1 min-h-0 flex flex-col p-3">{children}</div>
    </div>
  );
}

export function Toggle<T extends string>({
  value,
  options,
  onChange,
  accent = 'text-white',
}: {
  value: T;
  options: readonly T[] | { value: T; label: string; title?: string }[];
  onChange: (v: T) => void;
  accent?: string;
}) {
  const opts = (options as any[]).map((o) => (typeof o === 'string' ? { value: o, label: o } : o)) as {
    value: T;
    label: string;
    title?: string;
  }[];
  return (
    <div className="flex gap-0.5 p-0.5 rounded-full bg-bg/70 ring-1 ring-white/5">
      {opts.map((o) => (
        <button
          key={o.value}
          onClick={() => onChange(o.value)}
          title={o.title}
          className={`px-2.5 py-0.5 rounded-full text-[12px] font-medium cursor-pointer transition-colors ${
            value === o.value ? `bg-panel2 shadow-sm ${accent}` : 'text-mute hover:text-white'
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="text-mute/70 text-[13px] p-4 text-center">{children}</div>;
}
