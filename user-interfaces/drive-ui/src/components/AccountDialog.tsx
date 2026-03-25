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
import { useDrive } from "@/hooks/useDrive";

interface AccountDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const DEV_ACCOUNTS = [
  { name: "Alice", seed: "//Alice", color: "bg-indigo-100 text-indigo-700 border-indigo-200" },
  { name: "Bob", seed: "//Bob", color: "bg-emerald-100 text-emerald-700 border-emerald-200" },
  { name: "Charlie", seed: "//Charlie", color: "bg-amber-100 text-amber-700 border-amber-200" },
  { name: "Dave", seed: "//Dave", color: "bg-rose-100 text-rose-700 border-rose-200" },
];

export default function AccountDialog({ open, onOpenChange }: AccountDialogProps) {
  const { setSigner } = useDrive();
  const [loading, setLoading] = useState(false);
  const [customSeed, setCustomSeed] = useState("");

  const handleSelect = async (seed: string, name?: string) => {
    setLoading(true);
    try {
      await setSigner(seed, name);
      onOpenChange(false);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
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
                onClick={() => handleSelect(account.seed, account.name)}
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
              value={customSeed}
              onChange={(e) => setCustomSeed(e.target.value)}
              placeholder="//CustomAccount or mnemonic"
              className="flex-1"
            />
            <Button
              onClick={() => handleSelect(customSeed)}
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
