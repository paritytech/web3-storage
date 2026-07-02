// SPDX-License-Identifier: GPL-3.0-only

import { useState } from "react";
import { Lock, LockOpen, Copy, RefreshCw, AlertTriangle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useEncryptionKey, setEncryptionKey, clearEncryptionKey } from "@/state";
import { EncryptionKey, bytesToHex } from "@/lib/encryption";
import { toast } from "@/components/ui/toaster";

export default function EncryptionPanel() {
  const encryptionKey = useEncryptionKey();
  const [keyHex, setKeyHex] = useState("");

  const handleGenerate = async () => {
    const { rawKey } = await EncryptionKey.generate();
    const hex = bytesToHex(rawKey);
    setKeyHex(hex);
    setEncryptionKey(hex);
    toast({ title: "Encryption key generated", description: "Save this key — it cannot be recovered." });
  };

  const handleEnable = () => {
    if (!keyHex || keyHex.length !== 64) {
      toast({
        title: "Invalid key",
        description: "Encryption key must be 64 hex characters (32 bytes).",
        variant: "destructive",
      });
      return;
    }
    try {
      setEncryptionKey(keyHex);
      toast({ title: "Encryption enabled" });
    } catch (err) {
      toast({
        title: "Invalid key",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };

  const handleDisable = () => {
    clearEncryptionKey();
    setKeyHex("");
    toast({ title: "Encryption disabled" });
  };

  const handleCopy = () => {
    if (keyHex) {
      navigator.clipboard.writeText(keyHex).then(
        () => toast({ title: "Key copied to clipboard" }),
        () => toast({ title: "Copy failed", variant: "destructive" }),
      );
    }
  };

  return (
    <Card data-testid="encryption-panel">
      <CardHeader className="pb-3">
        <CardTitle className="text-sm flex items-center gap-2">
          {encryptionKey ? (
            <Lock className="h-4 w-4 text-amber-500" />
          ) : (
            <LockOpen className="h-4 w-4 text-muted-foreground" />
          )}
          Client-Side Encryption (AES-256-GCM)
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        {encryptionKey ? (
          <div className="space-y-2">
            <div className="flex items-center gap-2 p-2 rounded-md bg-amber-50 border border-amber-200 text-amber-800">
              <Lock className="h-4 w-4 flex-shrink-0" />
              <span className="text-xs">Encryption is active. Uploads will be encrypted.</span>
            </div>
            {keyHex && (
              <div className="flex items-center gap-2">
                <code className="flex-1 text-xs bg-muted p-2 rounded font-mono break-all">
                  {keyHex}
                </code>
                <Button variant="ghost" size="icon" className="h-8 w-8" onClick={handleCopy}>
                  <Copy className="h-3.5 w-3.5" />
                </Button>
              </div>
            )}
            <Button variant="outline" size="sm" onClick={handleDisable}>
              <LockOpen className="mr-2 h-3.5 w-3.5" />
              Disable Encryption
            </Button>
          </div>
        ) : (
          <div className="space-y-3">
            <div className="flex items-start gap-2 p-2 rounded-md bg-muted text-xs text-muted-foreground">
              <AlertTriangle className="h-4 w-4 flex-shrink-0 mt-0.5" />
              <span>
                Save your encryption key — it cannot be recovered. Without it,
                encrypted objects are permanently unreadable.
              </span>
            </div>
            <div className="flex gap-2">
              <Input
                data-testid="encryption-key-input"
                value={keyHex}
                onChange={(e) => setKeyHex(e.target.value)}
                placeholder="64 hex characters (32 bytes)"
                className="font-mono text-xs"
              />
              <Button variant="outline" size="sm" onClick={handleCopy} disabled={!keyHex}>
                <Copy className="h-3.5 w-3.5" />
              </Button>
            </div>
            <div className="flex gap-2">
              <Button size="sm" onClick={handleGenerate}>
                <RefreshCw className="mr-2 h-3.5 w-3.5" />
                Generate Key
              </Button>
              <Button variant="outline" size="sm" onClick={handleEnable} disabled={!keyHex}>
                <Lock className="mr-2 h-3.5 w-3.5" />
                Enable
              </Button>
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
