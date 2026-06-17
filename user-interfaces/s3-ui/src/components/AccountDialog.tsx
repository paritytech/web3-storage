// SPDX-License-Identifier: GPL-3.0-only

import { useState } from "react";
import { User, Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { setSigner, selectDevAccount, useIsSettingSigner, DEV_ACCOUNTS } from "@/state";

interface AccountDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export default function AccountDialog({ open, onOpenChange }: AccountDialogProps) {
  const loading = useIsSettingSigner();
  const [customSeed, setCustomSeed] = useState("");

  const handleSelectDev = async (name: string) => {
    try {
      await selectDevAccount(name);
      onOpenChange(false);
    } catch {
      /* swallow; loading$ resets in finally */
    }
  };

  const handleCustom = async () => {
    if (!customSeed) return;
    try {
      await setSigner(customSeed);
      onOpenChange(false);
    } catch {
      /* swallow */
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md" data-testid="account-dialog">
        <DialogHeader>
          <DialogTitle>Select Account</DialogTitle>
          <DialogDescription>
            Choose a development account or enter a custom seed.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="grid grid-cols-2 gap-3">
            {DEV_ACCOUNTS.map((account) => (
              <button
                key={account.name}
                data-testid={`account-dialog-${account.name.toLowerCase()}`}
                onClick={() => handleSelectDev(account.name)}
                disabled={loading}
                className={`flex items-center gap-3 rounded-lg border p-3 transition-colors hover:bg-accent ${account.color}`}
              >
                <div className="flex h-9 w-9 items-center justify-center rounded-full bg-white/60 font-semibold text-sm">
                  {account.name[0]}
                </div>
                <span className="font-medium text-sm">{account.name}</span>
              </button>
            ))}
          </div>

          <div className="relative">
            <div className="absolute inset-0 flex items-center">
              <span className="w-full border-t" />
            </div>
            <div className="relative flex justify-center text-xs uppercase">
              <span className="bg-background px-2 text-muted-foreground">
                or custom seed
              </span>
            </div>
          </div>

          <div className="flex gap-2">
            <Input
              data-testid="account-custom-seed"
              value={customSeed}
              onChange={(e) => setCustomSeed(e.target.value)}
              placeholder="//CustomAccount or mnemonic"
              className="flex-1"
            />
            <Button
              data-testid="account-custom-submit"
              onClick={handleCustom}
              disabled={loading || !customSeed}
              size="sm"
            >
              {loading ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <User className="h-4 w-4" />
              )}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
