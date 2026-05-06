import { useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X, Plug } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useChain } from "@/hooks/useChain";

export default function ConnectDialog() {
  const { connect, connecting, error } = useChain();
  const [chainWs, setChainWs] = useState("ws://127.0.0.1:2222");
  const [open, setOpen] = useState(false);

  const handleConnect = async () => {
    await connect(chainWs);
    setOpen(false);
  };

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Trigger asChild>
        <Button data-testid="connect-button" size="sm" variant="outline">
          <Plug className="h-4 w-4" />
        </Button>
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/50 data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0" />
        <Dialog.Content data-testid="connect-dialog" className="fixed left-[50%] top-[50%] max-h-[85vh] w-[90vw] max-w-[450px] translate-x-[-50%] translate-y-[-50%] rounded-lg border bg-card p-6 shadow-lg focus:outline-none data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[state=closed]:slide-out-to-left-1/2 data-[state=closed]:slide-out-to-top-[48%] data-[state=open]:slide-in-from-left-1/2 data-[state=open]:slide-in-from-top-[48%]">
          <Dialog.Title className="text-lg font-semibold">
            Connect to Network
          </Dialog.Title>
          <Dialog.Description className="mt-2 text-sm text-muted-foreground">
            Enter the WebSocket endpoint for the parachain. Provider endpoints
            are resolved automatically from on-chain data.
          </Dialog.Description>

          <div className="mt-4 space-y-4">
            <div className="space-y-2">
              <label className="text-sm font-medium">Chain WebSocket</label>
              <Input
                data-testid="connect-ws-input"
                value={chainWs}
                onChange={(e) => setChainWs(e.target.value)}
                placeholder="ws://127.0.0.1:2222"
              />
            </div>

            {error && (
              <p className="text-sm text-destructive">{error}</p>
            )}

            <div className="flex justify-end gap-3">
              <Dialog.Close asChild>
                <Button data-testid="connect-cancel" variant="outline">Cancel</Button>
              </Dialog.Close>
              <Button data-testid="connect-submit" onClick={handleConnect} disabled={connecting}>
                {connecting ? "Connecting..." : "Connect"}
              </Button>
            </div>
          </div>

          <Dialog.Close asChild>
            <button
              className="absolute right-4 top-4 rounded-sm opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2"
              aria-label="Close"
            >
              <X className="h-4 w-4" />
            </button>
          </Dialog.Close>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
