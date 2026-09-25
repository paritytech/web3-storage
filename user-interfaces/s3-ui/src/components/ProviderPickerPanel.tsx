// SPDX-License-Identifier: GPL-3.0-only

import { useState, useEffect } from "react";
import { RefreshCw, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { queryMatchingProviders } from "@/state";
import type { MatchingProviders, AvailableProvider } from "@/lib/s3-client";
import { formatBytes, truncateHash } from "@web3-storage/format";

interface ProviderPickerPanelProps {
  onSelect: (provider: AvailableProvider) => void;
  requiredCapacity: bigint;
  requiredDuration: number;
  requiredPricePerByte: bigint;
  disabled?: boolean;
}

export default function ProviderPickerPanel({
  onSelect,
  requiredCapacity,
  requiredDuration,
  requiredPricePerByte,
  disabled,
}: ProviderPickerPanelProps) {
  const [providers, setProviders] = useState<MatchingProviders[]>([]);
  const [loading, setLoading] = useState(false);

  const load = async () => {
    setLoading(true);
    try {
      const results = await queryMatchingProviders(
        {
          bytesNeeded: requiredCapacity,
          minDuration: requiredDuration,
          maxPricePerByte: requiredPricePerByte,
          primaryOnly: true,
        },
        10,
      );
      setProviders(results);
    } catch {
      setProviders([]);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load();
  }, []);

  if (loading) {
    return (
      <div className="flex items-center justify-center py-8">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        <span className="ml-2 text-sm text-muted-foreground">Querying providers...</span>
      </div>
    );
  }

  return (
    <div className="space-y-3" data-testid="provider-picker">
      <div className="flex items-center justify-between">
        <p className="text-sm font-medium">
          {providers.length} provider{providers.length !== 1 && "s"} found
        </p>
        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={load}>
          <RefreshCw className="h-4 w-4" />
        </Button>
      </div>

      {providers.length === 0 ? (
        <p className="text-sm text-muted-foreground py-4 text-center">
          No matching providers found. Try adjusting your requirements.
        </p>
      ) : (
        <div className="rounded-lg border overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b bg-muted/50">
                <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Provider</th>
                <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground w-16">Score</th>
                <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground w-36">Available</th>
                <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground w-24">Price/byte</th>
                <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground w-24">Duration</th>
                <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground w-28">Reputation</th>
                <th className="px-3 py-2 w-20" />
              </tr>
            </thead>
            <tbody>
              {providers.map((p) => {
                // `undefined` is unlimited — nothing to meter.
                const free = p.availableCapacity;
                // Percent free, like the drive-ui picker.
                const availablePct =
                  free !== undefined && p.maxCapacity > 0n
                    ? Number((free * 100n) / p.maxCapacity)
                    : 0;
                const isPartial = p.matchScore < 100;

                return (
                  <tr
                    key={p.account}
                    className={`border-b last:border-b-0 ${isPartial ? "opacity-60" : ""}`}
                  >
                    <td className="px-3 py-2 font-mono text-xs">
                      {truncateHash(p.account, 6, 4)}
                    </td>
                    <td className="px-3 py-2">
                      <span className={`font-medium ${p.matchScore >= 80 ? "text-emerald-600" : p.matchScore >= 50 ? "text-amber-600" : "text-red-600"}`}>
                        {p.matchScore}
                      </span>
                    </td>
                    <td className="px-3 py-2">
                      {free === undefined ? (
                        <p className="text-xs text-muted-foreground">Unlimited</p>
                      ) : (
                        <div className="space-y-1">
                          <Progress value={availablePct} className="h-1.5" />
                          <p className="text-xs text-muted-foreground">
                            {formatBytes(Number(free))} / {formatBytes(Number(p.maxCapacity))}
                          </p>
                        </div>
                      )}
                    </td>
                    <td className="px-3 py-2 text-xs">{p.pricePerByte.toString()}</td>
                    <td className="px-3 py-2 text-xs">
                      {p.minDuration}–{p.maxDuration}
                    </td>
                    <td className="px-3 py-2 text-xs text-muted-foreground">
                      {/* The chain's own score, not a UI-side formula. */}
                      <span
                        className={`font-medium ${p.reputation >= 90 ? "text-emerald-600" : p.reputation >= 50 ? "text-amber-600" : "text-red-600"}`}
                      >
                        {p.reputation}
                      </span>
                      <span className="ml-1">
                        · {p.agreementsTotal} agmt{p.agreementsTotal !== 1 && "s"}
                      </span>
                      {p.challengesFailed > 0 && (
                        <span className="text-red-500 ml-1">
                          ({p.challengesFailed} fail)
                        </span>
                      )}
                    </td>
                    <td className="px-3 py-2">
                      <Button
                        data-testid={`select-provider-${p.account}`}
                        size="sm"
                        variant="outline"
                        className="h-7 text-xs"
                        onClick={() => onSelect(p)}
                        disabled={disabled}
                      >
                        Select
                      </Button>
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
