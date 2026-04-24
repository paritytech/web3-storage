import { useState, useEffect, useCallback, useRef } from "react";
import {
  Search,
  Activity,
  Box,
  Database,
  Archive,
  RefreshCw,
  ChevronDown,
  ChevronRight,
  Filter,
  Zap,
  Shield,
  AlertTriangle,
  CheckCircle,
  Clock,
  User,
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
import { parachain } from "@polkadot-api/descriptors";
import { truncateHash } from "@/lib/utils";

// Event types we care about
type PalletFilter = "all" | "StorageProvider" | "S3Registry";

interface BlockEvent {
  blockNumber: number;
  blockHash: string;
  timestamp: number;
  pallet: string;
  eventName: string;
  eventData: Record<string, unknown>;
  extrinsicIndex?: number;
}

interface BlockInfo {
  number: number;
  hash: string;
  timestamp: number;
  eventCount: number;
  events: BlockEvent[];
}

// Map event types to icons and colors
const eventStyles: Record<string, { icon: typeof Activity; color: string; bg: string }> = {
  // StorageProvider events
  ProviderRegistered: { icon: User, color: "text-green-500", bg: "bg-green-500/10" },
  ProviderDeregistered: { icon: User, color: "text-red-500", bg: "bg-red-500/10" },
  ProviderStakeAdded: { icon: Database, color: "text-blue-500", bg: "bg-blue-500/10" },
  BucketCreated: { icon: Box, color: "text-purple-500", bg: "bg-purple-500/10" },
  BucketFrozen: { icon: Shield, color: "text-yellow-500", bg: "bg-yellow-500/10" },
  BucketDeleted: { icon: Box, color: "text-red-500", bg: "bg-red-500/10" },
  BucketCheckpointed: { icon: CheckCircle, color: "text-green-500", bg: "bg-green-500/10" },
  AgreementRequested: { icon: Clock, color: "text-blue-500", bg: "bg-blue-500/10" },
  AgreementAccepted: { icon: CheckCircle, color: "text-green-500", bg: "bg-green-500/10" },
  AgreementRejected: { icon: AlertTriangle, color: "text-red-500", bg: "bg-red-500/10" },
  AgreementEnded: { icon: Clock, color: "text-gray-500", bg: "bg-gray-500/10" },
  ChallengeCreated: { icon: AlertTriangle, color: "text-orange-500", bg: "bg-orange-500/10" },
  ChallengeDefended: { icon: Shield, color: "text-green-500", bg: "bg-green-500/10" },
  ChallengeSlashed: { icon: Zap, color: "text-red-500", bg: "bg-red-500/10" },
  ProviderCheckpointSubmitted: { icon: CheckCircle, color: "text-blue-500", bg: "bg-blue-500/10" },
  // S3Registry events
  S3BucketCreated: { icon: Archive, color: "text-cyan-500", bg: "bg-cyan-500/10" },
  S3BucketDeleted: { icon: Archive, color: "text-red-500", bg: "bg-red-500/10" },
};

const defaultStyle = { icon: Activity, color: "text-gray-500", bg: "bg-gray-500/10" };

// Module-level cache so events survive tab switches (component unmount/remount)
let cachedEvents: BlockEvent[] = [];
let cachedBlocks: BlockInfo[] = [];

export default function Explorer() {
  const { client, connected, blockNumber } = useChain();
  const [blocks, setBlocks] = useState<BlockInfo[]>(cachedBlocks);
  const [events, setEvents] = useState<BlockEvent[]>(cachedEvents);
  const [loading, setLoading] = useState(false);
  const [palletFilter, setPalletFilter] = useState<PalletFilter>("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [expandedBlocks, setExpandedBlocks] = useState<Set<number>>(new Set());
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [subscriptionActive, setSubscriptionActive] = useState(false);
  const subscriptionRef = useRef<{ unsubscribe: () => void } | null>(null);
  const blockNumberRef = useRef(blockNumber);
  blockNumberRef.current = blockNumber;

  // Keep module-level cache in sync so events survive tab switches
  useEffect(() => { cachedEvents = events; }, [events]);
  useEffect(() => { cachedBlocks = blocks; }, [blocks]);

  // Layer 0 pallets we care about
  const layer0Pallets = ["StorageProvider", "S3Registry"];

  // Parse events from event records
  const parseEventRecords = useCallback((eventsAtBlock: unknown[], currentBlockNumber: number, blockHash: string = ""): BlockEvent[] => {
    const blockEvents: BlockEvent[] = [];

    for (const eventRecord of eventsAtBlock) {
      try {
        // Handle the event record structure
        const record = eventRecord as {
          event?: { type: string; value?: { type?: string; value?: unknown } };
          phase?: { type: string; value?: number };
        };

        const event = record.event;
        const phase = record.phase;

        if (!event) continue;

        const pallet = event.type;
        const eventValue = event.value;
        const eventName = eventValue?.type || "Unknown";
        const eventData = (eventValue?.value || {}) as Record<string, unknown>;

        if (layer0Pallets.includes(pallet)) {
          blockEvents.push({
            blockNumber: currentBlockNumber,
            blockHash,
            timestamp: Date.now(),
            pallet,
            eventName,
            eventData,
            extrinsicIndex: phase?.type === "ApplyExtrinsic"
              ? phase.value
              : undefined,
          });
        }
      } catch (err) {
        console.warn("Error parsing event:", err);
      }
    }

    return blockEvents;
  }, []);

  // Subscribe to finalized events
  useEffect(() => {
    if (!client || !connected || !autoRefresh) {
      if (subscriptionRef.current) {
        subscriptionRef.current.unsubscribe();
        subscriptionRef.current = null;
        setSubscriptionActive(false);
      }
      return;
    }

    const api = client.getTypedApi(parachain);

    try {
      // Subscribe to finalized events
      const subscription = api.query.System.Events.watchValue({ at: "finalized" }).subscribe({
        next: (rawResult) => {
          // PAPI watchValue returns { block, value } — unwrap to get the events array
          const eventsAtBlock = Array.isArray(rawResult)
            ? rawResult
            : (rawResult as any)?.value ?? rawResult;
          const blockInfo = (rawResult as any)?.block;
          const blockHash: string = blockInfo?.hash ?? "";

          if (!eventsAtBlock || !Array.isArray(eventsAtBlock)) {
            console.log("[Explorer] Events not an array after unwrap:", typeof eventsAtBlock, eventsAtBlock);
            return;
          }

          // Use block number from the event payload if available, otherwise fall back to ref
          const currentBlock: number = blockInfo?.number ?? blockNumberRef.current;

          // Log first event structure for debugging PAPI format
          if (eventsAtBlock.length > 0) {
            const sample = eventsAtBlock[0] as Record<string, unknown>;
            console.log("[Explorer] Block", currentBlock, "- received", eventsAtBlock.length,
              "events. Sample keys:", Object.keys(sample),
              "event.type:", (sample.event as any)?.type);
          }

          const newEvents = parseEventRecords(eventsAtBlock, currentBlock, blockHash);

          if (newEvents.length > 0) {
            console.log(`Found ${newEvents.length} Layer 0 events in block ${currentBlock}`);

            // Update events list
            setEvents(prev => {
              const combined = [...newEvents, ...prev];
              // Keep only last 500 events
              return combined.slice(0, 500);
            });

            // Update blocks list
            setBlocks(prev => {
              const newBlock: BlockInfo = {
                number: currentBlock,
                hash: "",
                timestamp: Date.now(),
                eventCount: newEvents.length,
                events: newEvents,
              };

              // Find existing block or add new one
              const existingIdx = prev.findIndex(b => b.number === currentBlock);
              if (existingIdx >= 0) {
                const updated = [...prev];
                updated[existingIdx] = newBlock;
                return updated;
              }

              // Add new block and keep only last 50
              return [newBlock, ...prev].slice(0, 50);
            });
          }
        },
        error: (err) => {
          console.error("Event subscription error:", err);
          setSubscriptionActive(false);
        },
      });

      subscriptionRef.current = subscription;
      setSubscriptionActive(true);
      console.log("Event subscription active");
    } catch (err) {
      console.error("Failed to subscribe to events:", err);
    }

    return () => {
      if (subscriptionRef.current) {
        subscriptionRef.current.unsubscribe();
        subscriptionRef.current = null;
        setSubscriptionActive(false);
      }
    };
  }, [client, connected, autoRefresh, parseEventRecords]);

  // Manual refresh - fetch current events
  const loadCurrentEvents = useCallback(async () => {
    if (!client || !connected || loading) return;

    setLoading(true);
    try {
      const api = client.getTypedApi(parachain);

      // Get current events — getValue may return raw array or { block, value }
      const rawResult = await api.query.System.Events.getValue();
      const eventsAtBlock = Array.isArray(rawResult)
        ? rawResult
        : (rawResult as any)?.value ?? rawResult;

      if (eventsAtBlock && Array.isArray(eventsAtBlock)) {
        const newEvents = parseEventRecords(eventsAtBlock, blockNumber);
        console.log(`Manual refresh: Found ${newEvents.length} Layer 0 events`);

        if (newEvents.length > 0) {
          setEvents(prev => {
            // Add new events, avoiding duplicates by checking block number
            const combined = [...newEvents, ...prev.filter(e => e.blockNumber !== blockNumber)];
            return combined.slice(0, 500);
          });

          setBlocks(prev => {
            const newBlock: BlockInfo = {
              number: blockNumber,
              hash: "",
              timestamp: Date.now(),
              eventCount: newEvents.length,
              events: newEvents,
            };

            const existingIdx = prev.findIndex(b => b.number === blockNumber);
            if (existingIdx >= 0) {
              const updated = [...prev];
              updated[existingIdx] = newBlock;
              return updated;
            }

            return [newBlock, ...prev].slice(0, 50);
          });
        }
      }
    } catch (err) {
      console.error("Error loading events:", err);
    } finally {
      setLoading(false);
    }
  }, [client, connected, blockNumber, loading, parseEventRecords]);

  const toggleBlockExpand = (blockNum: number) => {
    setExpandedBlocks(prev => {
      const newSet = new Set(prev);
      if (newSet.has(blockNum)) {
        newSet.delete(blockNum);
      } else {
        newSet.add(blockNum);
      }
      return newSet;
    });
  };

  // Filter events
  const filteredEvents = events.filter(event => {
    const matchesPallet = palletFilter === "all" || event.pallet === palletFilter;
    const matchesSearch = searchQuery === "" ||
      event.eventName.toLowerCase().includes(searchQuery.toLowerCase()) ||
      event.pallet.toLowerCase().includes(searchQuery.toLowerCase()) ||
      JSON.stringify(event.eventData).toLowerCase().includes(searchQuery.toLowerCase());
    return matchesPallet && matchesSearch;
  });

  // Format event data for display
  const formatEventValue = (value: unknown): string => {
    if (value === null || value === undefined) return "-";
    if (typeof value === "bigint") return value.toString();
    if (typeof value === "object") {
      const obj = value as Record<string, unknown>;
      if ("asHex" in obj && typeof obj.asHex === "function") {
        return (obj.asHex as () => string)();
      }
      if ("asText" in obj && typeof obj.asText === "function") {
        return (obj.asText as () => string)();
      }
      if ("asBytes" in obj && typeof obj.asBytes === "function") {
        const bytes = (obj.asBytes as () => Uint8Array)();
        return "0x" + Array.from(bytes).map(b => b.toString(16).padStart(2, "0")).join("");
      }
      return JSON.stringify(value);
    }
    return String(value);
  };

  if (!connected) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Block Explorer</h1>
          <p className="text-muted-foreground">
            View Layer 0 blockchain events and transactions
          </p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <Activity className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p className="text-muted-foreground">
              Connect to the network to view blockchain activity
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
          <h1 className="text-3xl font-bold tracking-tight">Block Explorer</h1>
          <p className="text-muted-foreground">
            View Layer 0 blockchain events and transactions
          </p>
        </div>
        <div className="flex items-center gap-2">
          <div className="flex items-center gap-1 text-sm text-muted-foreground">
            <Box className="h-4 w-4" />
            Block #{blockNumber}
          </div>
          {subscriptionActive && (
            <span className="px-2 py-0.5 text-xs rounded-full bg-green-500/10 text-green-500">
              Live
            </span>
          )}
          <Button
            variant={autoRefresh ? "default" : "outline"}
            size="sm"
            onClick={() => setAutoRefresh(!autoRefresh)}
          >
            <Activity className={`mr-2 h-4 w-4 ${autoRefresh ? "animate-pulse" : ""}`} />
            {autoRefresh ? "Live" : "Paused"}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={loadCurrentEvents}
            disabled={loading}
          >
            <RefreshCw className={`mr-2 h-4 w-4 ${loading ? "animate-spin" : ""}`} />
            Refresh
          </Button>
        </div>
      </div>

      {/* Stats Cards */}
      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              Total Events
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{events.length}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              StorageProvider
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold text-purple-500">
              {events.filter(e => e.pallet === "StorageProvider").length}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              S3Registry
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold text-cyan-500">
              {events.filter(e => e.pallet === "S3Registry").length}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Info message when no events */}
      {events.length === 0 && (
        <Card className="border-dashed">
          <CardContent className="py-6">
            <div className="text-center text-muted-foreground">
              <Activity className="mx-auto h-8 w-8 mb-2 opacity-50" />
              <p className="font-medium">Listening for Layer 0 events...</p>
              <p className="text-sm mt-1">
                Events from StorageProvider and S3Registry pallets will appear here in real time.
              </p>
              <p className="text-sm mt-2">
                This only shows <strong>new</strong> events from this point forward.
                Try creating a bucket or running <code>just s3-demo-ci</code> now.
              </p>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Filters */}
      {events.length > 0 && (
        <Card>
          <CardHeader className="pb-3">
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2">
                <Filter className="h-5 w-5" />
                Events
              </CardTitle>
              <div className="flex items-center gap-2">
                <div className="flex gap-1">
                  {(["all", "StorageProvider", "S3Registry"] as PalletFilter[]).map(
                    (filter) => (
                      <Button
                        key={filter}
                        variant={palletFilter === filter ? "default" : "outline"}
                        size="sm"
                        onClick={() => setPalletFilter(filter)}
                      >
                        {filter === "all" ? "All" : filter.replace("Registry", "")}
                      </Button>
                    )
                  )}
                </div>
                <div className="relative">
                  <Search className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                  <Input
                    placeholder="Search events..."
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    className="pl-8 w-64"
                  />
                </div>
              </div>
            </div>
          </CardHeader>
          <CardContent>
            {filteredEvents.length === 0 ? (
              <div className="rounded-lg border p-8 text-center text-muted-foreground">
                <Activity className="mx-auto h-12 w-12 mb-4 opacity-50" />
                <p>No events found</p>
                <p className="text-sm">
                  {searchQuery
                    ? "Try adjusting your search"
                    : "Waiting for new blockchain activity..."}
                </p>
              </div>
            ) : (
              <div className="space-y-2">
                {filteredEvents.slice(0, 50).map((event, idx) => {
                  const style = eventStyles[event.eventName] || defaultStyle;
                  const Icon = style.icon;

                  return (
                    <div
                      key={`${event.blockNumber}-${event.eventName}-${idx}`}
                      className={`rounded-lg border p-4 ${style.bg}`}
                    >
                      <div className="flex items-start justify-between">
                        <div className="flex items-start gap-3">
                          <div className={`mt-0.5 ${style.color}`}>
                            <Icon className="h-5 w-5" />
                          </div>
                          <div>
                            <div className="flex items-center gap-2">
                              <span className="font-semibold">{event.eventName}</span>
                              <span className="text-xs px-2 py-0.5 rounded bg-muted">
                                {event.pallet}
                              </span>
                            </div>
                            <div className="mt-2 space-y-1">
                              {Object.entries(event.eventData).map(([key, value]) => (
                                <div key={key} className="text-sm">
                                  <span className="text-muted-foreground">{key}: </span>
                                  <span className="font-mono">
                                    {truncateHash(formatEventValue(value), 20, 8)}
                                  </span>
                                </div>
                              ))}
                            </div>
                          </div>
                        </div>
                        <div className="text-right text-sm text-muted-foreground">
                          <div className="flex items-center gap-1">
                            <Box className="h-3 w-3" />
                            #{event.blockNumber}
                          </div>
                          {event.extrinsicIndex !== undefined && (
                            <div className="text-xs">Extrinsic #{event.extrinsicIndex}</div>
                          )}
                        </div>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </CardContent>
        </Card>
      )}

      {/* Recent Blocks */}
      {blocks.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Box className="h-5 w-5" />
              Recent Blocks with Events
            </CardTitle>
            <CardDescription>
              {blocks.length} blocks with Layer 0 events
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="space-y-2">
              {blocks.map((block) => (
                <div
                  key={block.number}
                  className="rounded-lg border"
                >
                  <button
                    className="w-full p-4 flex items-center justify-between hover:bg-muted/50 transition-colors"
                    onClick={() => toggleBlockExpand(block.number)}
                  >
                    <div className="flex items-center gap-4">
                      <div className="flex items-center gap-2">
                        {expandedBlocks.has(block.number) ? (
                          <ChevronDown className="h-4 w-4" />
                        ) : (
                          <ChevronRight className="h-4 w-4" />
                        )}
                        <Box className="h-5 w-5 text-primary" />
                      </div>
                      <div className="text-left">
                        <div className="font-semibold">Block #{block.number}</div>
                        {block.hash && (
                          <div className="text-xs text-muted-foreground font-mono">
                            {truncateHash(block.hash, 12, 8)}
                          </div>
                        )}
                      </div>
                    </div>
                    <div className="flex items-center gap-4">
                      <div className="text-sm">
                        <span className="text-muted-foreground">Events: </span>
                        <span className={block.eventCount > 0 ? "text-primary font-semibold" : ""}>
                          {block.eventCount}
                        </span>
                      </div>
                    </div>
                  </button>

                  {expandedBlocks.has(block.number) && block.events.length > 0 && (
                    <div className="border-t px-4 py-2 bg-muted/30">
                      <div className="space-y-2">
                        {block.events.map((event, idx) => {
                          const style = eventStyles[event.eventName] || defaultStyle;
                          const Icon = style.icon;

                          return (
                            <div
                              key={`${event.eventName}-${idx}`}
                              className="flex items-center gap-2 text-sm py-1"
                            >
                              <Icon className={`h-4 w-4 ${style.color}`} />
                              <span className="font-medium">{event.eventName}</span>
                              <span className="text-xs px-1.5 py-0.5 rounded bg-muted">
                                {event.pallet}
                              </span>
                            </div>
                          );
                        })}
                      </div>
                    </div>
                  )}

                  {expandedBlocks.has(block.number) && block.events.length === 0 && (
                    <div className="border-t px-4 py-3 bg-muted/30 text-sm text-muted-foreground">
                      No Layer 0 events in this block
                    </div>
                  )}
                </div>
              ))}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
