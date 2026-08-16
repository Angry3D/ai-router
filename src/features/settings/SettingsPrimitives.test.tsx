import { fireEvent, render, screen } from "@testing-library/react";
import { Route, Settings } from "lucide-react";
import { describe, expect, it, vi } from "vitest";

import {
  SettingsButton,
  SettingsConfirmDialog,
  SettingsFieldRow,
  SettingsPage,
  SettingsSection,
  SettingsSidebar,
  SettingsStatus,
  SettingsSwitch,
  SettingsTextInput,
} from "./SettingsPrimitives";

describe("Settings visual primitives", () => {
  it("keeps sidebar selection semantic and delegates navigation", () => {
    const onSelect = vi.fn();
    render(
      <SettingsSidebar
        activeSection="routes"
        onSelect={onSelect}
        version="0.1.1"
        items={[
          { id: "routes", label: "路由", icon: <Route aria-hidden="true" /> },
          { id: "codex", label: "Codex", icon: <span aria-hidden="true" /> },
          {
            id: "system",
            label: "系统",
            icon: <Settings aria-hidden="true" />,
            hasIndicator: true,
          },
        ]}
      />,
    );

    expect(screen.getByRole("button", { name: "路由" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(screen.getByRole("button", { name: "Codex" })).not.toHaveAttribute(
      "aria-current",
    );
    expect(
      screen.getByRole("button", { name: "系统，有可用更新" }),
    ).toContainElement(document.querySelector(".application-update-indicator"));
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    expect(onSelect).toHaveBeenCalledWith("codex");
    expect(screen.getByText("版本 0.1.1")).toBeInTheDocument();
  });

  it("preserves native field, switch, button, and heading semantics", () => {
    const onSwitch = vi.fn();
    render(
      <SettingsPage title="Codex" titleId="codex-title">
        <SettingsSection
          title="本地代理"
          status={<SettingsStatus tone="success">运行中</SettingsStatus>}
        >
          <SettingsFieldRow label="端口" htmlFor="proxy-port">
            <SettingsTextInput
              id="proxy-port"
              type="number"
              defaultValue="18080"
            />
          </SettingsFieldRow>
          <SettingsSwitch label="启用余额查询" checked onChange={onSwitch} />
          <SettingsButton variant="primary">保存</SettingsButton>
        </SettingsSection>
      </SettingsPage>,
    );

    const pageTitle = screen.getByRole("heading", {
      name: "Codex",
      level: 2,
    });
    expect(pageTitle).toHaveAttribute("data-tauri-drag-region");
    expect(pageTitle.parentElement).toHaveClass("settings-page-title-band");
    expect(pageTitle.parentElement).toHaveAttribute("data-tauri-drag-region");
    expect(
      screen.getByRole("heading", { name: "本地代理", level: 3 }),
    ).toBeInTheDocument();
    expect(screen.getByText("运行中")).toHaveClass("settings-status-success");
    expect(screen.getByLabelText("端口")).toHaveValue(18080);
    expect(screen.getByLabelText("端口")).not.toHaveAttribute(
      "data-tauri-drag-region",
    );
    expect(screen.getByRole("switch", { name: "启用余额查询" })).toBeChecked();
    fireEvent.click(screen.getByRole("switch", { name: "启用余额查询" }));
    expect(onSwitch).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "保存" })).toHaveClass(
      "settings-button-primary",
    );
  });

  it("keeps confirmation cancel-first focus and destructive button order", () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    render(
      <SettingsConfirmDialog
        confirmation={{
          title: "放弃未保存的修改？",
          body: "当前设置的修改尚未保存。",
          confirmLabel: "放弃修改",
          cancelLabel: "继续编辑",
          destructive: true,
          onConfirm,
        }}
        onCancel={onCancel}
      />,
    );

    expect(screen.getByRole("alertdialog")).toHaveAccessibleName(
      "放弃未保存的修改？",
    );
    const buttons = screen.getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual([
      "继续编辑",
      "放弃修改",
    ]);
    expect(buttons[0]).toHaveFocus();
    expect(buttons[1]).toHaveClass("settings-button-danger");
    fireEvent.keyDown(buttons[0], { key: "Escape" });
    fireEvent.click(buttons[0]);
    fireEvent.click(buttons[1]);
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });
});
