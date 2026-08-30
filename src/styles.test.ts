import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const styles = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
const htmlEntrypoints = ["menu.html", "settings.html"].map((file) => ({
  file,
  source: readFileSync(resolve(process.cwd(), file), "utf8"),
}));

describe("cross-window style isolation", () => {
  it("keeps the Settings repository link circular and geometry-stable", () => {
    const footerRule =
      styles.match(/\.settings-nav-footer \{([^}]*)\}/u)?.[1] ?? "";
    const versionRule =
      styles.match(/\.settings-nav-version \{([^}]*)\}/u)?.[1] ?? "";
    const buttonRule =
      styles.match(/\.settings-github-link \{([^}]*)\}/u)?.[1] ?? "";
    const iconRule =
      styles.match(/\.settings-github-link \.anticon \{([^}]*)\}/u)?.[1] ?? "";
    const hoverRule =
      styles.match(
        /\.settings-github-link:hover:not\(:disabled\) \{([^}]*)\}/u,
      )?.[1] ?? "";
    const focusRule =
      styles.match(/\.settings-github-link:focus-visible \{([^}]*)\}/u)?.[1] ??
      "";

    expect(footerRule).toContain("display: flex;");
    expect(footerRule).toContain("min-width: 0;");
    expect(footerRule).toContain("align-items: center;");
    expect(footerRule).toContain("gap: 8px;");
    expect(footerRule).toContain("margin: auto 8px 0;");
    expect(versionRule).toContain("min-width: 0;");
    expect(versionRule).toContain("flex: 1 1 auto;");
    expect(versionRule).toContain("margin: 0;");
    expect(buttonRule).toContain("width: 24px;");
    expect(buttonRule).toContain("height: 24px;");
    expect(buttonRule).toContain("flex: 0 0 24px;");
    expect(buttonRule).toContain("background: transparent;");
    expect(buttonRule).toContain("border-radius: 50%;");
    expect(iconRule).toContain("font-size: 18px;");
    expect(hoverRule).toContain("background: transparent;");
    expect(focusRule).toContain("background: transparent;");
    expect(focusRule).toContain("outline: 2px solid var(--blue);");
  });

  it("constrains Settings page scroll areas to the available grid row", () => {
    const pageRule = styles.match(/\.settings-page \{([^}]*)\}/u)?.[1] ?? "";
    const rootRule =
      styles.match(/\.settings-page-scroll \{([^}]*)\}/u)?.[1] ?? "";
    const viewportRule =
      styles.match(/\.settings-page-viewport \{([^}]*)\}/u)?.[1] ?? "";

    expect(pageRule).toContain("min-height: 0;");
    expect(pageRule).toContain("height: 100%;");
    expect(pageRule).toContain("overflow: hidden;");
    expect(rootRule).toContain("height: 100%;");
    expect(viewportRule).toContain("height: 100%;");
  });

  it("renders Settings required markers with the shared danger color", () => {
    const markerRule =
      styles.match(/\.settings-required-marker \{([^}]*)\}/u)?.[1] ?? "";
    const glyphRule =
      styles.match(/\.settings-required-marker::before \{([^}]*)\}/u)?.[1] ??
      "";

    expect(markerRule).toContain("display: inline-block;");
    expect(markerRule).toContain("color: var(--red);");
    expect(glyphRule).toContain('content: "*";');
  });

  it("preserves the Usage page flex layout inside the Radix content wrapper", () => {
    const contentRule =
      styles.match(
        /\.usage-page > \.settings-page-scroll > \.settings-page-viewport > div \{([^}]*)\}/u,
      )?.[1] ?? "";
    const tableRule =
      styles.match(/\.usage-table-wrap \{([^}]*)\}/u)?.[1] ?? "";
    const detailRule = styles.match(/\.usage-detail \{([^}]*)\}/u)?.[1] ?? "";

    expect(contentRule).toContain("display: flex !important;");
    expect(contentRule).toContain("height: 100%;");
    expect(contentRule).toContain("min-height: 0;");
    expect(contentRule).toContain("flex-direction: column;");
    expect(tableRule).toContain("flex: 1 1 auto;");
    expect(detailRule).toContain("flex: 1 1 auto;");
  });

  it("keeps Fallback decisions full-width, wrapping, and geometry-stable", () => {
    const decisionRule =
      styles.match(/\.usage-routing-decision \{([^}]*)\}/u)?.[1] ?? "";
    const iconRule =
      styles.match(/\.usage-routing-decision svg \{([^}]*)\}/u)?.[1] ?? "";
    const copyRule =
      styles.match(/\.usage-routing-decision span \{([^}]*)\}/u)?.[1] ?? "";

    expect(decisionRule).toContain(
      "grid-template-columns: 16px minmax(0, 1fr);",
    );
    expect(decisionRule).toContain("margin-top: 10px;");
    expect(decisionRule).toContain("padding: 7px 9px;");
    expect(decisionRule).toContain("border-radius: 4px;");
    expect(decisionRule).toContain("font-size: 13px;");
    expect(decisionRule).toContain("line-height: 18px;");
    expect(iconRule).toContain("width: 16px;");
    expect(copyRule).toContain("overflow-wrap: anywhere;");
  });

  it("bounds the Usage statistics scroller and keeps its lower grid equal at the minimum viewport", () => {
    const shellRule =
      styles.match(
        /\.usage-statistics-panel-shell,\n\.usage-statistics-panel \{([^}]*)\}/u,
      )?.[1] ?? "";
    const panelRule =
      styles.match(/\n\n\.usage-statistics-panel \{([^}]*)\}/u)?.[1] ?? "";
    const viewportRule =
      styles.match(/\.usage-statistics-panel-viewport \{([^}]*)\}/u)?.[1] ?? "";
    const lowerGridRule =
      styles.match(/\.usage-statistics-lower-grid \{([^}]*)\}/u)?.[1] ?? "";

    expect(shellRule).toContain("min-width: 0;");
    expect(shellRule).toContain("min-height: 0;");
    expect(shellRule).toContain("overflow: hidden;");
    expect(panelRule).toContain("height: 100%;");
    expect(panelRule).toContain("flex: 1 1 auto;");
    expect(viewportRule).toContain("height: 100%;");
    expect(viewportRule).toContain("overflow-x: hidden;");
    expect(lowerGridRule).toContain(
      "grid-template-columns: repeat(2, minmax(0, 1fr));",
    );
    expect(styles).not.toMatch(
      /@media \(max-width: 800px\) \{[\s\S]*?\.usage-statistics-lower-grid \{/u,
    );
  });

  it("keeps the Usage tabs and actions in one stable toolbar", () => {
    const toolbarRule =
      styles.match(/\.usage-filter-toolbar \{([^}]*)\}/u)?.[1] ?? "";
    const tabsRule = styles.match(/\.usage-tabs \{([^}]*)\}/u)?.[1] ?? "";
    const actionsRule =
      styles.match(/\.usage-filter-actions \{([^}]*)\}/u)?.[1] ?? "";

    expect(toolbarRule).toContain("height: 42px;");
    expect(toolbarRule).toContain(
      "grid-template-columns: minmax(0, 1fr) auto;",
    );
    expect(toolbarRule).toContain(
      "border-bottom: 1px solid var(--settings-divider);",
    );
    expect(tabsRule).toContain("height: 100%;");
    expect(actionsRule).toContain("display: flex;");
  });

  it("extends title drag bands across the complete Settings top strip", () => {
    const titleBandRule =
      styles.match(/\.settings-page-title-band \{([^}]*)\}/u)?.[1] ?? "";
    const routeTitleBandRule =
      styles.match(
        /\.route-list-pane > header > \.settings-page-title-band \{([^}]*)\}/u,
      )?.[1] ?? "";
    const routeTopDragRule =
      styles.match(/\.route-list-top-drag-region \{([^}]*)\}/u)?.[1] ?? "";
    const contentTitleBandRule =
      styles.match(
        /\.settings-page \.settings-page-title-band,\s*\.route-form-fields > \.settings-page-title-band \{([^}]*)\}/u,
      )?.[1] ?? "";

    expect(titleBandRule).toContain("width: 100%;");
    expect(titleBandRule).toContain("flex: 0 0 auto;");
    expect(routeTitleBandRule).toContain("width: auto;");
    expect(routeTitleBandRule).toContain(
      "height: var(--settings-title-drag-height);",
    );
    expect(routeTitleBandRule).toContain("margin-left: -18px;");
    expect(routeTitleBandRule).toContain("flex: 1 1 auto;");
    expect(routeTopDragRule).toContain("position: absolute;");
    expect(routeTopDragRule).toContain("inset: 0;");
    expect(contentTitleBandRule).toContain(
      "min-height: var(--settings-title-drag-height);",
    );
    expect(contentTitleBandRule).toContain(
      "padding-inline: var(--settings-page-padding-inline);",
    );
    expect(contentTitleBandRule).toContain(
      "margin-top: calc(0px - var(--settings-page-padding-block));",
    );
    expect(contentTitleBandRule).toContain(
      "margin-inline: calc(0px - var(--settings-page-padding-inline));",
    );
  });

  it("shares donut host geometry and limits 200px widths to image controls", () => {
    const donutRule =
      styles.match(/\.usage-donut-chart \{([^}]*)\}/u)?.[1] ?? "";
    const routeRule =
      styles.match(/\.images-generation-route-select \{([^}]*)\}/u)?.[1] ?? "";
    const timeoutRule =
      styles.match(/\.images-generation-timeout-control \{([^}]*)\}/u)?.[1] ??
      "";
    const sharedParameterRule =
      styles.match(/\.parameter-input-control \{([^}]*)\}/u)?.[1] ?? "";

    expect(donutRule).toContain("height: 220px;");
    expect(donutRule).toContain("margin-top: 10px;");
    expect(routeRule).toContain("width: min(200px, 100%);");
    expect(timeoutRule).toContain(
      "grid-template-columns: minmax(0, 200px) auto;",
    );
    expect(sharedParameterRule).toContain(
      "grid-template-columns: minmax(0, 120px) auto;",
    );
  });

  it("lets each Radix scrollbar orientation own only its cross-axis size", () => {
    const baseRule =
      styles.match(/\.app-scroll-scrollbar \{([^}]*)\}/u)?.[1] ?? "";
    const verticalRule =
      styles.match(
        /\.app-scroll-scrollbar\[data-orientation="vertical"\] \{([^}]*)\}/u,
      )?.[1] ?? "";
    const horizontalRule =
      styles.match(
        /\.app-scroll-scrollbar\[data-orientation="horizontal"\] \{([^}]*)\}/u,
      )?.[1] ?? "";

    expect(baseRule).not.toMatch(/\b(?:width|height):/u);
    expect(verticalRule).toContain("width: 10px;");
    expect(horizontalRule).toContain("height: 10px;");
  });

  it("hides native scrollbars through the scoped static viewport contract", () => {
    const viewportRule =
      styles.match(/\.app-scroll-viewport \{([^}]*)\}/u)?.[1] ?? "";
    const webkitRule =
      styles.match(
        /\.app-scroll-viewport::-webkit-scrollbar \{([^}]*)\}/u,
      )?.[1] ?? "";

    expect(viewportRule).toContain("scrollbar-width: none;");
    expect(viewportRule).toContain("-ms-overflow-style: none;");
    expect(webkitRule).toContain("width: 0;");
    expect(webkitRule).toContain("height: 0;");
    expect(webkitRule).toContain("display: none;");
    expect(styles).not.toMatch(
      /(?:^|\n)\s*(?:\*|html|body)?::-webkit-scrollbar\s*\{/u,
    );
  });

  it.each(htmlEntrypoints)(
    "provides the Tauri style nonce sentinel in $file",
    ({ source }) => {
      expect(source).toContain('<style id="app-csp-style-nonce"></style>');
    },
  );

  it("keeps runtime controls compact and swaps copy in one stable grid cell", () => {
    const statusRowRule =
      styles.match(/\.menu-runtime-status \{([^}]*)\}/u)?.[1] ?? "";
    const controlRule =
      styles.match(/\.runtime-control \{([^}]*)\}/u)?.[1] ?? "";
    const copyRule =
      styles.match(/\.runtime-control-copy \{([^}]*)\}/u)?.[1] ?? "";
    const copiesRule =
      styles.match(
        /\.runtime-control-state,\n\.runtime-control-action,\n\.runtime-control-pending \{([^}]*)\}/u,
      )?.[1] ?? "";
    const hoverRule =
      styles.match(
        /\.runtime-control:hover:not\(:disabled\),\n\.runtime-control:focus-visible \{([^}]*)\}/u,
      )?.[1] ?? "";

    expect(statusRowRule).toContain("min-width: 0;");
    expect(controlRule).toContain("height: 24px;");
    expect(controlRule).toContain("flex: 0 0 auto;");
    expect(controlRule).toContain("border-radius: 5px;");
    expect(copyRule).toContain("display: grid;");
    expect(copiesRule).toContain("grid-area: 1 / 1;");
    expect(styles).toContain(
      ".runtime-control:hover:not(.is-pending) .runtime-control-state,",
    );
    expect(styles).toContain(
      ".runtime-control:focus-visible:not(.is-pending) .runtime-control-action,",
    );
    expect(styles).toContain(
      ".runtime-control.is-pending .runtime-control-pending {",
    );
    expect(hoverRule).not.toMatch(/\b(?:width|height|padding|margin|gap):/u);
    expect(styles).toContain('.runtime-control[aria-disabled="true"],');
    expect(styles).toContain(".runtime-control:disabled {");
    expect(styles).not.toContain(".menu-connect-button");
  });

  it("keeps the Fallback boundary stable and limits drag capture to the bar", () => {
    const boundaryRule =
      styles.match(/\.fallback-boundary \{([^}]*)\}/u)?.[1] ?? "";
    const boundarySlotRule =
      styles.match(/\.fallback-boundary-slot \{([^}]*)\}/u)?.[1] ?? "";
    const routeViewportRule =
      styles.match(/\.settings-route-list-viewport \{([^}]*)\}/u)?.[1] ?? "";
    const routeViewportContentRule =
      styles.match(
        /\.app-scroll-viewport\.settings-route-list-viewport > div \{([^}]*)\}/u,
      )?.[1] ?? "";
    const labelRule =
      styles.match(/\.fallback-boundary-label \{([^}]*)\}/u)?.[1] ?? "";
    const railRule =
      styles.match(/\.fallback-boundary-rail \{([^}]*)\}/u)?.[1] ?? "";
    const focusRule =
      styles.match(/\.fallback-boundary:focus-visible \{([^}]*)\}/u)?.[1] ?? "";
    const sourcePlaceholderRule =
      styles.match(
        /\.fallback-boundary-slot\.is-source-placeholder \.fallback-boundary \{([^}]*)\}/u,
      )?.[1] ?? "";
    const dragOverlayRule =
      styles.match(/\.fallback-boundary\.is-drag-overlay \{([^}]*)\}/u)?.[1] ??
      "";
    const routeHandleRule =
      styles.match(/\.settings-route-drag-handle \{([^}]*)\}/u)?.[1] ?? "";
    const routeSelectRule =
      styles.match(/\.settings-route-select \{([^}]*)\}/u)?.[1] ?? "";

    expect(boundaryRule).toContain("display: flex;");
    expect(boundaryRule).toContain("height: 29px;");
    expect(boundaryRule).toContain("color: var(--muted);");
    expect(boundaryRule).toContain("background: var(--surface);");
    expect(boundaryRule).toContain("touch-action: none;");
    expect(boundaryRule).toContain("user-select: none;");
    expect(boundarySlotRule).toContain("min-width: 0;");
    expect(boundarySlotRule).not.toMatch(
      /\b(?:height|min-height|padding|margin|border|gap):/u,
    );
    expect(labelRule).toContain("pointer-events: none;");
    expect(railRule).toContain("pointer-events: none;");
    expect(focusRule).toContain("color: var(--blue);");
    expect(focusRule).toContain("box-shadow: inset 0 0 0 2px var(--blue);");
    expect(sourcePlaceholderRule).toContain("opacity: 0.24;");
    expect(dragOverlayRule).toContain("pointer-events: none;");
    expect(routeHandleRule).toContain("touch-action: none;");
    expect(routeHandleRule).toContain("cursor: grab;");
    expect(routeHandleRule).toContain("margin-left: 8px;");
    expect(routeHandleRule).not.toContain("margin-right:");
    expect(routeSelectRule).toContain("padding: 8px 8px 8px 4px;");
    expect(routeViewportRule).toContain("height: 100%;");
    expect(routeViewportRule).not.toContain("overflow: hidden;");
    expect(routeViewportContentRule).toContain("display: flex !important;");
    expect(routeViewportContentRule).toContain("min-width: 100% !important;");
    expect(routeViewportContentRule).toContain("flex-direction: column;");
  });

  it("keeps the Routes Fallback help above the fixed toolbar without theme forks", () => {
    const paneRule = styles.match(/\.route-list-pane \{([^}]*)\}/u)?.[1] ?? "";
    const toolbarRule = styles.match(/\.route-tools \{([^}]*)\}/u)?.[1] ?? "";
    const groupRule =
      styles.match(/\.route-fallback-switch-help \{([^}]*)\}/u)?.[1] ?? "";
    const orderRule =
      styles.match(/\.route-order-tools \{([^}]*)\}/u)?.[1] ?? "";
    const orderButtonRule =
      styles.match(
        /\.route-order-tools \.settings-icon-button \{([^}]*)\}/u,
      )?.[1] ?? "";
    const tooltipRule =
      styles.match(
        /\.route-fallback-switch-help \.settings-help-tooltip-content \{([^}]*)\}/u,
      )?.[1] ?? "";
    const switchRule =
      styles.match(
        /\.route-tools \.settings-switch-control \{([^}]*)\}/u,
      )?.[1] ?? "";

    expect(paneRule).toContain("grid-template-rows: 62px minmax(0, 1fr) 50px;");
    expect(toolbarRule).toContain("padding: 8px 10px;");
    expect(toolbarRule).toContain("gap: 4px;");
    expect(orderRule).toContain("gap: 2px;");
    expect(orderButtonRule).toContain("width: 28px;");
    expect(orderButtonRule).toContain("height: 28px;");
    expect(orderButtonRule).toContain("flex-basis: 28px;");
    expect(groupRule).toContain("display: flex;");
    expect(groupRule).toContain("min-width: 0;");
    expect(groupRule).toContain("align-items: center;");
    expect(groupRule).toContain("gap: 3px;");
    expect(groupRule).toContain("padding-left: 4px;");
    expect(groupRule).not.toContain("flex: 0 0 auto;");
    expect(switchRule).toContain("margin: 0;");
    expect(tooltipRule).toContain("top: auto;");
    expect(tooltipRule).toContain("right: 0;");
    expect(tooltipRule).toContain("bottom: calc(100% + 7px);");
    expect(tooltipRule).toContain("left: auto;");
    expect(tooltipRule).toContain("width: 200px;");
    const tooltipListRule =
      styles.match(
        /\.route-fallback-switch-help \.settings-help-tooltip-content ul \{([^}]*)\}/u,
      )?.[1] ?? "";
    expect(tooltipListRule).toContain("margin: 0;");
    expect(tooltipListRule).toContain("padding-left: 16px;");
    expect(tooltipListRule).toContain("list-style: disc;");
    expect(styles).not.toMatch(
      /:root\[data-theme="(?:light|dark)"\][^{]*\.route-fallback-switch-help/u,
    );
  });

  it("keeps the route balance result in a shrinkable trailing action cluster", () => {
    const actionRule =
      styles.match(/\.route-script-actions \{([^}]*)\}/u)?.[1] ?? "";
    const clusterRule =
      styles.match(/\.route-balance-actions \{([^}]*)\}/u)?.[1] ?? "";
    const resultRule =
      styles.match(
        /\.route-balance-actions \.balance-test-result \{([^}]*)\}/u,
      )?.[1] ?? "";
    const buttonRule =
      styles.match(
        /\.route-balance-actions \.settings-button-primary \{([^}]*)\}/u,
      )?.[1] ?? "";

    expect(actionRule).toContain("align-items: center;");
    expect(clusterRule).toContain("display: flex;");
    expect(clusterRule).toContain("min-width: 0;");
    expect(clusterRule).toContain("max-width: 100%;");
    expect(clusterRule).toContain("margin-left: auto;");
    expect(resultRule).toContain("min-width: 0;");
    expect(resultRule).toContain("overflow-wrap: anywhere;");
    expect(resultRule).toContain("text-align: right;");
    expect(buttonRule).toContain("flex: 0 0 auto;");
  });

  it("keeps both Fast statuses compact, outlined, and on one line", () => {
    expect(styles).toMatch(
      /\.usage-cost-tier \{[\s\S]*?justify-self: start;[\s\S]*?padding: 0 4px;[\s\S]*?color: var\(--blue\);[\s\S]*?border: 1px solid currentColor;[\s\S]*?border-radius: 4px;[\s\S]*?font-size: 10px;[\s\S]*?white-space: nowrap;/u,
    );
    expect(styles).toMatch(
      /\.usage-cost-tier-unconfirmed \{[\s\S]*?color: var\(--orange\);[\s\S]*?\}/u,
    );
  });

  it("caps the route scroller at six complete rows without stretching header or footer", () => {
    const menuShellRule = styles.match(/\.menu-shell \{([^}]*)\}/u)?.[1] ?? "";
    const routeScrollerRule =
      styles.match(/\.menu-routes \{([^}]*)\}/u)?.[1] ?? "";
    const routeViewportRule =
      styles.match(/\.menu-routes-viewport \{([^}]*)\}/u)?.[1] ?? "";
    const routeRowRule =
      styles.match(/\.menu-route-row \{([^}]*)\}/u)?.[1] ?? "";
    const maxHeight = Number(
      routeViewportRule.match(/max-height: (\d+)px;/u)?.[1],
    );
    const padding = Number(routeViewportRule.match(/padding: (\d+)px;/u)?.[1]);
    const rowHeight = Number(routeRowRule.match(/min-height: (\d+)px;/u)?.[1]);

    expect(menuShellRule).toContain("display: flex;");
    expect(menuShellRule).toContain("flex-direction: column;");
    expect(menuShellRule).toContain("min-height: 188px;");
    expect(routeScrollerRule).toContain("min-height: 0;");
    expect(routeScrollerRule).toContain("flex: 1 1 auto;");
    expect(routeViewportRule).toContain("overscroll-behavior: contain;");
    expect(rowHeight).toBe(45);
    expect(padding).toBe(8);
    expect(maxHeight).toBe(286);
    expect(rowHeight * 6 + padding * 2).toBe(maxHeight);
    expect(rowHeight * 7 + padding * 2).toBeGreaterThan(maxHeight);
  });

  it("keeps the full route-name column as the bounded preview trigger", () => {
    const routeSelectRule =
      styles.match(/\.route-select \{([^}]*)\}/u)?.[1] ?? "";
    const nameTrackRule =
      styles.match(/\.route-identity \{([^}]*)\}/u)?.[1] ?? "";
    const nameRule =
      styles.match(/\.route-identity strong \{([^}]*)\}/u)?.[1] ?? "";
    const truncationRule =
      styles.match(/\.route-identity strong,\n\.inference \{([^}]*)\}/u)?.[1] ??
      "";

    expect(routeSelectRule).toContain(
      "grid-template-columns: 18px minmax(0, 1fr) 66px 86px;",
    );
    expect(nameTrackRule).toContain("display: flex;");
    expect(nameTrackRule).toContain("align-self: stretch;");
    expect(nameTrackRule).toContain("align-items: center;");
    expect(nameTrackRule).toContain("min-width: 0;");
    expect(nameTrackRule).toContain("overflow: hidden;");
    expect(nameRule).toContain("min-width: 0;");
    expect(nameRule).toContain("flex: 1;");
    expect(nameRule).toContain("display: block;");
    expect(nameRule).not.toContain("width: fit-content;");
    expect(truncationRule).toContain("overflow: hidden;");
    expect(truncationRule).toContain("text-overflow: ellipsis;");
    expect(truncationRule).toContain("white-space: nowrap;");
  });

  it("keeps route usage preview geometry and motion bounded without moving the menu", () => {
    const layoutRule =
      styles.match(/\.menu-window-layout \{([^}]*)\}/u)?.[1] ?? "";
    const shellRule = styles.match(/\.menu-shell \{([^}]*)\}/u)?.[1] ?? "";
    const previewRule =
      styles.match(/\.menu-usage-preview \{([^}]*)\}/u)?.[1] ?? "";
    const headingRule =
      styles.match(/\.menu-usage-preview-heading \{([^}]*)\}/u)?.[1] ?? "";
    const closingRule =
      styles.match(/(?:^|\n)\.menu-usage-preview-closing \{([^}]*)\}/u)?.[1] ??
      "";
    const totalTokenRule =
      styles.match(/\.usage-token-total-cell > span \{([^}]*)\}/u)?.[1] ?? "";
    const firstOutputRule =
      styles.match(
        /\.usage-table-preview \.usage-first-output-cell \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewHeaderRule =
      styles.match(/\.usage-table-preview th \{([^}]*)\}/u)?.[1] ?? "";
    const previewEdgeHeaderRule =
      styles.match(
        /\.usage-table-preview th:first-child,\n\.usage-table-preview th:last-child \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewHeaderExtensionRule =
      styles.match(
        /\.usage-table-preview th:first-child::before,\n\.usage-table-preview th:last-child::after \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewFirstHeaderExtensionRule =
      styles.match(
        /\.usage-table-preview th:first-child::before \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewLastHeaderExtensionRule =
      styles.match(
        /(?:^|\n\n)\.usage-table-preview th:last-child::after \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewCellRule =
      styles.match(/\.usage-table-preview td \{([^}]*)\}/u)?.[1] ?? "";
    const standardCellRule =
      styles.match(/\.usage-table th,\n\.usage-table td \{([^}]*)\}/u)?.[1] ??
      "";
    const standardHeaderRule =
      styles.match(/(?:^|\n)\.usage-table th \{([^}]*)\}/u)?.[1] ?? "";
    const previewTableRule =
      styles.match(/\.usage-table-preview \{([^}]*)\}/u)?.[1] ?? "";
    const previewMetricRule =
      styles.match(
        /\.usage-table-preview \.usage-metric-cell \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewFastRule =
      styles.match(
        /\.usage-table-preview \.usage-cost-tier,\n\.usage-table-preview \.usage-cost-tier-unconfirmed \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewModelRule =
      styles.match(/\.usage-preview-request-model \{([^}]*)\}/u)?.[1] ?? "";
    const completedPreviewModelRule =
      styles.match(
        /\.usage-preview-row-completed \.usage-preview-request-model \{([^}]*)\}/u,
      )?.[1] ?? "";
    const failedPreviewModelRule =
      styles.match(
        /\.usage-preview-row-failed \.usage-preview-request-model \{([^}]*)\}/u,
      )?.[1] ?? "";
    const cancelledPreviewModelRule =
      styles.match(
        /\.usage-preview-row-cancelled \.usage-preview-request-model,\n\.usage-preview-row-no_upstream \.usage-preview-request-model \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewCostRule =
      styles.match(
        /\.usage-table-preview \.usage-cost-cell > span:first-child,\n\.usage-table-preview \.usage-cost-exact > span:first-child,\n\.usage-table-preview \.usage-cost-partial > span:first-child \{([^}]*)\}/u,
      )?.[1] ?? "";
    const previewRailRule =
      styles.match(
        /\.usage-preview-row td:first-child::before \{([^}]*)\}/u,
      )?.[1] ?? "";

    expect(layoutRule).toContain("position: relative;");
    expect(layoutRule).toContain("gap: 8px;");
    expect(layoutRule).toContain("height: auto;");
    expect(layoutRule).toContain("align-items: stretch;");
    expect(shellRule).toContain("width: 360px;");
    expect(shellRule).toContain("flex: 0 0 360px;");
    expect(shellRule).toContain("border-radius: 14px;");
    expect(previewRule).toContain("position: absolute;");
    expect(previewRule).toContain("width: 344px;");
    expect(previewRule).toContain("top: 0;");
    expect(previewRule).toContain("bottom: 0;");
    expect(previewRule).toContain("min-height: 383px;");
    expect(previewRule).toContain("max-height: var(--menu-preview-height);");
    expect(previewRule).toContain("border-radius: 14px;");
    expect(previewRule).toContain("opacity 180ms ease-out");
    expect(headingRule).toContain("height: 36px;");
    expect(previewTableRule).toContain("margin: 0 8px;");
    expect(previewHeaderRule).toContain("background: var(--surface-subtle);");
    expect(previewEdgeHeaderRule).toContain("overflow: visible;");
    expect(previewHeaderExtensionRule).toContain("top: 0;");
    expect(previewHeaderExtensionRule).toContain("bottom: -1px;");
    expect(previewHeaderExtensionRule).toContain("width: 8px;");
    expect(previewHeaderExtensionRule).toContain(
      "background: var(--surface-subtle);",
    );
    expect(previewHeaderExtensionRule).toContain(
      "border-bottom: 1px solid var(--settings-divider, var(--border));",
    );
    expect(previewFirstHeaderExtensionRule).toContain("right: 100%;");
    expect(previewLastHeaderExtensionRule).toContain("left: 100%;");
    expect(closingRule).toContain("transition-duration: 100ms;");
    const leftArrowRule =
      styles.match(
        /\.menu-window-layout-left \.menu-arrow \{([^}]*)\}/u,
      )?.[1] ?? "";
    expect(leftArrowRule).toContain(
      "right: calc(384px - var(--menu-arrow-x));",
    );
    expect(leftArrowRule).toContain("left: auto;");
    expect(leftArrowRule).toContain(
      "transform: translateX(50%) rotate(45deg);",
    );
    expect(styles).not.toContain("data-preview-open");
    expect(styles).toContain("right: 368px;");
    expect(styles).toContain("left: 368px;");
    expect(previewTableRule).toContain("width: 328px;");
    expect(previewTableRule).toContain("min-width: 328px;");
    expect(previewTableRule).toContain("margin: 0 8px;");
    expect(previewHeaderRule).toContain("height: 26px;");
    expect(previewCellRule).toContain("height: 40px;");
    expect(previewMetricRule).toContain("height: 32px;");
    expect(standardCellRule).toContain("height: 54px;");
    expect(standardHeaderRule).toContain("height: 34px;");
    expect(styles).toContain("width: 136px;");
    expect(styles).toContain("width: 54px;");
    expect(styles).toContain("width: 76px;");
    expect(styles).toContain("width: 62px;");
    expect(previewModelRule).toContain("color: var(--text);");
    expect(completedPreviewModelRule).toContain("color: var(--green);");
    expect(failedPreviewModelRule).toContain("color: var(--red);");
    expect(cancelledPreviewModelRule).toContain("color: var(--orange);");
    expect(previewCostRule).toContain("color: var(--text);");
    expect(previewFastRule).toContain("color: var(--muted);");
    expect(previewFastRule).toContain("border: 0;");
    expect(previewRailRule).toContain("width: 4px;");
    expect(previewRailRule).toContain("height: 16px;");
    expect(previewRailRule).toContain("top: 12px;");
    expect(totalTokenRule).toContain("font-variant-numeric: tabular-nums;");
    expect(totalTokenRule).toContain("font-weight: 600;");
    expect(firstOutputRule).toContain("display: flex;");
    expect(firstOutputRule).toContain("align-items: center;");
    expect(firstOutputRule).not.toContain("grid-template");
    expect(styles).toContain("@media (prefers-reduced-motion: reduce)");
    expect(styles).not.toMatch(/\.menu-shell[^}]*transform:/u);
  });

  it("overlays one passive full-width Fallback boundary without changing row capacity", () => {
    const boundaryRule =
      styles.match(/\.menu-fallback-boundary \{([^}]*)\}/u)?.[1] ?? "";
    const startRule =
      styles.match(/\.menu-fallback-boundary-start \{([^}]*)\}/u)?.[1] ?? "";
    const endRule =
      styles.match(/\.menu-fallback-boundary-end \{([^}]*)\}/u)?.[1] ?? "";
    const labelRule =
      styles.match(/\.menu-fallback-boundary > span \{([^}]*)\}/u)?.[1] ?? "";
    const disabledRule =
      styles.match(/\.menu-fallback-boundary\.is-disabled \{([^}]*)\}/u)?.[1] ??
      "";

    expect(boundaryRule).toContain("position: absolute;");
    expect(boundaryRule).toContain("right: 0;");
    expect(boundaryRule).toContain("left: 0;");
    expect(boundaryRule).toContain("display: flex;");
    expect(boundaryRule).toContain("height: 1px;");
    expect(boundaryRule).toContain("justify-content: center;");
    expect(boundaryRule).toContain("pointer-events: none;");
    expect(boundaryRule).toContain("user-select: none;");
    expect(boundaryRule).not.toMatch(/\b(?:cursor|overflow|transition):/u);
    expect(startRule).toContain("top: 0;");
    expect(endRule).toContain("bottom: 0;");
    expect(labelRule).toContain("background: var(--surface);");
    expect(labelRule).toContain("white-space: nowrap;");
    expect(disabledRule).toContain("color: var(--muted);");
    expect(styles).toContain(
      ".menu-route-row-fallback-boundary-end::after {\n  content: none;",
    );
  });

  it("scopes legacy menu controls to the menu view", () => {
    expect(styles).toContain('body[data-view="menu"] .icon-button {');
    expect(styles).toContain('body[data-view="menu"] .primary-button {');
    expect(styles).not.toMatch(/(?:^|\n)\.icon-button\s*\{/);
    expect(styles).not.toMatch(/(?:^|\n)\.primary-button\s*\{/);
  });

  it("bounds confirmation dimming to the menu card without changing Settings", () => {
    expect(styles).toContain(".dialog-backdrop {\n  position: fixed;");
    expect(styles).toContain(
      'body[data-view="menu"] .dialog-backdrop {\n  position: absolute;',
    );
    expect(styles).toContain(".menu-shell-confirming .menu-arrow {");
    expect(styles).toContain("z-index: 21;");
    expect(styles).toContain("filter: brightness(80%);");
  });
});
