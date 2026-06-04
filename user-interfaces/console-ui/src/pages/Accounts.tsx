import { useState } from "react";
import {
  Users,
  Plus,
  Key,
  Copy,
  Trash2,
  Check,
  Loader2,
} from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { toast } from "@/components/ui/toaster";
import { truncateHash } from "@/lib/utils";
import { useChain } from "@/hooks/useChain";
import { useStorage } from "@/hooks/useStorage";
import { seedToKeypair, toSs58 } from "@/lib/crypto";
import { isSameAddress } from "@web3-storage/papi";

interface Account {
  name: string;
  seed: string;
  address: string;
}

// Pre-configured dev accounts. Addresses are derived from seeds so they match
// the chain's SS58 prefix (see lib/crypto.ts) rather than being hardcoded.
const DEV_ACCOUNTS: Account[] = [
  { name: "Alice", seed: "//Alice" },
  { name: "Bob", seed: "//Bob" },
  { name: "Charlie", seed: "//Charlie" },
].map(({ name, seed }) => ({
  name,
  seed,
  address: toSs58(seedToKeypair(seed).publicKey),
}));


export default function Accounts() {
  const { connected } = useChain();
  const { setSigner, signerAddress, loading } = useStorage();

  const [accounts, setAccounts] = useState<Account[]>(DEV_ACCOUNTS);
  const [customSeed, setCustomSeed] = useState("");
  const [customName, setCustomName] = useState("");
  const [settingAccount, setSettingAccount] = useState<string | null>(null);

  // Compare by raw bytes: dev-account addresses are encoded at module load
  // (default prefix) while `signerAddress` follows the connected runtime, so
  // string equality can spuriously fail across prefixes.
  const isActive = (account: Account) =>
    signerAddress != null && isSameAddress(account.address, signerAddress);

  const isDevAccount = (address: string) =>
    DEV_ACCOUNTS.some((d) => isSameAddress(d.address, address));

  const copyToClipboard = (text: string, label: string) => {
    navigator.clipboard.writeText(text);
    toast({ title: "Copied", description: `${label} copied to clipboard` });
  };

  const handleSetActive = async (account: Account) => {
    if (!connected) {
      toast({
        title: "Error",
        description: "Connect to the network first",
        variant: "destructive",
      });
      return;
    }

    setSettingAccount(account.address);
    try {
      await setSigner(account.seed);
      toast({ title: "Success", description: `Active account set to ${account.name}` });
    } catch (err) {
      toast({
        title: "Error",
        description: err instanceof Error ? err.message : "Failed to set account",
        variant: "destructive",
      });
    } finally {
      setSettingAccount(null);
    }
  };

  const handleAddCustomAccount = async () => {
    if (!customSeed.trim()) {
      toast({
        title: "Error",
        description: "Please enter a seed phrase",
        variant: "destructive",
      });
      return;
    }

    // Check if already exists
    if (accounts.some((a) => a.seed === customSeed)) {
      toast({
        title: "Error",
        description: "This account already exists",
        variant: "destructive",
      });
      return;
    }

    try {
      const { seedToKeypair, toSs58 } = await import("@/lib/crypto");
      const keypair = seedToKeypair(customSeed);

      const newAccount: Account = {
        name: customName || `Account ${accounts.length + 1}`,
        seed: customSeed,
        address: toSs58(keypair.publicKey),
      };

      setAccounts([...accounts, newAccount]);
      setCustomSeed("");
      setCustomName("");
      toast({ title: "Success", description: `Account "${newAccount.name}" added` });
    } catch (err) {
      toast({
        title: "Error",
        description: err instanceof Error ? err.message : "Invalid seed phrase",
        variant: "destructive",
      });
    }
  };

  const handleDeleteAccount = (address: string) => {
    // Don't allow deleting dev accounts
    if (isDevAccount(address)) {
      toast({
        title: "Error",
        description: "Cannot delete dev accounts",
        variant: "destructive",
      });
      return;
    }

    setAccounts(accounts.filter((a) => a.address !== address));
    toast({ title: "Success", description: "Account removed" });
  };

  const activeAccount = accounts.find(isActive);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Accounts</h1>
        <p className="text-muted-foreground">
          Manage your signing accounts
        </p>
      </div>

      {!connected && (
        <Card className="border-yellow-500">
          <CardContent className="py-4">
            <p className="text-sm text-yellow-500">
              Connect to the network to activate accounts for signing transactions.
            </p>
          </CardContent>
        </Card>
      )}

      {/* Active Account */}
      {activeAccount && (
        <Card className="border-primary">
          <CardHeader>
            <div className="flex items-center gap-2">
              <div className="h-2 w-2 rounded-full bg-green-500" />
              <CardTitle>Active Account</CardTitle>
            </div>
            <CardDescription>
              This account is used for signing transactions
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <p className="font-semibold text-lg">{activeAccount.name}</p>
                  <p className="font-mono text-sm text-muted-foreground">
                    {truncateHash(activeAccount.address, 12, 8)}
                  </p>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() =>
                    copyToClipboard(activeAccount.address, "Address")
                  }
                >
                  <Copy className="h-4 w-4" />
                </Button>
              </div>

              <div className="rounded-lg border p-4">
                <div>
                  <p className="text-sm text-muted-foreground mb-1">Address</p>
                  <div className="flex items-center gap-2">
                    <code className="flex-1 font-mono text-sm bg-muted px-2 py-1 rounded overflow-hidden text-ellipsis">
                      {activeAccount.address}
                    </code>
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() =>
                        copyToClipboard(activeAccount.address, "Address")
                      }
                    >
                      <Copy className="h-4 w-4" />
                    </Button>
                  </div>
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Add Custom Account */}
      <Card data-testid="accounts-custom-form">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Plus className="h-5 w-5" />
            Add Custom Account
          </CardTitle>
          <CardDescription>
            Add a custom account using a seed phrase or derivation path
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium">Name</label>
              <Input
                data-testid="accounts-custom-name-input"
                placeholder="My Account"
                value={customName}
                onChange={(e) => setCustomName(e.target.value)}
              />
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Seed / Derivation Path</label>
              <Input
                data-testid="accounts-custom-seed-input"
                placeholder="//MyAccount or seed phrase"
                value={customSeed}
                onChange={(e) => setCustomSeed(e.target.value)}
                type="password"
              />
            </div>
          </div>
          <Button data-testid="accounts-custom-submit" onClick={handleAddCustomAccount}>
            <Key className="mr-2 h-4 w-4" />
            Add Account
          </Button>
        </CardContent>
      </Card>

      {/* All Accounts */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Users className="h-5 w-5" />
            Available Accounts ({accounts.length})
          </CardTitle>
          <CardDescription>
            Click "Set Active" to use an account for signing
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div data-testid="accounts-list" className="space-y-3">
            {accounts.map((account) => (
              <div
                key={account.address}
                data-testid={`accounts-list-row-${account.name}`}
                className={`flex items-center justify-between rounded-lg border p-4 ${
                  isActive(account) ? "border-primary bg-primary/5" : ""
                }`}
              >
                <div className="flex items-center gap-4">
                  <div
                    className={`h-10 w-10 rounded-full flex items-center justify-center ${
                      isActive(account)
                        ? "bg-primary text-primary-foreground"
                        : "bg-muted"
                    }`}
                  >
                    {account.name[0].toUpperCase()}
                  </div>
                  <div>
                    <div className="flex items-center gap-2">
                      <p className="font-medium">{account.name}</p>
                      {isActive(account) && (
                        <span data-testid={`accounts-active-badge-${account.name}`} className="text-xs bg-primary text-primary-foreground px-2 py-0.5 rounded">
                          Active
                        </span>
                      )}
                      {isDevAccount(account.address) && (
                        <span className="text-xs bg-muted text-muted-foreground px-2 py-0.5 rounded">
                          Dev
                        </span>
                      )}
                    </div>
                    <p className="font-mono text-sm text-muted-foreground">
                      {truncateHash(account.address, 10, 6)}
                    </p>
                  </div>
                </div>

                <div className="flex items-center gap-2">
                  {!isActive(account) && connected && (
                    <Button
                      data-testid={`accounts-set-active-${account.name}`}
                      variant="outline"
                      size="sm"
                      onClick={() => handleSetActive(account)}
                      disabled={loading || settingAccount === account.address}
                    >
                      {settingAccount === account.address ? (
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      ) : (
                        <Check className="mr-2 h-4 w-4" />
                      )}
                      Set Active
                    </Button>
                  )}
                  <Button
                    data-testid={`accounts-copy-${account.name}`}
                    variant="ghost"
                    size="icon"
                    onClick={() =>
                      copyToClipboard(account.address, "Address")
                    }
                  >
                    <Copy className="h-4 w-4" />
                  </Button>
                  {!isDevAccount(account.address) && (
                    <Button
                      data-testid={`accounts-delete-${account.name}`}
                      variant="ghost"
                      size="icon"
                      onClick={() => handleDeleteAccount(account.address)}
                    >
                      <Trash2 className="h-4 w-4 text-muted-foreground hover:text-destructive" />
                    </Button>
                  )}
                </div>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
