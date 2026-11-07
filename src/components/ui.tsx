import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { createPortal } from "react-dom";

export function Card({
  title,
  action,
  children,
}: {
  title?: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="card">
      {(title || action) && (
        <div className="card-head">
          {title ? <h2>{title}</h2> : <span />}
          {action}
        </div>
      )}
      {children}
    </section>
  );
}

export function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="row">
      <div>
        <div className="row-label">{label}</div>
        {hint && <div className="row-hint">{hint}</div>}
      </div>
      <div className="row-control">{children}</div>
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  disabled,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      className={`toggle ${checked ? "on" : ""}`}
      onClick={() => onChange(!checked)}
    />
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  return (
    <div className="seg" role="tablist">
      {options.map((option) => (
        <button
          type="button"
          key={option.value}
          role="tab"
          aria-selected={option.value === value}
          className={option.value === value ? "active" : ""}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function Tabs<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  // 标签栏吸顶时切换标签：把新内容顶到标签栏下面，否则会停在
  // 上一个标签滚到的位置，直接从半截开始。吸顶后量不到标签栏的
  // 原位，所以先临时滚回顶部量一次再定位（同一帧内，不会闪）。
  useEffect(() => {
    const el = ref.current;
    const main = el?.closest(".main") as HTMLElement | null;
    if (!el || !main) return;
    const stickyTop = parseFloat(getComputedStyle(el).top) || 0;
    const mainTop = main.getBoundingClientRect().top;
    if (el.getBoundingClientRect().top - mainTop > stickyTop + 0.5) return;
    main.scrollTop = 0;
    const natural = el.getBoundingClientRect().top - mainTop;
    main.scrollTop = Math.max(0, natural - stickyTop);
  }, [value]);

  return (
    <div className="tabs" role="tablist" ref={ref}>
      {options.map((option) => (
        <button
          type="button"
          key={option.value}
          role="tab"
          aria-selected={option.value === value}
          className={`tab ${option.value === value ? "active" : ""}`}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function Slider({
  value,
  min,
  max,
  step,
  onChange,
  format,
}: {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
  format: (value: number) => string;
}) {
  return (
    <>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <span className="value">{format(value)}</span>
    </>
  );
}

/// 精确数值：滑杆调不准的毫秒、像素用这个直接输。
export function NumberField({
  value,
  min,
  max,
  step = 1,
  suffix,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  suffix?: string;
  onChange: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));

  // 外部改了值（换了显示器、恢复默认）才同步，正在输入时不动。
  useEffect(() => {
    setText((current) => (Number(current) === value ? current : String(value)));
  }, [value]);

  const commit = (raw: string) => {
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) {
      setText(String(value));
      return;
    }
    const clamped = Math.min(max, Math.max(min, Math.round(parsed)));
    setText(String(clamped));
    if (clamped !== value) onChange(clamped);
  };

  return (
    <span className="number-field">
      <input
        type="number"
        inputMode="numeric"
        min={min}
        max={max}
        step={step}
        value={text}
        onChange={(event) => {
          const raw = event.target.value;
          setText(raw);
          const parsed = Number(raw);
          // 输入过程中只在范围内就写回，超出范围留到失焦再夹紧。
          if (raw !== "" && Number.isFinite(parsed) && parsed >= min && parsed <= max) {
            if (parsed !== value) onChange(parsed);
          }
        }}
        onBlur={(event) => commit(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
        }}
      />
      {suffix && <span className="number-suffix">{suffix}</span>}
    </span>
  );
}

/// 空列表统一长这样：图标、一句说明，可选一个动作。
export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon?: ReactNode;
  title: string;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty-state">
      {icon && <div className="empty-state-icon">{icon}</div>}
      <div className="empty-state-title">{title}</div>
      {hint && <div className="empty-state-hint">{hint}</div>}
      {action && <div className="empty-state-action">{action}</div>}
    </div>
  );
}

/// 危险操作的确认。系统 confirm 不能给删除按钮上红，也挡不住误点。
export function ConfirmDialog({
  open,
  title,
  children,
  confirmLabel = "删除",
  cancelLabel = "取消",
  danger = true,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  children: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onCancel]);

  if (!open) return null;
  // 挂到 body 上：页面本身有 transform 动画，会让 position: fixed 相对页面
  // 定位；页面一长，弹窗就落到首屏下面去了。
  return createPortal(
    <div
      className="modal-backdrop"
      role="presentation"
      onClick={onCancel}
    >
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onClick={(event) => event.stopPropagation()}
      >
        <h2>{title}</h2>
        <div className="confirm-body">{children}</div>
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onCancel}>
            {cancelLabel}
          </button>
          <button
            type="button"
            className={`btn ${danger ? "danger" : "primary"}`}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
