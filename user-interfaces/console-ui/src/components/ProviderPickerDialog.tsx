import { useState, useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";
import { RefreshCw, X, Server } from "lucide-react";
import { useStorage } from "@/hooks/useStorage";
import { formatBytes, truncateHash, formatTokens } from "@/lib/utils";
import type { AvailableProvider } from "@/lib/storage";

interface ProviderPickerDialogProps {
  open: boolean;
  onClose: () => void;
  onSelect: (providerAccount: string) => void;
  requiredCapacity: bigint;
  requiredDuration: number;
}

export default function ProviderPickerDialog({
  open,
  onClose,
  onSelect,
  requiredCapacity,
  requiredDuration,
}: ProviderPickerDialogProps) {
  const { listAvailableProviders } = useStorage();
  const [providers, setProviders] = useState<AvailableProvider[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      loadProviders();
    }
  }, [open]);

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

  if (!open) return null;

  const getDisabledReason = (p: AvailableProvider): string | null => {
    if (!p.acceptingPrimary) return "Not accepting";
    if (p.availableCapacity < requiredCapacity) return "Capacity full";
    if (requiredDuration < p.minDuration) return "Duration too short";
    if (p.maxDuration > 0 && requiredDuration > p.maxDuration) return "Duration too long";
    return null;
  };

  return (
    <Card className="border-2">
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <div>
            <CardTitle className="text-base">Choose a Provider</CardTitle>
            <CardDescription>
              Automatic provider selection failed. Select a provider manually to
              retry.
            </CardDescription>
          </div>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-foreground"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </CardHeader>
      <CardContent>
        {loading ? (
          <div className="flex items-center justify-center py-8 text-muted-foreground">
            <RefreshCw className="h-5 w-5 animate-spin mr-2" />
            Loading providers...
          </div>
        ) : error ? (
          <div className="py-6 text-center">
            <p className="text-sm text-red-500 mb-2">{error}</p>
            <Button variant="outline" size="sm" onClick={loadProviders}>
              Retry
            </Button>
          </div>
        ) : providers.length === 0 ? (
          <div className="py-8 text-center text-muted-foreground">
            <Server className="mx-auto h-8 w-8 mb-2 opacity-50" />
            <p className="text-sm">No providers registered on chain</p>
          </div>
        ) : (
          <div className="space-y-3">
            <div className="rounded-lg border overflow-hidden">
              <table className="w-full">
                <thead>
                  <tr className="border-b bg-muted/50">
                    <th className="px-3 py-2 text-left text-xs font-medium">
                      Account
                    </th>
                    <th className="px-3 py-2 text-left text-xs font-medium">
                      Available
                    </th>
                    <th className="px-3 py-2 text-left text-xs font-medium">
                      Price/byte
                    </th>
                    <th className="px-3 py-2 text-left text-xs font-medium">
                      Duration
                    </th>
                    <th className="px-3 py-2 text-left text-xs font-medium">
                      Agreements
                    </th>
                    <th className="px-3 py-2 text-right text-xs font-medium w-24">
                      Action
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {providers.map((p) => {
                    const reason = getDisabledReason(p);
                    const disabled = reason !== null;
                    const utilization =
                      p.maxCapacity > 0n
                        ? Number(
                            ((p.maxCapacity - p.availableCapacity) * 100n) /
                              p.maxCapacity,
                          )
                        : 0;
                    return (
                      <tr
                        key={p.account}
                        className={`border-b last:border-b-0 ${disabled ? "opacity-50" : "hover:bg-muted/30"}`}
                      >
                        <td className="px-3 py-2">
                          <span className="font-mono text-xs">
                            {truncateHash(p.account, 8, 6)}
                          </span>
                        </td>
                        <td className="px-3 py-2">
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
                        </td>
                        <td className="px-3 py-2 text-xs">
                          {formatTokens(p.pricePerByte)}
                        </td>
                        <td className="px-3 py-2 text-xs">
                          {p.minDuration}–{p.maxDuration || "\u221E"}
                        </td>
                        <td className="px-3 py-2 text-xs">
                          {p.agreementsTotal}
                        </td>
                        <td className="px-3 py-2 text-right">
                          {disabled ? (
                            <span className="text-xs text-muted-foreground">
                              {reason}
                            </span>
                          ) : (
                            <Button
                              variant="outline"
                              size="sm"
                              className="h-7 text-xs"
                              onClick={() => onSelect(p.account)}
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
            <div className="flex justify-between items-center">
              <Button variant="ghost" size="sm" onClick={loadProviders}>
                <RefreshCw className="h-3.5 w-3.5 mr-1" />
                Refresh
              </Button>
              <Button variant="ghost" size="sm" onClick={onClose}>
                Cancel
              </Button>
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
