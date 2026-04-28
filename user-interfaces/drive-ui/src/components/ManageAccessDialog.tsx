import { useState, useEffect, useCallback } from "react";
import { Users, Plus, Trash2, RefreshCw } from "lucide-react";
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
import { toast } from "@/components/ui/toaster";
import { truncateHash } from "@/lib/utils";
import type { BucketMember, MemberRole } from "@/lib/drive-client";

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
  const { signerAddress, fetchMembers, addMember, removeMember } = useDrive();

  const [members, setMembers] = useState<BucketMember[]>([]);
  const [loading, setLoading] = useState(false);
  const [newAccount, setNewAccount] = useState("");
  const [newRole, setNewRole] = useState<MemberRole>("Writer");
  const [adding, setAdding] = useState(false);

  const isAdmin = members.some(
    (m) => m.account === signerAddress && m.role === "Admin"
  );

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const m = await fetchMembers(bucketId);
      setMembers(m);
    } catch {
      setMembers([]);
    } finally {
      setLoading(false);
    }
  }, [bucketId, fetchMembers]);

  useEffect(() => {
    if (open) refresh();
  }, [open, refresh]);

  const handleAdd = async () => {
    if (!newAccount.trim()) return;
    setAdding(true);
    try {
      await addMember(bucketId, newAccount.trim(), newRole);
      setNewAccount("");
      toast({
        title: "Member added",
        description: `${truncateHash(newAccount)} as ${newRole}`,
      });
      refresh();
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
      toast({ title: "Member removed", description: truncateHash(account) });
      refresh();
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
      <DialogContent className="sm:max-w-md">
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
          {/* Member list */}
          <div>
            <div className="flex items-center justify-between mb-2">
              <span className="text-sm font-medium">Members</span>
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                onClick={refresh}
                disabled={loading}
              >
                <RefreshCw
                  className={`h-3 w-3 ${loading ? "animate-spin" : ""}`}
                />
              </Button>
            </div>

            {members.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                {loading ? "Loading..." : "No members found"}
              </p>
            ) : (
              <table className="w-full text-xs">
                <thead>
                  <tr className="border-b">
                    <th className="text-left py-1 font-medium">Account</th>
                    <th className="text-left py-1 font-medium w-20">Role</th>
                    {isAdmin && (
                      <th className="text-right py-1 font-medium w-16" />
                    )}
                  </tr>
                </thead>
                <tbody>
                  {members.map((m) => (
                    <tr key={m.account} className="border-b last:border-0">
                      <td className="py-1.5 font-mono text-muted-foreground">
                        {truncateHash(m.account)}
                      </td>
                      <td className="py-1.5">{roleBadge(m.role)}</td>
                      {isAdmin && (
                        <td className="py-1.5 text-right">
                          {m.account !== signerAddress && (
                            <Button
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

          {/* Add member form (admin only) */}
          {isAdmin && (
            <div className="space-y-2">
              <span className="text-sm font-medium">Add Member</span>
              <div className="flex items-center gap-2">
                <Input
                  placeholder="Account address"
                  value={newAccount}
                  onChange={(e) => setNewAccount(e.target.value)}
                  className="flex-1 text-xs h-8"
                />
                <select
                  className="rounded-md border border-input bg-background px-2 py-1 text-xs h-8"
                  value={newRole}
                  onChange={(e) =>
                    setNewRole(e.target.value as MemberRole)
                  }
                >
                  <option value="Admin">Admin</option>
                  <option value="Writer">Writer</option>
                  <option value="Reader">Reader</option>
                </select>
                <Button
                  size="sm"
                  className="h-8 text-xs"
                  onClick={handleAdd}
                  disabled={adding || !newAccount.trim()}
                >
                  <Plus className="h-3 w-3 mr-1" />
                  {adding ? "Adding..." : "Add"}
                </Button>
              </div>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
