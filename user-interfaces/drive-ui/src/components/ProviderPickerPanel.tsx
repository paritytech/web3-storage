import { useState, useEffect } from "react";
import { RefreshCw, Server } from "lucide-react";
import { Button } from "@/components/ui/button";
import { listAvailableProviders } from "@/state";
import type { AvailableProvider } from "@/lib/drive-client";
import { formatBytes, truncateHash, formatTokens } from "@/lib/utils";

interface ProviderPickerPanelProps {
  onSelect: (provider: AvailableProvider) => void;
  requiredCapacity: bigint;
  requiredDuration: number;
  /** Disable all Select buttons (e.g. while a submission is in flight). */
  disabled?: boolean;
}

/**
 * Inline panel that lists registered storage providers. Designed to be
 * embedded inside the Create New Drive dialog so the user picks a
 * provider as part of the same form — no separate modal.
 *
 * Mirrors console-ui's ProviderPickerDialog table — same `data-testid`s
 * (`provider-picker`, `provider-picker-select`) so shared e2e patterns
 * address both UIs.
 */
export default function ProviderPickerPanel({
  onSelect,
  requiredCapacity,
  requiredDuration,
  disabled = false,
}: ProviderPickerPanelProps) {
  const [providers, setProviders] = useState<AvailableProvider[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    loadProviders();
  }, []);

  const loadProviders = async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await listAvailableProviders();
      setProviders(list);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load providers");
    } finally {
      setLoading(false);
    }
  };

  const getDisabledReason = (p: AvailableProvider): string | null => {
    if (!p.acceptingPrimary) return "Not accepting";
    // `maxCapacity === 0n` means unlimited — skip the capacity check.
    if (p.maxCapacity !== 0n && p.availableCapacity < requiredCapacity)
      return "Capacity full";
    if (requiredDuration < p.minDuration) return "Duration too short";
    if (p.maxDuration > 0 && requiredDuration > p.maxDuration)
      return "Duration too long";
    return null;
  };

  return (
    <div className="space-y-2" data-testid="provider-picker">
      <div className="flex items-center justify-between">
        <label className="text-sm font-medium">Choose a Provider</label>
        <Button
          variant="ghost"
          size="sm"
          onClick={loadProviders}
          disabled={loading}
        >
          <RefreshCw className={`h-3.5 w-3.5 mr-1 ${loading ? "animate-spin" : ""}`} />
          Refresh
        </Button>
      </div>

      {loading ? (
        <div className="flex items-center justify-center py-6 text-muted-foreground">
          <RefreshCw className="h-5 w-5 animate-spin mr-2" />
          Loading providers...
        </div>
      ) : error ? (
        <div className="py-4 text-center">
          <p className="text-sm text-red-500 mb-2">{error}</p>
          <Button variant="outline" size="sm" onClick={loadProviders}>
            Retry
          </Button>
        </div>
      ) : providers.length === 0 ? (
        <div className="py-6 text-center text-muted-foreground">
          <Server className="mx-auto h-8 w-8 mb-2 opacity-50" />
          <p className="text-sm">No providers registered on chain</p>
        </div>
      ) : (
        <div className="rounded-lg border overflow-hidden">
          <table className="w-full">
            <thead>
              <tr className="border-b bg-muted/50">
                <th className="px-3 py-2 text-left text-xs font-medium">Account</th>
                <th className="px-3 py-2 text-left text-xs font-medium">Available</th>
                <th className="px-3 py-2 text-left text-xs font-medium">Price/byte</th>
                <th className="px-3 py-2 text-left text-xs font-medium">Duration</th>
                <th className="px-3 py-2 text-right text-xs font-medium w-24">Action</th>
              </tr>
            </thead>
            <tbody>
              {providers.map((p) => {
                const reason = getDisabledReason(p);
                const rowDisabled = reason !== null;
                // `max_capacity == 0` is the substrate convention for
                // "unlimited" (see runtime ProviderSettings docs).
                const unlimited = p.maxCapacity === 0n;
                const utilization = unlimited
                  ? 0
                  : Number(
                      ((p.maxCapacity - p.availableCapacity) * 100n) / p.maxCapacity,
                    );
                return (
                  <tr
                    key={p.account}
                    className={`border-b last:border-b-0 ${rowDisabled ? "opacity-50" : "hover:bg-muted/30"}`}
                  >
                    <td className="px-3 py-2">
                      <span className="font-mono text-xs">
                        {truncateHash(p.account, 8, 6)}
                      </span>
                    </td>
                    <td className="px-3 py-2">
                      {unlimited ? (
                        <span className="text-xs text-muted-foreground">Unlimited</span>
                      ) : (
                        <div className="space-y-1">
                          <span className="text-xs">
                            {formatBytes(Number(p.availableCapacity))}
                          </span>
                          <div className="h-1 w-16 rounded-full bg-secondary">
                            <div
                              className="h-full rounded-full bg-primary transition-all"
                              style={{ width: `${utilization}%` }}
                            />
                          </div>
                        </div>
                      )}
                    </td>
                    <td className="px-3 py-2 text-xs">
                      {formatTokens(p.pricePerByte)}
                    </td>
                    <td className="px-3 py-2 text-xs">
                      {p.minDuration}–{p.maxDuration || "∞"}
                    </td>
                    <td className="px-3 py-2 text-right">
                      {rowDisabled ? (
                        <span className="text-xs text-muted-foreground">{reason}</span>
                      ) : (
                        <Button
                          data-testid="provider-picker-select"
                          variant="outline"
                          size="sm"
                          className="h-7 text-xs"
                          onClick={() => onSelect(p)}
                          disabled={disabled}
                        >
                          Select
                        </Button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
