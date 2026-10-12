import { LoaderCircle, RefreshCw } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { openPricingSource, syncPricingFromWeb } from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type { PricingTableDto } from "../../generated";
import { useLanguage } from "../language/useLanguage";
import { pricingPresentation } from "./pricingPresentation";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsSection,
  SettingsStatus,
} from "./SettingsPrimitives";

const MONEY_SCALE = 1_000_000;
const PRICING_SOURCE_DOMAIN = "developers.openai.com";

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
  const { messages } = useLanguage();
  const pricingMessages = messages.imagesPricing.pricing;
  const [syncing, setSyncing] = useState(false);
  // The backend only reports `syncing` once it owns the run; the click keeps
  // the section in that state until the snapshot arrives.
  const busy = syncing || snapshot?.status === "syncing";
  const presentation = pricingPresentation(snapshot, pricingMessages, busy);
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
      title={pricingMessages.sectionTitle}
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
          {pricingMessages.syncButton}
        </SettingsButton>
      </SettingsActionGroup>
      <div className="pricing-status-line">
        <SettingsStatus tone={presentation.actionTone} aria-live="polite">
          {presentation.actionText}
        </SettingsStatus>
      </div>
      <p className="pricing-caption">
        {pricingMessages.captionBilling}
        <br />
        {pricingMessages.captionSourcePrefix}
        <button
          className="pricing-source-link"
          type="button"
          aria-label={pricingMessages.sourceLinkLabel}
          onClick={() => void openSource()}
        >
          {PRICING_SOURCE_DOMAIN}
        </button>
        {pricingMessages.captionNote}
      </p>
      <div className="pricing-table-viewport">
        <table className="pricing-table" aria-label={pricingMessages.tableLabel}>
          <thead>
            <tr>
              <th scope="col">{pricingMessages.columnModel}</th>
              <th scope="col">{pricingMessages.columnContext}</th>
              <th scope="col" className="num">
                {pricingMessages.columnInput}
              </th>
              <th scope="col" className="num">
                {pricingMessages.columnCachedRead}
              </th>
              <th scope="col" className="num">
                {pricingMessages.columnCachedWrite}
              </th>
              <th scope="col" className="num">
                {pricingMessages.columnOutput}
              </th>
              <th scope="col">{pricingMessages.columnSource}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={`${row.modelId}-${row.band}`}>
                <td>{row.modelId}</td>
                <td>
                  {row.band === "long"
                    ? pricingMessages.bandLong
                    : pricingMessages.bandShort}
                </td>
                <td className="num">{formatMoney(row.inputMicroUsd)}</td>
                <td className="num">{formatMoney(row.cachedInputMicroUsd)}</td>
                <td className="num">
                  {row.cacheWriteMicroUsd === null
                    ? "—"
                    : formatMoney(row.cacheWriteMicroUsd)}
                </td>
                <td className="num">{formatMoney(row.outputMicroUsd)}</td>
                <td className="source">
                  {row.source === "official"
                    ? pricingMessages.sourceOfficial
                    : pricingMessages.sourceBundled}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </SettingsSection>
  );
}
