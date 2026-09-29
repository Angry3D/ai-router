import { act, fireEvent, render, screen } from "@testing-library/react";
import { Route, Settings } from "lucide-react";
import { createRef, useState, type Ref } from "react";
import { describe, expect, it, vi } from "vitest";

import {
  SettingsButton,
  SettingsCombobox,
  SettingsConfirmDialog,
  SettingsFieldRow,
  SettingsPage,
  SettingsSection,
  SettingsSidebar,
  SettingsStatus,
  SettingsSwitch,
  SettingsTextInput,
  type SettingsComboboxOption,
} from "./SettingsPrimitives";

describe("Settings visual primitives", () => {
  it("keeps sidebar selection semantic and delegates navigation", () => {
    const onSelect = vi.fn();
    const onOpenRepository = vi.fn();
    render(
      <SettingsSidebar
        activeSection="routes"
        onSelect={onSelect}
        onOpenRepository={onOpenRepository}
        isRepositoryPending={false}
        version="0.1.1"
        items={[
          { id: "routes", label: "路由", icon: <Route aria-hidden="true" /> },
          { id: "codex", label: "Codex", icon: <span aria-hidden="true" /> },
          {
            id: "system",
            label: "系统",
            icon: <Settings aria-hidden="true" />,
            indicatorLabel: "有可用更新",
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
    ).toContainElement(document.querySelector(".settings-navigation-indicator"));
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    expect(onSelect).toHaveBeenCalledWith("codex");
    expect(screen.getByText("版本 0.1.1")).toBeInTheDocument();
    const repositoryLink = screen.getByRole("button", {
      name: "打开 GitHub 项目",
    });
    expect(repositoryLink).toHaveAttribute("title", "打开 GitHub 项目");
    expect(repositoryLink).toHaveAttribute("type", "button");
    expect(repositoryLink.querySelector(".anticon")).toHaveAttribute(
      "aria-hidden",
      "true",
    );
    fireEvent.click(repositoryLink);
    expect(onOpenRepository).toHaveBeenCalledOnce();
  });

  it("preserves the pending button footprint and native disabled state", () => {
    render(
      <SettingsSidebar
        activeSection="routes"
        onSelect={vi.fn()}
        onOpenRepository={vi.fn()}
        isRepositoryPending
        version="0.1.1"
        items={[
          { id: "routes", label: "路由", icon: <Route aria-hidden="true" /> },
        ]}
      />,
    );

    const repositoryLink = screen.getByRole("button", {
      name: "打开 GitHub 项目",
    });
    expect(repositoryLink).toBeDisabled();
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
    expect(pageTitle.closest(".settings-page-viewport")).toBeNull();
    expect(
      screen
        .getByRole("heading", { name: "本地代理", level: 3 })
        .closest(".settings-page-viewport"),
    ).not.toBeNull();
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

const COMBOBOX_OPTIONS: SettingsComboboxOption[] = [
  { id: "gpt-5.2" },
  { id: "gpt-5.2-codex", disabled: true },
  { id: "gpt-5.2-mini" },
  { id: "gpt-5.3-codex" },
  { id: "glm-5.3-flash" },
];

function optionLabels() {
  return screen
    .queryAllByRole("option")
    .map(
      (option) =>
        option.querySelector(".settings-combobox-option-label")?.textContent,
    );
}

function activeLabel() {
  const activeId = screen
    .getByRole("combobox")
    .getAttribute("aria-activedescendant");
  const active = screen
    .getAllByRole("option")
    .find((option) => option.id === activeId);
  return active?.querySelector(".settings-combobox-option-label")?.textContent;
}

function renderCombobox({
  options = COMBOBOX_OPTIONS,
  ariaLabel = "搜索或输入模型 ID",
  placement,
  disabled,
  invalid,
  describedBy,
  maxLength,
  popupHost,
  inputRef,
}: {
  options?: SettingsComboboxOption[];
  ariaLabel?: string;
  placement?: "below" | "above";
  disabled?: boolean;
  invalid?: boolean;
  describedBy?: string;
  maxLength?: number;
  popupHost?: HTMLElement | null;
  inputRef?: Ref<HTMLInputElement>;
} = {}) {
  const onChange = vi.fn();
  const onSelect = vi.fn();
  const onSubmit = vi.fn();

  function Harness() {
    const [value, setValue] = useState("");
    return (
      <SettingsCombobox
        value={value}
        onChange={(next) => {
          setValue(next);
          onChange(next);
        }}
        options={options}
        onSelect={onSelect}
        onSubmit={onSubmit}
        ariaLabel={ariaLabel}
        placement={placement}
        disabled={disabled}
        invalid={invalid}
        describedBy={describedBy}
        maxLength={maxLength}
        popupHost={popupHost}
        inputRef={inputRef}
      />
    );
  }

  render(<Harness />);

  return { onChange, onSelect, onSubmit, input: screen.getByRole("combobox") };
}

describe("Settings combobox primitive", () => {
  it("opens on focus and filters options by case-insensitive substring", () => {
    const { input } = renderCombobox();
    expect(input).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).toBeNull();

    fireEvent.focus(input);
    expect(input).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("listbox")).toHaveAttribute(
      "data-placement",
      "below",
    );
    expect(input.closest(".settings-combobox")).toContainElement(
      screen.getByRole("listbox"),
    );
    expect(optionLabels()).toEqual([
      "gpt-5.2",
      "gpt-5.2-codex",
      "gpt-5.2-mini",
      "gpt-5.3-codex",
      "glm-5.3-flash",
    ]);

    fireEvent.change(input, { target: { value: "GPT-5.2" } });
    expect(optionLabels()).toEqual([
      "gpt-5.2",
      "gpt-5.2-codex",
      "gpt-5.2-mini",
    ]);

    fireEvent.change(input, { target: { value: "codex" } });
    expect(optionLabels()).toEqual(["gpt-5.2-codex", "gpt-5.3-codex"]);

    fireEvent.change(input, { target: { value: "claude" } });
    expect(optionLabels()).toEqual([]);
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    expect(screen.getByText("无匹配模型")).toHaveClass("settings-combobox-hint");
  });

  it("moves the highlight with arrow keys and skips disabled options", () => {
    const { input } = renderCombobox();
    fireEvent.focus(input);
    expect(activeLabel()).toBe("gpt-5.2");

    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(activeLabel()).toBe("gpt-5.2-mini");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(activeLabel()).toBe("gpt-5.3-codex");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(activeLabel()).toBe("glm-5.3-flash");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(activeLabel()).toBe("glm-5.3-flash");

    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(activeLabel()).toBe("gpt-5.3-codex");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(activeLabel()).toBe("gpt-5.2");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(activeLabel()).toBe("gpt-5.2");
  });

  it("selects the highlighted option with Enter and submits trimmed text otherwise", () => {
    const { input, onSelect, onSubmit } = renderCombobox();
    fireEvent.focus(input);

    fireEvent.change(input, { target: { value: "codex" } });
    expect(activeLabel()).toBe("gpt-5.3-codex");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith("gpt-5.3-codex");
    expect(onSubmit).not.toHaveBeenCalled();

    fireEvent.change(input, { target: { value: "  custom-model  " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledWith("custom-model");
    expect(onSelect).toHaveBeenCalledTimes(1);

    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape, Tab, and blur while keeping the typed text", () => {
    const { input, onChange } = renderCombobox();
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "gpt" } });
    expect(screen.getByRole("listbox")).toBeInTheDocument();

    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(input).toHaveAttribute("aria-expanded", "false");
    expect(input).not.toHaveAttribute("aria-activedescendant");
    expect(input).toHaveValue("gpt");
    expect(onChange).toHaveBeenLastCalledWith("gpt");

    fireEvent.change(input, { target: { value: "gpt-5" } });
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    fireEvent.keyDown(input, { key: "Tab" });
    expect(screen.queryByRole("listbox")).toBeNull();

    fireEvent.focus(input);
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    fireEvent.blur(input);
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(input).toHaveValue("gpt-5");
  });

  it("exposes combobox, listbox, and option roles with the active descendant", () => {
    const { input } = renderCombobox({
      ariaLabel: "搜索或输入模型 ID",
      invalid: true,
    });
    expect(input).toHaveAttribute("role", "combobox");
    expect(input).toHaveAccessibleName("搜索或输入模型 ID");
    expect(input).toHaveAttribute("aria-autocomplete", "list");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).not.toHaveAttribute("aria-describedby");
    const listboxId = input.getAttribute("aria-controls");

    fireEvent.focus(input);
    const options = screen.getAllByRole("option");
    expect(screen.getByRole("listbox")).toHaveAttribute("id", listboxId);
    expect(screen.getAllByRole("option")).toHaveLength(
      COMBOBOX_OPTIONS.length,
    );
    expect(input).toHaveAttribute("aria-activedescendant", options[0].id);
    expect(options.map((option) => option.getAttribute("aria-selected"))).toEqual(
      ["true", "false", "false", "false", "false"],
    );
    expect(options[1]).toHaveAttribute("aria-disabled", "true");
    expect(options[0]).not.toHaveAttribute("aria-disabled");
  });

  it("badges added options, blocks their selection, and keeps input focus", () => {
    const { input, onSelect } = renderCombobox();
    fireEvent.focus(input);
    const [first, added, selectable] = screen.getAllByRole("option");

    expect(added).toHaveAttribute("aria-disabled", "true");
    expect(added).toHaveTextContent("已添加");
    expect(added.querySelector(".settings-combobox-badge")).toHaveTextContent(
      "已添加",
    );

    fireEvent.mouseEnter(added);
    expect(input).toHaveAttribute("aria-activedescendant", first.id);

    expect(fireEvent.mouseDown(added)).toBe(false);
    fireEvent.click(added);
    expect(onSelect).not.toHaveBeenCalled();

    expect(fireEvent.mouseDown(selectable)).toBe(false);
    fireEvent.click(selectable);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith("gpt-5.2-mini");
  });

  it("anchors the popup above when requested", () => {
    const { input } = renderCombobox({ placement: "above" });
    fireEvent.focus(input);
    expect(screen.getByRole("listbox")).toHaveAttribute(
      "data-placement",
      "above",
    );
  });

  it("stays closed without options", () => {
    const { input } = renderCombobox({ options: [] });
    fireEvent.focus(input);
    expect(input).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("stays closed while disabled", () => {
    const { input } = renderCombobox({ disabled: true });
    expect(input).toBeDisabled();
    fireEvent.focus(input);
    expect(input).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("forwards the description and length constraints to the input", () => {
    const { input } = renderCombobox({
      describedBy: "fallback-model-error",
      maxLength: 256,
    });
    expect(input).toHaveAttribute("aria-describedby", "fallback-model-error");
    expect(input).toHaveAttribute("maxlength", "256");
  });

  it("forwards a ref to the internal input", () => {
    const inputRef = createRef<HTMLInputElement>();
    const { input } = renderCombobox({ inputRef });
    expect(inputRef.current).toBe(input);

    act(() => {
      inputRef.current?.focus();
    });
    expect(input).toHaveFocus();
    expect(screen.getByRole("listbox")).toBeInTheDocument();
  });

  it("renders the popup into the supplied host element", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const { input, onSelect } = renderCombobox({ popupHost: host });

    fireEvent.focus(input);
    const listbox = screen.getByRole("listbox");
    expect(host).toContainElement(listbox);
    expect(input.closest(".settings-combobox")).not.toContainElement(listbox);
    expect(input).toHaveAttribute("aria-controls", listbox.id);

    fireEvent.change(input, { target: { value: "codex" } });
    expect(optionLabels()).toEqual(["gpt-5.2-codex", "gpt-5.3-codex"]);
    expect(input).toHaveAttribute(
      "aria-activedescendant",
      screen.getAllByRole("option")[1].id,
    );

    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith("gpt-5.3-codex");

    fireEvent.click(screen.getAllByRole("option")[0]);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(input).toHaveValue("codex");

    host.remove();
  });
});
