// SPDX-License-Identifier: Apache-2.0

import { useState, useEffect } from "react";
import { Plug, Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  useEndpoint,
  useIsConnecting,
  useConnectionError,
  useNetworkList,
  useSelectedNetworkId,
  selectNetwork,
  selectCustomNetwork,
} from "@/state";

interface ConnectDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export default function ConnectDialog({ open, onOpenChange }: ConnectDialogProps) {
  const currentEndpoint = useEndpoint();
  const connecting = useIsConnecting();
  const error = useConnectionError();
  const networks = useNetworkList();
  const selectedId = useSelectedNetworkId();
  const [endpoint, setEndpoint] = useState(currentEndpoint);

  useEffect(() => {
    setEndpoint(currentEndpoint);
  }, [currentEndpoint]);

  const handleConnect = async () => {
    try {
      if (endpoint && endpoint !== currentEndpoint) {
        await selectCustomNetwork({ parachainWs: endpoint, providerHttp: "" });
      } else {
        await selectNetwork(selectedId);
      }
      onOpenChange(false);
    } catch {
      /* error surfaced via useConnectionError */
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md" data-testid="connect-dialog">
        <DialogHeader>
          <DialogTitle>Connect to Chain</DialogTitle>
          <DialogDescription>
            Choose a network or enter a custom WebSocket endpoint.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <label className="text-sm font-medium">Network</label>
            <div className="grid grid-cols-2 gap-2">
              {networks.map((n) => (
                <button
                  key={n.id}
                  data-testid={`network-${n.id}`}
                  onClick={() => {
                    setEndpoint(n.parachainWs);
                  }}
                  className={`rounded-md border px-2 py-1.5 text-xs text-left transition-colors ${
                    endpoint === n.parachainWs
                      ? "border-primary bg-primary/10"
                      : "border-input hover:bg-accent"
                  }`}
                >
                  <span className="font-medium">{n.name}</span>
                  <span className="block truncate text-muted-foreground">{n.parachainWs}</span>
                </button>
              ))}
            </div>
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium">WebSocket Endpoint</label>
            <Input
              data-testid="connect-endpoint-input"
              value={endpoint}
              onChange={(e) => setEndpoint(e.target.value)}
              placeholder="ws://127.0.0.1:2222"
              onKeyDown={(e) => e.key === "Enter" && handleConnect()}
            />
          </div>
          {error && <p className="text-sm text-destructive">{error}</p>}
          <Button
            data-testid="connect-submit"
            onClick={handleConnect}
            disabled={connecting || !endpoint}
            className="w-full"
          >
            {connecting ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                Connecting...
              </>
            ) : (
              <>
                <Plug className="mr-2 h-4 w-4" />
                Connect
              </>
            )}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
