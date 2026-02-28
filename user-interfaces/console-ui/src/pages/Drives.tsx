import { useState, useEffect } from "react";
import {
  HardDrive,
  Plus,
  Folder,
  File,
  RefreshCw,
  Trash2,
  ChevronRight,
  AlertCircle,
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
import { useChain } from "@/hooks/useChain";
import { useStorage } from "@/hooks/useStorage";
import { toast } from "@/components/ui/toaster";
import { formatBytes } from "@/lib/utils";
import type { DriveInfo } from "@/lib/storage";

// Local drive state with additional UI fields
interface LocalDrive extends DriveInfo {
  fileCount?: number;
  totalSize?: number;
}

export default function Drives() {
  const { connected } = useChain();
  const {
    signerAddress,
    drives: chainDrives,
    loading,
    createDrive,
    refreshDrives,
    deleteDrive: sdkDeleteDrive,
  } = useStorage();

  // Local state for drives (combines chain data with local state)
  const [drives, setDrives] = useState<LocalDrive[]>([]);
  const [newDriveName, setNewDriveName] = useState("");
  const [capacity, setCapacity] = useState("1000000000"); // 1 GB default
  const [duration, setDuration] = useState("500");
  const [maxPayment, setMaxPayment] = useState("1000000000000000"); // 1000 tokens
  const [creating, setCreating] = useState(false);
  const [selectedDrive, setSelectedDrive] = useState<LocalDrive | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);

  // Sync chain drives to local state
  useEffect(() => {
    if (chainDrives.length > 0) {
      setDrives(chainDrives.map(d => ({
        ...d,
        fileCount: 0,
        totalSize: 0,
      })));
    }
  }, [chainDrives]);

  // Refresh drives when signer is set
  useEffect(() => {
    if (signerAddress && connected) {
      refreshDrives();
    }
  }, [signerAddress, connected, refreshDrives]);

  const handleCreateDrive = async () => {
    if (!signerAddress) {
      toast({
        title: "Error",
        description: "Please set a signer in the Accounts page first",
        variant: "destructive",
      });
      return;
    }

    setCreating(true);
    try {
      const driveId = await createDrive({
        name: newDriveName || undefined,
        capacity: BigInt(capacity),
        duration: parseInt(duration, 10),
        maxPayment: BigInt(maxPayment),
      });

      // Add to local state immediately
      const newDrive: LocalDrive = {
        driveId,
        owner: signerAddress,
        name: newDriveName || null,
        bucketId: driveId, // Simulated - would be from chain
        rootCid: null,
        createdAt: BigInt(Date.now()),
        updatedAt: BigInt(Date.now()),
        fileCount: 0,
        totalSize: 0,
      };
      setDrives([...drives, newDrive]);
      setNewDriveName("");
      toast({ title: "Success", description: `Drive "${newDriveName || driveId}" created` });
    } catch (err) {
      toast({
        title: "Error",
        description: err instanceof Error ? err.message : "Failed to create drive",
        variant: "destructive",
      });
    } finally {
      setCreating(false);
    }
  };

  const handleDeleteDrive = async (drive: LocalDrive) => {
    try {
      await sdkDeleteDrive(drive.driveId);
      setDrives(drives.filter((d) => d.driveId !== drive.driveId));
      if (selectedDrive?.driveId === drive.driveId) {
        setSelectedDrive(null);
      }
      toast({ title: "Success", description: `Drive "${drive.name || drive.driveId}" deleted` });
    } catch (err) {
      toast({
        title: "Error",
        description: err instanceof Error ? err.message : "Failed to delete drive",
        variant: "destructive",
      });
    }
  };

  if (!connected) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Drives</h1>
          <p className="text-muted-foreground">Manage your File System drives</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <HardDrive className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p className="text-muted-foreground">
              Connect to the network to manage drives
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (!signerAddress) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Drives</h1>
          <p className="text-muted-foreground">Manage your File System drives</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <AlertCircle className="mx-auto h-12 w-12 mb-4 text-yellow-500" />
            <p className="text-muted-foreground mb-2">
              No signer set
            </p>
            <p className="text-sm text-muted-foreground">
              Go to the Accounts page to set a signing account first
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Drives</h1>
          <p className="text-muted-foreground">Manage your File System drives</p>
        </div>
        <Button variant="outline" size="sm" onClick={refreshDrives} disabled={loading}>
          <RefreshCw className={`mr-2 h-4 w-4 ${loading ? "animate-spin" : ""}`} />
          Refresh
        </Button>
      </div>

      {/* Create Drive */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Plus className="h-5 w-5" />
            Create New Drive
          </CardTitle>
          <CardDescription>
            Create a new file system drive for organizing your files
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex gap-3">
            <Input
              placeholder="Drive name (optional)"
              value={newDriveName}
              onChange={(e) => setNewDriveName(e.target.value)}
              className="max-w-sm"
            />
            <Button onClick={handleCreateDrive} disabled={creating || loading}>
              {creating ? "Creating..." : "Create Drive"}
            </Button>
          </div>

          <Button
            variant="ghost"
            size="sm"
            onClick={() => setShowAdvanced(!showAdvanced)}
          >
            {showAdvanced ? "Hide" : "Show"} Advanced Options
          </Button>

          {showAdvanced && (
            <div className="grid gap-4 md:grid-cols-3 pt-2">
              <div className="space-y-2">
                <label className="text-sm font-medium">Capacity (bytes)</label>
                <Input
                  type="number"
                  value={capacity}
                  onChange={(e) => setCapacity(e.target.value)}
                  placeholder="1000000000"
                />
                <p className="text-xs text-muted-foreground">
                  {formatBytes(parseInt(capacity, 10) || 0)}
                </p>
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Duration (blocks)</label>
                <Input
                  type="number"
                  value={duration}
                  onChange={(e) => setDuration(e.target.value)}
                  placeholder="500"
                />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Max Payment (12 decimals)</label>
                <Input
                  type="number"
                  value={maxPayment}
                  onChange={(e) => setMaxPayment(e.target.value)}
                  placeholder="1000000000000000"
                />
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      {/* Drives List */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {drives.length === 0 ? (
          <Card className="col-span-full">
            <CardContent className="py-8 text-center">
              <HardDrive className="mx-auto h-12 w-12 mb-4 opacity-50" />
              <p className="text-muted-foreground">No drives yet</p>
              <p className="text-sm text-muted-foreground">
                Create your first drive to start organizing files
              </p>
            </CardContent>
          </Card>
        ) : (
          drives.map((drive) => (
            <Card
              key={drive.driveId.toString()}
              className={`cursor-pointer transition-colors hover:border-primary ${
                selectedDrive?.driveId === drive.driveId ? "border-primary" : ""
              }`}
              onClick={() => setSelectedDrive(drive)}
            >
              <CardHeader className="pb-2">
                <div className="flex items-center justify-between">
                  <CardTitle className="flex items-center gap-2 text-lg">
                    <HardDrive className="h-5 w-5 text-primary" />
                    {drive.name || `Drive ${drive.driveId.toString()}`}
                  </CardTitle>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDeleteDrive(drive);
                    }}
                  >
                    <Trash2 className="h-4 w-4 text-muted-foreground hover:text-destructive" />
                  </Button>
                </div>
              </CardHeader>
              <CardContent>
                <div className="grid grid-cols-2 gap-2 text-sm">
                  <div>
                    <p className="text-muted-foreground">Files</p>
                    <p className="font-medium">{drive.fileCount || 0}</p>
                  </div>
                  <div>
                    <p className="text-muted-foreground">Size</p>
                    <p className="font-medium">
                      {formatBytes(drive.totalSize || 0)}
                    </p>
                  </div>
                </div>
                <div className="mt-2 text-xs text-muted-foreground">
                  <p>ID: {drive.driveId.toString()}</p>
                  <p>Bucket ID: {drive.bucketId.toString()}</p>
                </div>
                {drive.rootCid && (
                  <p className="mt-2 font-mono text-xs text-muted-foreground truncate">
                    Root: {drive.rootCid}
                  </p>
                )}
              </CardContent>
            </Card>
          ))
        )}
      </div>

      {/* Selected Drive Explorer */}
      {selectedDrive && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Folder className="h-5 w-5" />
              {selectedDrive.name || `Drive ${selectedDrive.driveId.toString()}`}
              <ChevronRight className="h-4 w-4" />
              <span className="text-muted-foreground">/</span>
            </CardTitle>
            <CardDescription>Browse files in this drive</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="rounded-lg border">
              <div className="p-4 text-center text-muted-foreground">
                <File className="mx-auto h-8 w-8 mb-2 opacity-50" />
                <p>This drive is empty</p>
                <p className="text-sm">Go to Upload page to add files</p>
              </div>
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
