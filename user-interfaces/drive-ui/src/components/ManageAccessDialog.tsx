// SPDX-License-Identifier: GPL-3.0-only

import { useState, useEffect, useCallback } from "react";
import { Users, Plus, Trash2, RefreshCw, Globe, Lock } from "lucide-react";
import { isValidSs58 } from "@/lib/crypto";
import { isSameAddress } from "@web3-storage/sdk";
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
  useSignerAddress,
  fetchMembers,
  addMember,
  removeMember,
  fetchVisibility,
  setBucketVisibility,
} from "@/state";
import { toast } from "@/components/ui/toaster";
import { truncateHash } from "@/lib/utils";
import type { BucketMember, MemberRole, Visibility } from "@/lib/drive-client";

interface ManageAccessDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  bucketId: bigint;
  driveName: string;
}

function roleBadge(role: string) {
  const colors: Record<string, string> = {
    Admin: "bg-red-100 text-red-800",
    Writer: "bg-blue-100 text-blue-800",
    Reader: "bg-gray-100 text-gray-800",
  };
  return (
    <span
      className={`inline-flex items-center rounded px-2 py-0.5 text-xs font-medium ${colors[role] ?? colors.Reader}`}
    >
      {role}
    </span>
  );
}

export default function ManageAccessDialog({
  open,
  onOpenChange,
  bucketId,
  driveName,
}: ManageAccessDialogProps) {
  const signerAddress = useSignerAddress();

  const [members, setMembers] = useState<BucketMember[]>([]);
  const [loading, setLoading] = useState(false);
  const [newAccount, setNewAccount] = useState("");
  const [newRole, setNewRole] = useState<MemberRole>("Writer");
  const [adding, setAdding] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [visibility, setVisibility] = useState<Visibility | null>(null);
  const [togglingVisibility, setTogglingVisibility] = useState(false);

  // Compare by raw bytes: member accounts come from chain (runtime prefix)
  // while signerAddress / typed input may use a different SS58 prefix.
  const isMe = (account: string) =>
    signerAddress != null && isSameAddress(account, signerAddress);
  const isMember = (account: string) =>
    members.some((m) => isSameAddress(m.account, account));

  const isAdmin = members.some((m) => isMe(m.account) && m.role === "Admin");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [m, vis] = await Promise.all([fetchMembers(bucketId), fetchVisibility(bucketId)]);
      setMembers(m);
      setVisibility(vis);
    } catch {
      setMembers([]);
      setVisibility(null);
    } finally {
      setLoading(false);
    }
  }, [bucketId]);

  useEffect(() => {
    if (open) refresh();
  }, [open, refresh]);

  // Validate as the user types
  useEffect(() => {
    if (!newAccount.trim()) {
      setValidationError(null);
      return;
    }
    if (!isValidSs58(newAccount.trim())) {
      setValidationError("Invalid SS58 address");
      return;
    }
    if (isMember(newAccount.trim())) {
      setValidationError("This account is already a member");
      return;
    }
    setValidationError(null);
  }, [newAccount, members]);

  const handleAdd = async () => {
    const trimmed = newAccount.trim();
    if (!trimmed) return;
    if (!isValidSs58(trimmed)) {
      setValidationError("Invalid SS58 address");
      return;
    }
    if (isMember(trimmed)) {
      setValidationError("This account is already a member");
      return;
    }
    setAdding(true);
    try {
      await addMember(bucketId, trimmed, newRole);
      setNewAccount("");
      toast({
        title: "Member added",
        description: `${truncateHash(trimmed)} as ${newRole}`,
      });
      await refresh();
    } catch (err) {
      console.error("[ManageAccessDialog] addMember failed:", err);
      toast({
        title: "Failed to add member",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    } finally {
      setAdding(false);
    }
  };

  const handleToggleVisibility = async () => {
    if (!visibility) return;
    const next: Visibility = visibility === "Private" ? "Public" : "Private";
    setTogglingVisibility(true);
    try {
      await setBucketVisibility(bucketId, next);
      setVisibility(next);
      toast({ title: `Drive is now ${next.toLowerCase()}` });
    } catch (err) {
      toast({
        title: "Failed to change visibility",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    } finally {
      setTogglingVisibility(false);
    }
  };

  const handleRemove = async (account: string) => {
    try {
      await removeMember(bucketId, account);
      toast({ title: "Member removed", description: truncateHash(account) });
      await refresh();
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
      <DialogContent className="sm:max-w-md" data-testid="manage-access-dialog">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Users className="h-4 w-4" />
            Manage Access
          </DialogTitle>
          <DialogDescription>
            Members of "{driveName}" (Bucket #{bucketId.toString()})
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div
            className="flex items-center justify-between rounded-md border px-3 py-2"
            data-testid="bucket-visibility"
          >
            <div className="flex items-center gap-2">
              {visibility === "Public" ? (
                <Globe className="h-4 w-4 text-muted-foreground" />
              ) : (
                <Lock className="h-4 w-4 text-muted-foreground" />
              )}
              <div>
                <p className="text-sm font-medium">{visibility ?? "…"}</p>
                <p className="text-xs text-muted-foreground">
                  Private: providers serve reads only to members.
                </p>
              </div>
            </div>
            {isAdmin && visibility && (
              <Button
                data-testid="toggle-visibility"
                variant="outline"
                size="sm"
                className="h-8 text-xs"
                onClick={handleToggleVisibility}
                disabled={togglingVisibility}
              >
                Make {visibility === "Private" ? "Public" : "Private"}
              </Button>
            )}
          </div>

          <div>
            <div className="flex items-center justify-between mb-2">
              <span className="text-sm font-medium">Members</span>
              <Button
                data-testid="members-refresh"
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                onClick={refresh}
                disabled={loading}
              >
                <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
              </Button>
            </div>

            {members.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                {loading ? "Loading..." : "No members found"}
              </p>
            ) : (
              <table className="w-full text-xs" data-testid="members-table">
                <thead>
                  <tr className="border-b">
                    <th className="text-left py-1 font-medium">Account</th>
                    <th className="text-left py-1 font-medium w-20">Role</th>
                    {isAdmin && <th className="text-right py-1 font-medium w-16" />}
                  </tr>
                </thead>
                <tbody>
                  {members.map((m) => (
                    <tr key={m.account} className="border-b last:border-0" data-testid={`member-row-${m.account}`}>
                      <td className="py-1.5 font-mono text-muted-foreground">
                        {truncateHash(m.account)}
                      </td>
                      <td className="py-1.5">{roleBadge(m.role)}</td>
                      {isAdmin && (
                        <td className="py-1.5 text-right">
                          {!isMe(m.account) && (
                            <Button
                              data-testid={`member-remove-${m.account}`}
                              variant="ghost"
                              size="icon"
                              className="h-6 w-6 text-muted-foreground hover:text-destructive"
                              onClick={() => handleRemove(m.account)}
                            >
                              <Trash2 className="h-3 w-3" />
                            </Button>
                          )}
                        </td>
                      )}
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          {isAdmin && (
            <div className="space-y-2">
              <span className="text-sm font-medium">Add Member</span>
              <div className="flex items-center gap-2">
                <Input
                  data-testid="add-member-address"
                  placeholder="Account address (SS58)"
                  value={newAccount}
                  onChange={(e) => setNewAccount(e.target.value)}
                  className="flex-1 text-xs h-8"
                />
                <select
                  data-testid="add-member-role"
                  className="rounded-md border border-input bg-background px-2 py-1 text-xs h-8"
                  value={newRole}
                  onChange={(e) => setNewRole(e.target.value as MemberRole)}
                >
                  <option value="Admin">Admin</option>
                  <option value="Writer">Writer</option>
                  <option value="Reader">Reader</option>
                </select>
                <Button
                  data-testid="add-member-submit"
                  size="sm"
                  className="h-8 text-xs"
                  onClick={handleAdd}
                  disabled={adding || !newAccount.trim() || validationError !== null}
                >
                  <Plus className="h-3 w-3 mr-1" />
                  {adding ? "Adding..." : "Add"}
                </Button>
              </div>
              {validationError && (
                <p data-testid="add-member-error" className="text-xs text-destructive">
                  {validationError}
                </p>
              )}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
