import { useState } from "react";
import {
  HardDrive,
  Plus,
  Folder,
  File,
  RefreshCw,
  Trash2,
  ChevronRight,
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
import { toast } from "@/components/ui/toaster";

interface Drive {
  id: string;
  name: string;
  rootCid: string | null;
  createdAt: number;
  fileCount: number;
  totalSize: number;
}

export default function Drives() {
  const { connected } = useChain();
  const [drives, setDrives] = useState<Drive[]>([]);
  const [newDriveName, setNewDriveName] = useState("");
  const [creating, setCreating] = useState(false);
  const [selectedDrive, setSelectedDrive] = useState<Drive | null>(null);

  const handleCreateDrive = async () => {
    if (!newDriveName.trim()) {
      toast({ title: "Error", description: "Drive name is required", variant: "destructive" });
      return;
    }

    setCreating(true);
    try {
      // TODO: Call SDK to create drive
      const newDrive: Drive = {
        id: `drive-${Date.now()}`,
        name: newDriveName,
        rootCid: null,
        createdAt: Date.now(),
        fileCount: 0,
        totalSize: 0,
      };
      setDrives([...drives, newDrive]);
      setNewDriveName("");
      toast({ title: "Success", description: `Drive "${newDriveName}" created` });
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

  const handleDeleteDrive = async (drive: Drive) => {
    try {
      // TODO: Call SDK to delete drive
      setDrives(drives.filter((d) => d.id !== drive.id));
      if (selectedDrive?.id === drive.id) {
        setSelectedDrive(null);
      }
      toast({ title: "Success", description: `Drive "${drive.name}" deleted` });
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

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Drives</h1>
          <p className="text-muted-foreground">Manage your File System drives</p>
        </div>
        <Button variant="outline" size="sm">
          <RefreshCw className="mr-2 h-4 w-4" />
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
        <CardContent>
          <div className="flex gap-3">
            <Input
              placeholder="Drive name"
              value={newDriveName}
              onChange={(e) => setNewDriveName(e.target.value)}
              className="max-w-sm"
            />
            <Button onClick={handleCreateDrive} disabled={creating}>
              {creating ? "Creating..." : "Create Drive"}
            </Button>
          </div>
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
              key={drive.id}
              className={`cursor-pointer transition-colors hover:border-primary ${
                selectedDrive?.id === drive.id ? "border-primary" : ""
              }`}
              onClick={() => setSelectedDrive(drive)}
            >
              <CardHeader className="pb-2">
                <div className="flex items-center justify-between">
                  <CardTitle className="flex items-center gap-2 text-lg">
                    <HardDrive className="h-5 w-5 text-primary" />
                    {drive.name}
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
                    <p className="font-medium">{drive.fileCount}</p>
                  </div>
                  <div>
                    <p className="text-muted-foreground">Size</p>
                    <p className="font-medium">
                      {drive.totalSize === 0
                        ? "0 B"
                        : `${(drive.totalSize / 1024).toFixed(1)} KB`}
                    </p>
                  </div>
                </div>
                {drive.rootCid && (
                  <p className="mt-2 font-mono text-xs text-muted-foreground truncate">
                    CID: {drive.rootCid}
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
              {selectedDrive.name}
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
                <p className="text-sm">Upload files to get started</p>
              </div>
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
