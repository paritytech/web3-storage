import { useState } from "react";
import {
  Users,
  Plus,
  Key,
  Copy,
  Eye,
  EyeOff,
  Trash2,
  Check,
  Download,
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

interface Account {
  name: string;
  address: string;
  publicKey: string;
  isActive: boolean;
}

export default function Accounts() {
  const [accounts, setAccounts] = useState<Account[]>([
    {
      name: "Alice",
      address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
      publicKey: "0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d",
      isActive: true,
    },
  ]);
  const [showPrivateKey, setShowPrivateKey] = useState<string | null>(null);
  const [newAccountName, setNewAccountName] = useState("");

  const copyToClipboard = (text: string, label: string) => {
    navigator.clipboard.writeText(text);
    toast({ title: "Copied", description: `${label} copied to clipboard` });
  };

  const handleCreateAccount = () => {
    if (!newAccountName.trim()) {
      toast({
        title: "Error",
        description: "Account name is required",
        variant: "destructive",
      });
      return;
    }

    // TODO: Generate real keypair using polkadot-api
    const mockAddress = `5${Array.from({ length: 47 }, () =>
      "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz123456789"[
        Math.floor(Math.random() * 58)
      ]
    ).join("")}`;
    const mockPublicKey = `0x${Array.from({ length: 64 }, () =>
      Math.floor(Math.random() * 16).toString(16)
    ).join("")}`;

    const newAccount: Account = {
      name: newAccountName,
      address: mockAddress,
      publicKey: mockPublicKey,
      isActive: false,
    };

    setAccounts([...accounts, newAccount]);
    setNewAccountName("");
    toast({ title: "Success", description: `Account "${newAccountName}" created` });
  };

  const handleDeleteAccount = (address: string) => {
    setAccounts(accounts.filter((a) => a.address !== address));
    toast({ title: "Success", description: "Account deleted" });
  };

  const handleSetActive = (address: string) => {
    setAccounts(
      accounts.map((a) => ({
        ...a,
        isActive: a.address === address,
      }))
    );
    toast({ title: "Success", description: "Active account changed" });
  };

  const activeAccount = accounts.find((a) => a.isActive);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Accounts</h1>
        <p className="text-muted-foreground">
          Manage your signing accounts
        </p>
      </div>

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

              <div className="rounded-lg border p-4 space-y-3">
                <div>
                  <p className="text-sm text-muted-foreground mb-1">Address</p>
                  <div className="flex items-center gap-2">
                    <code className="flex-1 font-mono text-sm bg-muted px-2 py-1 rounded">
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

                <div>
                  <p className="text-sm text-muted-foreground mb-1">
                    Public Key
                  </p>
                  <div className="flex items-center gap-2">
                    <code className="flex-1 font-mono text-sm bg-muted px-2 py-1 rounded overflow-hidden text-ellipsis">
                      {activeAccount.publicKey}
                    </code>
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() =>
                        copyToClipboard(activeAccount.publicKey, "Public Key")
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

      {/* Create Account */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Plus className="h-5 w-5" />
            Create New Account
          </CardTitle>
          <CardDescription>
            Generate a new keypair for signing transactions
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex gap-3">
            <Input
              placeholder="Account name"
              value={newAccountName}
              onChange={(e) => setNewAccountName(e.target.value)}
              className="max-w-sm"
            />
            <Button onClick={handleCreateAccount}>
              <Key className="mr-2 h-4 w-4" />
              Generate
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* All Accounts */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Users className="h-5 w-5" />
            All Accounts ({accounts.length})
          </CardTitle>
        </CardHeader>
        <CardContent>
          {accounts.length === 0 ? (
            <div className="text-center py-8 text-muted-foreground">
              <Key className="mx-auto h-12 w-12 mb-4 opacity-50" />
              <p>No accounts yet</p>
              <p className="text-sm">Create an account to get started</p>
            </div>
          ) : (
            <div className="space-y-3">
              {accounts.map((account) => (
                <div
                  key={account.address}
                  className={`flex items-center justify-between rounded-lg border p-4 ${
                    account.isActive ? "border-primary bg-primary/5" : ""
                  }`}
                >
                  <div className="flex items-center gap-4">
                    <div
                      className={`h-10 w-10 rounded-full flex items-center justify-center ${
                        account.isActive
                          ? "bg-primary text-primary-foreground"
                          : "bg-muted"
                      }`}
                    >
                      {account.name[0].toUpperCase()}
                    </div>
                    <div>
                      <div className="flex items-center gap-2">
                        <p className="font-medium">{account.name}</p>
                        {account.isActive && (
                          <span className="text-xs bg-primary text-primary-foreground px-2 py-0.5 rounded">
                            Active
                          </span>
                        )}
                      </div>
                      <p className="font-mono text-sm text-muted-foreground">
                        {truncateHash(account.address, 10, 6)}
                      </p>
                    </div>
                  </div>

                  <div className="flex items-center gap-2">
                    {!account.isActive && (
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => handleSetActive(account.address)}
                      >
                        <Check className="mr-2 h-4 w-4" />
                        Set Active
                      </Button>
                    )}
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() =>
                        copyToClipboard(account.address, "Address")
                      }
                    >
                      <Copy className="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() =>
                        setShowPrivateKey(
                          showPrivateKey === account.address
                            ? null
                            : account.address
                        )
                      }
                    >
                      {showPrivateKey === account.address ? (
                        <EyeOff className="h-4 w-4" />
                      ) : (
                        <Eye className="h-4 w-4" />
                      )}
                    </Button>
                    {accounts.length > 1 && (
                      <Button
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
          )}
        </CardContent>
      </Card>

      {/* Export/Import */}
      <Card>
        <CardHeader>
          <CardTitle>Backup</CardTitle>
          <CardDescription>
            Export your accounts for backup or import existing accounts
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex gap-3">
            <Button variant="outline">
              <Download className="mr-2 h-4 w-4" />
              Export Accounts
            </Button>
            <Button variant="outline">
              <Plus className="mr-2 h-4 w-4" />
              Import Account
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
