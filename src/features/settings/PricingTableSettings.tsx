import { LoaderCircle, RefreshCw } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { openPricingSource, syncPricingFromWeb } from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type { PricingTableDto } from "../../generated";
import { pricingPresentation } from "./pricingPresentation";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsSection,
  SettingsStatus,
} from "./SettingsPrimitives";

const MONEY_SCALE = 1_000_000;

/** Micro-USD per million Tokens as the exact decimal money string pricing uses. */
function formatMoney(microUsd: number): string {
  const whole = Math.trunc(microUsd / MONEY_SCALE);
  const fraction = String(Math.abs(microUsd % MONEY_SCALE)).padStart(6, "0");
  const decimals = fraction.replace(/0+$/, "").padEnd(2, "0");
  return `$${whole}.${decimals}`;
}

export function PricingTableSettings({
  snapshot,
}: {
  snapshot: PricingTableDto | null;
}) {
  const queryClient = useQueryClient();
  const [syncing, setSyncing] = useState(false);
  // The backend only reports `syncing` once it owns the run; the click keeps
  // the section in that state until the snapshot arrives.
  const busy = syncing || snapshot?.status === "syncing";
  const presentation = pricingPresentation(snapshot, busy);
  const rows = snapshot?.rows ?? [];

  const openSource = async () => {
    try {
      await openPricingSource();
    } catch {
      // The external browser is optional; keep Settings usable on failure.
    }
  };

  const synchronize = async () => {
    if (busy) return;
    setSyncing(true);
    try {
      const next = await syncPricingFromWeb();
      queryClient.setQueryData(queryKeys.pricingTable, next);
    } catch {
      // The command reports its outcome in the snapshot; a transport failure
      // leaves the previous table and the previous status in place.
    } finally {
      setSyncing(false);
    }
  };

  return (
    <SettingsSection
      title="模型价格"
      status={
        <SettingsStatus tone={presentation.sectionTone}>
          {presentation.sectionLabel}
        </SettingsStatus>
      }
    >
      <SettingsActionGroup>
        <SettingsButton
          variant="primary"
          type="button"
          disabled={busy}
          onClick={() => void synchronize()}
        >
          {busy ? (
            <LoaderCircle aria-hidden="true" className="spin" size={15} />
          ) : (
            <RefreshCw aria-hidden="true" size={15} />
          )}
          同步官网
        </SettingsButton>
      </SettingsActionGroup>
      <div className="pricing-status-line">
        <SettingsStatus tone={presentation.actionTone} aria-live="polite">
          {presentation.actionText}
        </SettingsStatus>
      </div>
      <p className="pricing-caption">
        美元 / 每 100 万 tokens。输入 ≤27.2 万 tokens 按「短」计价，超出整单按「长」。
        <br />
        数据来源：
        <button
          className="pricing-source-link"
          type="button"
          aria-label="打开 OpenAI 官方定价页"
          onClick={() => void openSource()}
        >
          developers.openai.com
        </button>
        （仅 GPT 模型随官网同步）
      </p>
      <div className="pricing-table-viewport">
        <table className="pricing-table" aria-label="模型价格表">
          <thead>
            <tr>
              <th scope="col">模型</th>
              <th scope="col">上下文</th>
              <th scope="col" className="num">
                输入
              </th>
              <th scope="col" className="num">
                缓存读
              </th>
              <th scope="col" className="num">
                缓存写
              </th>
              <th scope="col" className="num">
                输出
              </th>
              <th scope="col">来源</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={`${row.modelId}-${row.band}`}>
                <td>{row.modelId}</td>
                <td>{row.band === "long" ? "长" : "短"}</td>
                <td className="num">{formatMoney(row.inputMicroUsd)}</td>
                <td className="num">{formatMoney(row.cachedInputMicroUsd)}</td>
                <td className="num">
                  {row.cacheWriteMicroUsd === null
                    ? "—"
                    : formatMoney(row.cacheWriteMicroUsd)}
                </td>
                <td className="num">{formatMoney(row.outputMicroUsd)}</td>
                <td className="source">
                  {row.source === "official" ? "官网" : "内置"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </SettingsSection>
  );
}
