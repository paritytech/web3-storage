import { useState } from "react";
import {
  Search,
  HardDrive,
  Archive,
  Folder,
  File,
  ChevronRight,
  ArrowLeft,
  RefreshCw,
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
import { formatBytes } from "@/lib/utils";

type ExplorerMode = "drives" | "buckets";

interface FileEntry {
  name: string;
  type: "file" | "directory";
  size: number;
  cid?: string;
  lastModified: number;
}

interface BreadcrumbItem {
  name: string;
  path: string;
}

export default function Explorer() {
  const { connected } = useChain();
  const [mode, setMode] = useState<ExplorerMode>("drives");
  const [selectedItem, setSelectedItem] = useState<string | null>(null);
  const [currentPath, setCurrentPath] = useState<BreadcrumbItem[]>([]);
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [searchQuery, setSearchQuery] = useState("");

  const navigateTo = (item: BreadcrumbItem) => {
    const index = currentPath.findIndex((p) => p.path === item.path);
    setCurrentPath(currentPath.slice(0, index + 1));
    // TODO: Load entries for this path
  };

  const navigateUp = () => {
    if (currentPath.length > 0) {
      setCurrentPath(currentPath.slice(0, -1));
      // TODO: Load entries for parent path
    } else {
      setSelectedItem(null);
      setEntries([]);
    }
  };

  const openEntry = (entry: FileEntry) => {
    if (entry.type === "directory") {
      const newPath = currentPath.length
        ? `${currentPath[currentPath.length - 1].path}/${entry.name}`
        : `/${entry.name}`;
      setCurrentPath([...currentPath, { name: entry.name, path: newPath }]);
      // TODO: Load entries for this directory
    } else {
      // TODO: Show file details or download
    }
  };

  const filteredEntries = entries.filter((entry) =>
    entry.name.toLowerCase().includes(searchQuery.toLowerCase())
  );

  if (!connected) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Explorer</h1>
          <p className="text-muted-foreground">Browse your storage</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <Search className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p className="text-muted-foreground">
              Connect to the network to browse storage
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
          <h1 className="text-3xl font-bold tracking-tight">Explorer</h1>
          <p className="text-muted-foreground">Browse your storage</p>
        </div>
        <div className="flex gap-2">
          <Button
            variant={mode === "drives" ? "default" : "outline"}
            size="sm"
            onClick={() => {
              setMode("drives");
              setSelectedItem(null);
              setCurrentPath([]);
              setEntries([]);
            }}
          >
            <HardDrive className="mr-2 h-4 w-4" />
            Drives
          </Button>
          <Button
            variant={mode === "buckets" ? "default" : "outline"}
            size="sm"
            onClick={() => {
              setMode("buckets");
              setSelectedItem(null);
              setCurrentPath([]);
              setEntries([]);
            }}
          >
            <Archive className="mr-2 h-4 w-4" />
            Buckets
          </Button>
        </div>
      </div>

      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              {(selectedItem || currentPath.length > 0) && (
                <Button variant="ghost" size="icon" onClick={navigateUp}>
                  <ArrowLeft className="h-4 w-4" />
                </Button>
              )}
              <CardTitle className="flex items-center gap-1">
                {mode === "drives" ? (
                  <HardDrive className="h-5 w-5" />
                ) : (
                  <Archive className="h-5 w-5" />
                )}
                {selectedItem ? (
                  <>
                    <span>{selectedItem}</span>
                    {currentPath.map((item, i) => (
                      <span key={item.path} className="flex items-center">
                        <ChevronRight className="h-4 w-4 mx-1" />
                        <button
                          className="hover:text-primary"
                          onClick={() => navigateTo(item)}
                        >
                          {item.name}
                        </button>
                      </span>
                    ))}
                  </>
                ) : (
                  <span>
                    {mode === "drives" ? "All Drives" : "All Buckets"}
                  </span>
                )}
              </CardTitle>
            </div>
            <div className="flex items-center gap-2">
              <div className="relative">
                <Search className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                <Input
                  placeholder="Search..."
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="pl-8 w-64"
                />
              </div>
              <Button variant="outline" size="icon">
                <RefreshCw className="h-4 w-4" />
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          {!selectedItem ? (
            // Show drives/buckets list
            <div className="rounded-lg border">
              <div className="p-8 text-center text-muted-foreground">
                {mode === "drives" ? (
                  <>
                    <HardDrive className="mx-auto h-12 w-12 mb-4 opacity-50" />
                    <p>No drives found</p>
                    <p className="text-sm">Create a drive to get started</p>
                  </>
                ) : (
                  <>
                    <Archive className="mx-auto h-12 w-12 mb-4 opacity-50" />
                    <p>No buckets found</p>
                    <p className="text-sm">Create a bucket to get started</p>
                  </>
                )}
              </div>
            </div>
          ) : filteredEntries.length === 0 ? (
            <div className="rounded-lg border p-8 text-center text-muted-foreground">
              <Folder className="mx-auto h-12 w-12 mb-4 opacity-50" />
              <p>
                {searchQuery
                  ? "No matching items found"
                  : "This location is empty"}
              </p>
            </div>
          ) : (
            <div className="rounded-lg border">
              <table className="w-full">
                <thead>
                  <tr className="border-b bg-muted/50">
                    <th className="px-4 py-2 text-left text-sm font-medium">
                      Name
                    </th>
                    <th className="px-4 py-2 text-left text-sm font-medium">
                      Size
                    </th>
                    <th className="px-4 py-2 text-left text-sm font-medium">
                      Modified
                    </th>
                    <th className="px-4 py-2 text-left text-sm font-medium">
                      CID
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {filteredEntries.map((entry) => (
                    <tr
                      key={entry.name}
                      className="border-b cursor-pointer hover:bg-muted/50"
                      onClick={() => openEntry(entry)}
                    >
                      <td className="px-4 py-2">
                        <div className="flex items-center gap-2">
                          {entry.type === "directory" ? (
                            <Folder className="h-4 w-4 text-primary" />
                          ) : (
                            <File className="h-4 w-4 text-muted-foreground" />
                          )}
                          <span className="font-medium">{entry.name}</span>
                        </div>
                      </td>
                      <td className="px-4 py-2 text-sm text-muted-foreground">
                        {entry.type === "directory"
                          ? "-"
                          : formatBytes(entry.size)}
                      </td>
                      <td className="px-4 py-2 text-sm text-muted-foreground">
                        {new Date(entry.lastModified).toLocaleDateString()}
                      </td>
                      <td className="px-4 py-2 font-mono text-xs text-muted-foreground">
                        {entry.cid ? `${entry.cid.slice(0, 10)}...` : "-"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
