import type { UsageHistoryPageDto } from "../../generated";
import { AppScrollArea } from "../shared/AppScrollArea";
import {
  USAGE_PREVIEW_COLUMNS,
  UsageRecordTable,
} from "../shared/UsageRecordTable";
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
  return (
    <aside
      className={`menu-usage-preview menu-usage-preview-${phase}`}
      aria-label={`${routeName} 用量速览`}
      onPointerEnter={onPointerEnter}
      onPointerLeave={onPointerLeave}
    >
      <header className="menu-usage-preview-heading">
        <strong title={routeName}>{routeName}</strong>
        <span>最近 10 条</span>
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
                  aria-label="正在读取请求记录"
                >
                  {Array.from({ length: 10 }, (_, index) => (
                    <span key={index} />
                  ))}
                </div>
              ) : error ? (
                <div className="menu-usage-preview-message" role="alert">
                  请求记录读取失败
                </div>
              ) : (
                <div className="menu-usage-preview-message" role="status">
                  暂无请求记录
                </div>
              )
            }
          />
        </AppScrollArea>
      </div>
    </aside>
  );
}
