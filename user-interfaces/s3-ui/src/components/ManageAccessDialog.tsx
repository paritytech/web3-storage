// SPDX-License-Identifier: GPL-3.0-only

import { useState, useEffect, useCallback } from "react";
import { RefreshCw, Trash2, Plus } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { fetchMembers, addMember, removeMember, useSignerAddress } from "@/state";
import type { BucketMember, MemberRole } from "@/lib/s3-client";
import { truncateHash } from "@/lib/utils";
import { toast } from "@/components/ui/toaster";
import { isValidSs58 } from "@/lib/crypto";

interface ManageAccessDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  bucketId: bigint;
  bucketName: string;
}

function isSameAddress(a: string, b: string): boolean {
  try {
    return a === b;
  } catch {
    return false;
  }
}

const ROLES: MemberRole[] = ["Admin", "Writer", "Reader"];

function roleBadge(role: MemberRole) {
  const colors: Record<MemberRole, string> = {
    Admin: "bg-purple-100 text-purple-700",
    Writer: "bg-blue-100 text-blue-700",
    Reader: "bg-slate-100 text-slate-700",
  };
  return (
    <span className={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${colors[role]}`}>
      {role}
    </span>
  );
}

export default function ManageAccessDialog({
  open,
  onOpenChange,
  bucketId,
  bucketName,
}: ManageAccessDialogProps) {
  const signerAddress = useSignerAddress();
  const [members, setMembers] = useState<BucketMember[]>([]);
  const [loading, setLoading] = useState(false);
  const [newAddress, setNewAddress] = useState("");
  const [newRole, setNewRole] = useState<MemberRole>("Writer");
  const [adding, setAdding] = useState(false);

  const loadMembers = useCallback(async () => {
    setLoading(true);
    try {
      const list = await fetchMembers(bucketId);
      setMembers(list);
    } catch (err) {
      toast({
        title: "Failed to load members",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    } finally {
      setLoading(false);
    }
  }, [bucketId]);

  useEffect(() => {
    if (open) loadMembers();
  }, [open, loadMembers]);

  const isAdmin = members.some(
    (m) => signerAddress && isSameAddress(m.account, signerAddress) && m.role === "Admin",
  );

  const handleAdd = async () => {
    if (!newAddress.trim()) return;
    if (!isValidSs58(newAddress.trim())) {
      toast({ title: "Invalid SS58 address", variant: "destructive" });
      return;
    }
    setAdding(true);
    try {
      await addMember(bucketId, newAddress.trim(), newRole);
      toast({ title: "Member added" });
      setNewAddress("");
      await loadMembers();
    } catch (err) {
      toast({
        title: "Failed to add member",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    } finally {
      setAdding(false);
    }
  };

  const handleRemove = async (account: string) => {
    try {
      await removeMember(bucketId, account);
      toast({ title: "Member removed" });
      await loadMembers();
    } catch (err) {
      toast({
        title: "Failed to remove member",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg" data-testid="manage-access-dialog">
        <DialogHeader>
          <DialogTitle>Manage Access — {bucketName}</DialogTitle>
          <DialogDescription>
            Add or remove bucket members and their roles.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <p className="text-sm text-muted-foreground">
              {members.length} member{members.length !== 1 && "s"}
            </p>
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8"
              onClick={loadMembers}
              disabled={loading}
            >
              <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
            </Button>
          </div>

          <div className="rounded-lg border">
            <table className="w-full">
              <thead>
                <tr className="border-b bg-muted/50">
                  <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Account</th>
                  <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground w-24">Role</th>
                  {isAdmin && <th className="px-3 py-2 w-12" />}
                </tr>
              </thead>
              <tbody>
                {members.map((m) => (
                  <tr key={m.account} className="border-b last:border-b-0">
                    <td className="px-3 py-2 text-sm font-mono">
                      {truncateHash(m.account, 8, 6)}
                    </td>
                    <td className="px-3 py-2">{roleBadge(m.role)}</td>
                    {isAdmin && (
                      <td className="px-3 py-2">
                        {!(signerAddress && isSameAddress(m.account, signerAddress)) && (
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-7 w-7 text-destructive hover:text-destructive"
                            onClick={() => handleRemove(m.account)}
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </Button>
                        )}
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {isAdmin && (
            <div className="flex gap-2 items-end">
              <div className="flex-1">
                <label className="text-xs font-medium">Address</label>
                <Input
                  data-testid="add-member-address"
                  value={newAddress}
                  onChange={(e) => setNewAddress(e.target.value)}
                  placeholder="5..."
                  className="font-mono text-xs"
                />
              </div>
              <div className="w-28">
                <label className="text-xs font-medium">Role</label>
                <select
                  value={newRole}
                  onChange={(e) => setNewRole(e.target.value as MemberRole)}
                  className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  {ROLES.map((r) => (
                    <option key={r} value={r}>{r}</option>
                  ))}
                </select>
              </div>
              <Button size="sm" onClick={handleAdd} disabled={adding}>
                <Plus className="mr-1 h-3.5 w-3.5" />
                Add
              </Button>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
