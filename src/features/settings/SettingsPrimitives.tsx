import type {
  ButtonHTMLAttributes,
  HTMLAttributes,
  InputHTMLAttributes,
  SelectHTMLAttributes,
  ReactNode,
  TextareaHTMLAttributes,
} from "react";
import { GithubFilled } from "@ant-design/icons";
import { CircleHelp } from "lucide-react";
import { forwardRef, useId, useState } from "react";

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
    hasIndicator?: boolean;
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
            item.hasIndicator ? `${item.label}，有可用更新` : item.label
          }
          aria-current={activeSection === item.id ? "page" : undefined}
          onClick={() => onSelect(item.id)}
        >
          <span className="settings-nav-icon">
            {item.icon}
            {item.hasIndicator ? (
              <span
                className="application-update-indicator"
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
      <AppScrollArea
        className="settings-page-scroll"
        viewportClassName="settings-page-viewport"
      >
        <SettingsPageTitle title={title} titleId={titleId} />
        {children}
      </AppScrollArea>
    </section>
  );
}

export function SettingsPageTitle({
  title,
  titleId,
}: {
  title: string;
  titleId?: string;
}) {
  return (
    <div className="settings-page-title-band" data-tauri-drag-region>
      <h2 className="settings-page-title" id={titleId} data-tauri-drag-region>
        {title}
      </h2>
    </div>
  );
}

export function SettingsSection({
  title,
  status,
  titleAccessory,
  children,
}: {
  title: string;
  status?: ReactNode;
  titleAccessory?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="settings-section">
      <div className="settings-section-heading">
        <div className="settings-section-title-group">
          <h3 className="settings-section-title">{title}</h3>
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
        <span
          id={tooltipId}
          className="settings-help-tooltip-content"
          role="tooltip"
        >
          {children}
        </span>
      ) : null}
    </span>
  );
}

export function SettingsFieldRow({
  label,
  htmlFor,
  children,
  className,
}: {
  label: string;
  htmlFor?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={classes("settings-field-row", className)}>
      <label className="settings-field-label" htmlFor={htmlFor}>
        {label}
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
