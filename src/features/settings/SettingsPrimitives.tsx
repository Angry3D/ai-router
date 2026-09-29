import type {
  ButtonHTMLAttributes,
  HTMLAttributes,
  InputHTMLAttributes,
  SelectHTMLAttributes,
  ReactNode,
  Ref,
  TextareaHTMLAttributes,
} from "react";
import { GithubFilled } from "@ant-design/icons";
import { CircleHelp } from "lucide-react";
import { forwardRef, useId, useState } from "react";
import { createPortal } from "react-dom";

import { appVariant, appVersionLabel } from "../../appVariant";
import { AppScrollArea } from "../shared/AppScrollArea";

export type SettingsSectionId = "routes" | "usage" | "codex" | "system";
export type SettingsTone = "neutral" | "success" | "warning" | "danger";

export interface SettingsConfirmation {
  title: string;
  body: ReactNode;
  details?: ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  destructive?: boolean;
  onConfirm: () => void;
}

function classes(...values: Array<string | false | null | undefined>) {
  return values.filter(Boolean).join(" ");
}

export function SettingsSidebar({
  activeSection,
  items,
  onSelect,
  onOpenRepository,
  isRepositoryPending,
  version,
}: {
  activeSection: SettingsSectionId;
  items: ReadonlyArray<{
    id: SettingsSectionId;
    label: string;
    icon: ReactNode;
    indicatorLabel?: string;
  }>;
  onSelect: (section: SettingsSectionId) => void;
  onOpenRepository: () => void;
  isRepositoryPending: boolean;
  version: string | null;
}) {
  return (
    <nav className="settings-nav" aria-label="设置分区">
      <div
        className="settings-drag-region"
        data-tauri-drag-region
        aria-hidden="true"
      />
      <div className="app-identity">
        <h1>AI Router</h1>
        {appVariant.badge ? (
          <span className="app-variant-badge">{appVariant.badge}</span>
        ) : null}
      </div>
      {items.map((item) => (
        <button
          className={classes(
            "settings-nav-item",
            activeSection === item.id && "is-active",
          )}
          type="button"
          key={item.id}
          aria-label={
            item.indicatorLabel
              ? `${item.label}，${item.indicatorLabel}`
              : item.label
          }
          aria-current={activeSection === item.id ? "page" : undefined}
          onClick={() => onSelect(item.id)}
        >
          <span className="settings-nav-icon">
            {item.icon}
            {item.indicatorLabel ? (
              <span
                className="settings-navigation-indicator"
                aria-hidden="true"
              />
            ) : null}
          </span>
          {item.label}
        </button>
      ))}
      <div className="settings-nav-footer">
        <p className="settings-nav-version">
          {appVersionLabel(version, appVariant)}
        </p>
        <button
          className="settings-github-link"
          type="button"
          aria-label="打开 GitHub 项目"
          title="打开 GitHub 项目"
          disabled={isRepositoryPending}
          onClick={onOpenRepository}
        >
          <GithubFilled aria-hidden="true" />
        </button>
      </div>
    </nav>
  );
}

export function SettingsPage({
  title,
  titleId,
  children,
  className,
}: {
  title: string;
  titleId: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section
      className={classes("settings-page", className)}
      aria-labelledby={titleId}
    >
      <SettingsPageTitle title={title} titleId={titleId} />
      <AppScrollArea
        className="settings-page-scroll"
        viewportClassName="settings-page-viewport"
      >
        {children}
      </AppScrollArea>
    </section>
  );
}

export function SettingsPageTitle({
  title,
  titleId,
  detail,
  detailTone = "warning",
}: {
  title: string;
  titleId?: string;
  detail?: string | null;
  detailTone?: "warning" | "accent";
}) {
  return (
    <div
      className={`settings-page-title-band${detail ? " has-detail" : ""}`}
      data-tauri-drag-region
    >
      <h2 className="settings-page-title" id={titleId} data-tauri-drag-region>
        {title}
      </h2>
      {detail ? (
        <span
          className={`settings-page-title-detail settings-page-title-detail-${detailTone}`}
          title={detail}
          data-tauri-drag-region
        >
          {detail}
        </span>
      ) : null}
    </div>
  );
}

export function SettingsSection({
  title,
  titleId,
  titleRef,
  titleTabIndex,
  status,
  titleAccessory,
  children,
}: {
  title: string;
  titleId?: string;
  titleRef?: React.Ref<HTMLHeadingElement>;
  titleTabIndex?: number;
  status?: ReactNode;
  titleAccessory?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="settings-section">
      <div className="settings-section-heading">
        <div className="settings-section-title-group">
          <h3
            className="settings-section-title"
            id={titleId}
            ref={titleRef}
            tabIndex={titleTabIndex}
          >
            {title}
          </h3>
          {titleAccessory}
        </div>
        {status}
      </div>
      {children}
    </section>
  );
}

