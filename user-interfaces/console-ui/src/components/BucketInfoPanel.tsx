import { useState, useEffect, useCallback } from "react";
import {
  Server,
  Users,
  ChevronDown,
  ChevronRight,
  RefreshCw,
  Plus,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent } from "@/components/ui/card";
import { useStorage } from "@/hooks/useStorage";
import { toast } from "@/components/ui/toaster";
import { truncateHash } from "@/lib/utils";
import { isSameAddress } from "@web3-storage/sdk";
import type { BucketMember, ProviderEndpointInfo } from "@/lib/storage";

interface BucketInfoPanelProps {
  bucketId: bigint;
}

export default function BucketInfoPanel({ bucketId }: BucketInfoPanelProps) {
  const {
    signerAddress,
    fetchBucketMembers,
    addBucketMember,
    removeBucketMember,
    fetchBucketProviders,
  } = useStorage();

  const [expanded, setExpanded] = useState(false);
  const [members, setMembers] = useState<BucketMember[]>([]);
  const [providers, setProviders] = useState<ProviderEndpointInfo[]>([]);
  const [loadingMembers, setLoadingMembers] = useState(false);
  const [loadingProviders, setLoadingProviders] = useState(false);

  // Add member form
  const [newMemberAccount, setNewMemberAccount] = useState("");
  const [newMemberRole, setNewMemberRole] = useState<'Admin' | 'Writer' | 'Reader'>('Writer');
  const [adding, setAdding] = useState(false);

  // Compare by raw bytes: member accounts come from chain (runtime prefix)
  // while signerAddress is locally encoded, so string equality can fail across
  // SS58 prefixes.
  const isMe = (account: string) =>
    signerAddress != null && isSameAddress(account, signerAddress);

  const isAdmin = members.some((m) => isMe(m.account) && m.role === 'Admin');

  const refreshMembers = useCallback(async () => {
    setLoadingMembers(true);
    try {
      const m = await fetchBucketMembers(bucketId);
      setMembers(m);
    } catch {
      setMembers([]);
    } finally {
      setLoadingMembers(false);
    }
  }, [bucketId, fetchBucketMembers]);

  const refreshProviders = useCallback(async () => {
    setLoadingProviders(true);
    try {
      const p = await fetchBucketProviders(bucketId);
      setProviders(p);
    } catch {
      setProviders([]);
    } finally {
      setLoadingProviders(false);
    }
  }, [bucketId, fetchBucketProviders]);

  useEffect(() => {
    if (expanded) {
      refreshMembers();
      refreshProviders();
    }
  }, [expanded, bucketId, refreshMembers, refreshProviders]);

  const handleAddMember = async () => {
    const trimmed = newMemberAccount.trim();
    if (!trimmed) return;
    // The runtime's set_member updates an existing member's role rather
    // than rejecting; surface the duplicate at the UI layer so users get
    // an obvious "already a member" message instead of a silent role swap.
    if (members.some((m) => isSameAddress(m.account, trimmed))) {
      toast({
        title: "Failed to add member",
        description: `${truncateHash(trimmed)} is already a member`,
        variant: "destructive",
      });
      return;
    }
    setAdding(true);
    try {
      await addBucketMember(bucketId, trimmed, newMemberRole);
      setNewMemberAccount("");
      toast({ title: "Member added", description: `${truncateHash(newMemberAccount)} as ${newMemberRole}` });
      // Optimistic insert: tx resolved at best-block, but chainHead can
      // lag a few hundred ms behind. Don't call refreshMembers here — it
      // races chainHead and would clobber this insert with the pre-tx
      // list. Reconciliation happens on the next expand or via the
      // manual refresh button. Mirrors useStorage.createBucket.
      setMembers((prev) =>
        prev.some((m) => isSameAddress(m.account, trimmed))
          ? prev
          : [...prev, { account: trimmed, role: newMemberRole }],
      );
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

  const handleRemoveMember = async (account: string) => {
    try {
      await removeBucketMember(bucketId, account);
      toast({ title: "Member removed", description: truncateHash(account) });
      refreshMembers();
    } catch (err) {
      toast({
        title: "Failed to remove member",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };

  const roleBadge = (role: string) => {
    const colors: Record<string, string> = {
      Admin: "bg-red-100 text-red-800 dark:bg-red-900 dark:text-red-200",
      Writer: "bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200",
      Reader: "bg-gray-100 text-gray-800 dark:bg-gray-900 dark:text-gray-200",
    };
    return (
      <span className={`inline-flex items-center rounded px-2 py-0.5 text-xs font-medium ${colors[role] ?? colors.Reader}`}>
        {role}
      </span>
    );
  };

  return (
    <Card data-testid="bucket-info-panel">
      <button
        data-testid="bucket-info-toggle"
        className="w-full flex items-center gap-2 p-4 text-left"
        onClick={() => setExpanded(!expanded)}
      >
        {expanded ? (
          <ChevronDown className="h-4 w-4" />
        ) : (
          <ChevronRight className="h-4 w-4" />
        )}
        <Server className="h-4 w-4" />
        <span className="font-medium text-sm">Bucket Info</span>
        <span className="text-xs text-muted-foreground ml-auto">
          Bucket #{bucketId.toString()}
        </span>
      </button>

      {expanded && (
        <CardContent className="pt-0 space-y-4">
          {/* Providers Section */}
          <div>
            <div className="flex items-center gap-2 mb-2">
              <Server className="h-3.5 w-3.5 text-muted-foreground" />
              <span className="text-sm font-medium">Providers</span>
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6 ml-auto"
                onClick={refreshProviders}
                disabled={loadingProviders}
              >
                <RefreshCw className={`h-3 w-3 ${loadingProviders ? "animate-spin" : ""}`} />
              </Button>
            </div>
            {providers.length === 0 ? (
              <p className="text-xs text-muted-foreground">No providers found</p>
            ) : (
              <div className="space-y-1">
                {providers.map((p) => (
                  <div
                    key={p.account}
                    className="flex items-center gap-2 rounded-md border px-3 py-1.5 text-xs"
                  >
                    <span
                      className={`h-2 w-2 rounded-full ${p.healthy ? "bg-green-500" : "bg-red-500"}`}
                      title={p.healthy ? "Healthy" : "Unreachable"}
                    />
                    <span className="font-mono text-muted-foreground">
                      {truncateHash(p.account)}
                    </span>
                    <span className="font-mono ml-auto">{p.endpoint}</span>
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* Members Section */}
          <div>
            <div className="flex items-center gap-2 mb-2">
              <Users className="h-3.5 w-3.5 text-muted-foreground" />
              <span className="text-sm font-medium">Members</span>
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6 ml-auto"
                onClick={refreshMembers}
                disabled={loadingMembers}
              >
                <RefreshCw className={`h-3 w-3 ${loadingMembers ? "animate-spin" : ""}`} />
              </Button>
            </div>
            {members.length === 0 ? (
              <p className="text-xs text-muted-foreground">No members found</p>
            ) : (
              <table data-testid="bucket-members-table" className="w-full text-xs">
                <thead>
                  <tr className="border-b">
                    <th className="text-left py-1 font-medium">Account</th>
                    <th className="text-left py-1 font-medium w-20">Role</th>
                    {isAdmin && <th className="text-right py-1 font-medium w-16">Actions</th>}
                  </tr>
                </thead>
                <tbody>
                  {members.map((m) => (
                    <tr key={m.account} data-testid={`bucket-member-row-${m.account}`} className="border-b last:border-0">
                      <td className="py-1.5 font-mono text-muted-foreground">
                        {truncateHash(m.account)}
                      </td>
                      <td className="py-1.5">{roleBadge(m.role)}</td>
                      {isAdmin && (
                        <td className="py-1.5 text-right">
                          {!isMe(m.account) && (
                            <Button
                              data-testid={`bucket-member-remove-${m.account}`}
                              variant="ghost"
                              size="icon"
                              className="h-6 w-6 text-muted-foreground hover:text-destructive"
                              onClick={() => handleRemoveMember(m.account)}
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

            {/* Add member form (admin only) */}
            {isAdmin && (
              <div className="flex items-center gap-2 mt-2">
                <Input
                  data-testid="bucket-add-member-address"
                  placeholder="Account address"
                  value={newMemberAccount}
                  onChange={(e) => setNewMemberAccount(e.target.value)}
                  className="flex-1 text-xs h-8"
                />
                <select
                  data-testid="bucket-add-member-role"
                  className="rounded-md border border-input bg-background px-2 py-1 text-xs h-8"
                  value={newMemberRole}
                  onChange={(e) => setNewMemberRole(e.target.value as 'Admin' | 'Writer' | 'Reader')}
                >
                  <option value="Admin">Admin</option>
                  <option value="Writer">Writer</option>
                  <option value="Reader">Reader</option>
                </select>
                <Button
                  data-testid="bucket-add-member-submit"
                  size="sm"
                  className="h-8 text-xs"
                  onClick={handleAddMember}
                  disabled={adding || !newMemberAccount.trim()}
                >
                  <Plus className="h-3 w-3 mr-1" />
                  {adding ? "Adding..." : "Add"}
                </Button>
              </div>
            )}
          </div>
        </CardContent>
      )}
    </Card>
  );
}
