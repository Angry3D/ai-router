import type { UsageHistoryPageDto } from "../../generated";
import { useLanguage } from "../language/useLanguage";
import { AppScrollArea } from "../shared/AppScrollArea";
import { UsageRecordTable } from "../shared/UsageRecordTable";
import { USAGE_PREVIEW_COLUMNS } from "../shared/usageRecordColumns";
import type { MenuUsagePreviewPhase } from "./useMenuUsagePreview";

export function MenuUsagePreview({
  routeName,
  phase,
  data,
  pending,
  error,
  onPointerEnter,
  onPointerLeave,
}: {
  routeName: string;
  phase: MenuUsagePreviewPhase;
  data: UsageHistoryPageDto | undefined;
  pending: boolean;
  error: boolean;
  onPointerEnter: () => void;
  onPointerLeave: () => void;
}) {
  const { messages } = useLanguage();
  const menuMessages = messages.menu;
  return (
    <aside
      className={`menu-usage-preview menu-usage-preview-${phase}`}
      aria-label={menuMessages.usagePreview.label(routeName)}
      onPointerEnter={onPointerEnter}
      onPointerLeave={onPointerLeave}
    >
      <header className="menu-usage-preview-heading">
        <strong title={routeName}>{routeName}</strong>
        <span>{menuMessages.usagePreview.recentCount}</span>
      </header>
      <div className="menu-usage-preview-body">
        <AppScrollArea
          axis="both"
          className="menu-usage-preview-table-wrap"
          viewportClassName="menu-usage-preview-table-viewport"
        >
          <UsageRecordTable
            rows={!pending && !error ? (data?.rows ?? []) : []}
            columns={USAGE_PREVIEW_COLUMNS}
            bodyFallback={
              pending ? (
                <div
                  className="menu-usage-preview-skeleton"
                  role="status"
                  aria-label={menuMessages.usagePreview.loading}
                >
                  {Array.from({ length: 10 }, (_, index) => (
                    <span key={index} />
                  ))}
                </div>
              ) : error ? (
                <div className="menu-usage-preview-message" role="alert">
                  {menuMessages.usagePreview.loadFailed}
                </div>
              ) : (
                <div className="menu-usage-preview-message" role="status">
                  {menuMessages.usagePreview.empty}
                </div>
              )
            }
          />
        </AppScrollArea>
      </div>
    </aside>
  );
}