export function SettingsHelpTooltip({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  const tooltipId = useId();
  const [visible, setVisible] = useState(false);
  return (
    <span
      className="settings-help-tooltip"
      onMouseEnter={() => setVisible(true)}
      onMouseLeave={() => setVisible(false)}
    >
      <button
        type="button"
        className="settings-help-tooltip-trigger"
        aria-label={label}
        aria-describedby={visible ? tooltipId : undefined}
        onFocus={() => setVisible(true)}
        onBlur={() => setVisible(false)}
      >
        <CircleHelp aria-hidden="true" size={15} />
      </button>
      {visible ? (
        <div
          id={tooltipId}
          className="settings-help-tooltip-content"
          role="tooltip"
        >
          {children}
        </div>
      ) : null}
    </span>
  );
}

export function SettingsFieldRow({
  label,
  htmlFor,
  required = false,
  children,
  className,
}: {
  label: string;
  htmlFor?: string;
  required?: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={classes("settings-field-row", className)}>
      <label className="settings-field-label" htmlFor={htmlFor}>
        {label}
        {required ? (
          <span className="settings-required-marker" aria-hidden="true" />
        ) : null}
      </label>
      <div className="settings-field-control">{children}</div>
    </div>
  );
}

export function SettingsReadonlyRow({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="settings-field-row settings-readonly-row">
      <span className="settings-field-label">{label}</span>
      <strong className="settings-readonly-value">{children}</strong>
    </div>
  );
}

export function SettingsDivider() {
  return <hr className="settings-divider" />;
}

export function SettingsButton({
  variant = "secondary",
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "danger" | "danger-link";
}) {
  return (
    <button
      {...props}
      className={classes(
        "settings-button",
        `settings-button-${variant}`,
        className,
      )}
    />
  );
}

export function SettingsIconButton({
  label,
  title = label,
  className,
  ...props
}: Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label" | "title"> & {
  label: string;
  title?: string;
}) {
  return (
    <button
      {...props}
      className={classes("settings-icon-button", className)}
      aria-label={label}
      title={title}
    />
  );
}

export const SettingsTextInput = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement>
>(function SettingsTextInput({ className, ...props }, ref) {
  return (
    <input
      {...props}
      ref={ref}
      className={classes("settings-text-input", className)}
    />
  );
});

export function SettingsSelect({
  className,
  ...props
}: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select {...props} className={classes("settings-select", className)} />
  );
}

export function SettingsTextarea({
  className,
  ...props
}: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea {...props} className={classes("settings-textarea", className)} />
  );
}

export interface SettingsComboboxOption {
  id: string;
  disabled?: boolean;
}

