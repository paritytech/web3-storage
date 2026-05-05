/**
 * State barrel — exports every hook + action so components can import from
 * a single path.
 */

export {
  useConnectionStatus,
  useBlockNumber,
  useEndpoint,
  useConnectionError,
  useIsConnected,
  useIsConnecting,
  connect,
  disconnect,
  reconnect,
  getApi,
  getEndpoint,
  isConnected,
} from "./chain.state";

export {
  useSelectedNetworkId,
  useSelectedNetwork,
  useParachainWs,
  useProviderHttp,
  useNetworkList,
  selectNetwork,
  selectCustomNetwork,
  getSelectedNetwork,
  getParachainWs,
  getProviderHttp,
} from "./network.state";

export {
  useSignerAddress,
  useSignerName,
  useBalance,
  useIsSettingSigner,
  useHasSigner,
  setSigner,
  selectDevAccount,
  clearSigner,
  refreshBalance,
  restoreSigner,
  getSignerAddress,
  DEV_ACCOUNTS,
} from "./wallet.state";
export type { DevAccount, AccountBalance } from "./wallet.state";

export {
  useDrives,
  useSelectedDrive,
  useCurrentPath,
  useEntries,
  useDriveLoading,
  useUploading,
  useDriveError,
  useViewMode,
  useCreations,
  refreshDrives,
  selectDrive,
  navigateTo,
  navigateUp,
  refreshDirectory,
  setViewMode,
  uploadFiles,
  abortUpload,
  downloadFile,
  deleteEntry,
  createFolder,
  createDrive,
  deleteDrive,
  renameDrive,
  clearDriveContents,
  commitChanges,
  fetchMembers,
  addMember,
  removeMember,
  dismissCreation,
  getDrives,
  getSelectedDrive,
  getCurrentPath,
} from "./drive.state";
export type { CreationStage, CreationStatus, ViewMode, CommitStrategy } from "./drive.state";

export {
  useCheckpointInfo,
  useCheckpointDuty,
  useCheckpointLoading,
  refreshCheckpoint,
  triggerCheckpoint,
  clearCheckpointState,
} from "./checkpoint.state";