export function SettingsCombobox({
  id,
  inputRef,
  value,
  onChange,
  options,
  onSelect,
  onSubmit,
  ariaLabel,
  placeholder,
  describedBy,
  maxLength,
  popupHost,
  disabled = false,
  invalid = false,
  placement = "below",
}: {
  id?: string;
  inputRef?: Ref<HTMLInputElement>;
  value: string;
  onChange: (value: string) => void;
  options: SettingsComboboxOption[];
  onSelect: (id: string) => void;
  onSubmit?: (value: string) => void;
  ariaLabel: string;
  placeholder?: string;
  describedBy?: string;
  maxLength?: number;
  popupHost?: HTMLElement | null;
  disabled?: boolean;
  invalid?: boolean;
  placement?: "below" | "above";
}) {
  const generatedId = useId();
  const inputId = id ?? `settings-combobox-${generatedId}`;
  const listboxId = `${inputId}-listbox`;
  const optionId = (index: number) => `${listboxId}-option-${index}`;
  const [open, setOpen] = useState(false);
  const [highlight, setHighlight] = useState(0);

  const query = value.trim().toLowerCase();
  const filtered = query
    ? options.filter((option) => option.id.toLowerCase().includes(query))
    : options;
  const expanded = open && !disabled && options.length > 0;

  let activeIndex = -1;
  if (expanded) {
    if (filtered[highlight] && !filtered[highlight].disabled) {
      activeIndex = highlight;
    } else {
      for (let index = highlight; index < filtered.length; index += 1) {
        if (!filtered[index].disabled) {
          activeIndex = index;
          break;
        }
      }
      if (activeIndex === -1) {
        const last = Math.min(highlight, filtered.length - 1);
        for (let index = last; index >= 0; index -= 1) {
          if (!filtered[index].disabled) {
            activeIndex = index;
            break;
          }
        }
      }
    }
  }

  const openPopup = () => {
    if (disabled || options.length === 0) return;
    setHighlight(0);
    setOpen(true);
  };

  const moveHighlight = (step: 1 | -1) => {
    if (!expanded) {
      setHighlight(step === 1 ? 0 : Math.max(filtered.length - 1, 0));
      setOpen(options.length > 0);
      return;
    }
    if (activeIndex === -1) {
      setHighlight(step === 1 ? 0 : Math.max(filtered.length - 1, 0));
      return;
    }
    for (
      let index = activeIndex + step;
      index >= 0 && index < filtered.length;
      index += step
    ) {
      if (!filtered[index].disabled) {
        setHighlight(index);
        return;
      }
    }
    setHighlight(activeIndex);
  };

  const selectOption = (option: SettingsComboboxOption) => {
    if (option.disabled) return;
    onSelect(option.id);
  };

  const popup = expanded ? (
    <div
      id={listboxId}
      className="settings-combobox-popup"
      data-placement={placement}
      role="listbox"
    >
      {filtered.length === 0 ? (
        <div className="settings-combobox-hint">无匹配模型</div>
      ) : (
        filtered.map((option, index) => (
          <div
            key={option.id}
            id={optionId(index)}
            className="settings-combobox-option"
            role="option"
            aria-selected={index === activeIndex}
            aria-disabled={option.disabled ? true : undefined}
            onMouseDown={(event) => event.preventDefault()}
            onMouseEnter={() => {
              if (!option.disabled) setHighlight(index);
            }}
            onClick={() => selectOption(option)}
          >
            <span className="settings-combobox-option-label">{option.id}</span>
            {option.disabled ? (
              <span className="settings-combobox-badge">已添加</span>
            ) : null}
          </div>
        ))
      )}
    </div>
  ) : null;
  const popupNode =
    popup && popupHost ? createPortal(popup, popupHost) : popup;

  return (
    <div className="settings-combobox">
      <input
        id={inputId}
        ref={inputRef}
        className="settings-combobox-input"
        type="text"
        role="combobox"
        aria-label={ariaLabel}
        aria-expanded={expanded}
        aria-controls={listboxId}
        aria-autocomplete="list"
        aria-activedescendant={
          activeIndex === -1 ? undefined : optionId(activeIndex)
        }
        aria-invalid={invalid ? true : undefined}
        aria-describedby={describedBy}
        placeholder={placeholder}
        maxLength={maxLength}
        disabled={disabled}
        value={value}
        onChange={(event) => {
          onChange(event.currentTarget.value);
          if (!open) openPopup();
        }}
        onFocus={openPopup}
        onBlur={() => setOpen(false)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            moveHighlight(1);
            return;
          }
          if (event.key === "ArrowUp") {
            event.preventDefault();
            moveHighlight(-1);
            return;
          }
          if (event.key === "Escape") {
            if (open) {
              event.preventDefault();
              setOpen(false);
            }
            return;
          }
          if (event.key === "Tab") {
            setOpen(false);
            return;
          }
          if (event.key !== "Enter") return;
          if (activeIndex !== -1) {
            event.preventDefault();
            selectOption(filtered[activeIndex]);
            return;
          }
          const submitted = value.trim();
          if (!submitted || !onSubmit) return;
          event.preventDefault();
          onSubmit(submitted);
        }}
      />
      {popupNode}
    </div>
  );
}

export function SettingsSwitch({
  label,
  checked,
  onChange,
  disabled,
}: {
  label: string;
  checked: boolean;
  onChange: InputHTMLAttributes<HTMLInputElement>["onChange"];
  disabled?: boolean;
}) {
  return (
    <label className="settings-switch-row">
      <input
        className="settings-switch-control"
        type="checkbox"
        role="switch"
        aria-checked={checked}
        checked={checked}
        disabled={disabled}
        onChange={onChange}
      />
      <span>{label}</span>
    </label>
  );
}

export function SettingsStatus({
  tone = "neutral",
  className,
  ...props
}: HTMLAttributes<HTMLSpanElement> & {
  tone?: SettingsTone;
}) {
  return (
    <span
      {...props}
      className={classes(
        "settings-status",
        `settings-status-${tone}`,
        className,
      )}
    />
  );
}

export function SettingsActionGroup({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={classes("settings-action-group", className)}>
      {children}
    </div>
  );
}

export function SettingsFooter({
  leading,
  children,
}: {
  leading?: ReactNode;
  children: ReactNode;
}) {
  return (
    <footer className="settings-footer">
      <div className="settings-footer-leading">{leading}</div>
      <SettingsActionGroup>{children}</SettingsActionGroup>
    </footer>
  );
}

export function SettingsConfirmDialog({
  confirmation,
  onCancel,
}: {
  confirmation: SettingsConfirmation;
  onCancel: () => void;
}) {
  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        className="confirm-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="dialog-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onCancel();
          }
        }}
      >
        <h2 id="dialog-title">{confirmation.title}</h2>
        <p>{confirmation.body}</p>
        {confirmation.details ? (
          <div className="settings-confirm-details">{confirmation.details}</div>
        ) : null}
        <div className="dialog-actions">
          <SettingsButton type="button" onClick={onCancel} autoFocus>
            {confirmation.cancelLabel ?? "取消"}
          </SettingsButton>
          <SettingsButton
            type="button"
            variant={confirmation.destructive ? "danger" : "primary"}
            onClick={confirmation.onConfirm}
          >
            {confirmation.confirmLabel}
          </SettingsButton>
        </div>
      </section>
    </div>
  );
}
