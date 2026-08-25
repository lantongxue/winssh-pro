import React, { useState, useRef, useEffect } from 'react';
import {
  FolderSync,
  Upload,
  Download,
  FileCode,
  Folder,
  File,
  FileText,
  FileArchive,
  Trash2,
  Edit2,
  RefreshCw,
  Plus,
  ArrowRight,
  ArrowLeft,
  ChevronRight,
  ChevronUp,
  ChevronDown,
  Home,
  HardDrive,
  Laptop,
  Server,
  X,
  Maximize2,
  Minimize2,
  Search,
  FolderPlus,
  Terminal as TerminalIcon,
  Copy,
  Check,
  Columns,
  Lock,
  MoreVertical,
  ArrowUpDown,
  Shield,
  Eye,
  EyeOff,
  CornerDownRight,
} from 'lucide-react';
import {
  SFTPFile,
  SFTPTransferItem,
  ServerHost,
  AppSettings,
  SessionTab,
} from '../types';
import { INITIAL_SFTP_FILES } from '../data/defaultData';

interface SessionBottomSFTPPanelProps {
  activeTab: SessionTab;
  activeHost: ServerHost;
  terminalCwd: string;
  sftpFiles: Record<string, SFTPFile[]>;
  onUpdateSftpFiles: (updated: Record<string, SFTPFile[]>) => void;
  onOpenFileEditor: (file: { path: string; name: string; content: string }) => void;
  onExecuteCommandInTerminal?: (command: string) => void;
  onClose: () => void;
  onUpdateTabState: (tabId: string, updates: Partial<SessionTab>) => void;
  settings: AppSettings;
}

export const SessionBottomSFTPPanel: React.FC<SessionBottomSFTPPanelProps> = ({
  activeTab,
  activeHost,
  terminalCwd,
  sftpFiles,
  onUpdateSftpFiles,
  onOpenFileEditor,
  onExecuteCommandInTerminal,
  onClose,
  onUpdateTabState,
  settings,
}) => {
  // Remote path state (per tab, defaults to terminalCwd or /root)
  const currentRemotePath = activeTab.sftpRemotePath || terminalCwd || '/root';
  const [remotePathInput, setRemotePathInput] = useState<string>(currentRemotePath);
  const [selectedRemoteFile, setSelectedRemoteFile] = useState<SFTPFile | null>(null);
  const [searchFilter, setSearchFilter] = useState<string>('');
  const [showHidden, setShowHidden] = useState<boolean>(settings.sftpShowHidden ?? true);
  const [viewMode, setViewMode] = useState<'dual' | 'remote-only'>(activeTab.sftpViewMode || 'dual');
  const [followTerminalCwd, setFollowTerminalCwd] = useState<boolean>(
    activeTab.sftpFollowTerminalCwd ?? true
  );

  // Local workstation state
  const localPath = activeTab.sftpLocalPath || '/Users/developer/Downloads';
  const [selectedLocalFile, setSelectedLocalFile] = useState<string | null>(null);

  // Transfer Queue state
  const [transfers, setTransfers] = useState<SFTPTransferItem[]>([
    {
      id: 'tx-init',
      filename: 'nginx.conf.bak',
      direction: 'download',
      size: 1840,
      transferred: 1840,
      speed: '14.8 MB/s',
      status: 'completed',
      progress: 100,
      hostName: activeHost.name,
    },
  ]);
  const [showTransferQueue, setShowTransferQueue] = useState<boolean>(false);

  // Modals / Dropdowns
  const [showNewFileModal, setShowNewFileModal] = useState(false);
  const [newFileName, setNewFileName] = useState('');
  const [showNewFolderModal, setShowNewFolderModal] = useState(false);
  const [newFolderName, setNewFolderName] = useState('');
  const [showChmodModal, setShowChmodModal] = useState(false);
  const [chmodValue, setChmodValue] = useState('755');
  const [fileToChmod, setFileToChmod] = useState<SFTPFile | null>(null);
  const [fileToRename, setFileToRename] = useState<SFTPFile | null>(null);
  const [renameValue, setRenameValue] = useState('');
  const [contextMenuFile, setContextMenuFile] = useState<{
    file: SFTPFile;
    x: number;
    y: number;
  } | null>(null);
  const [copiedNotification, setCopiedNotification] = useState<string | null>(null);

  // Drag and drop upload simulator
  const [isDragOver, setIsDragOver] = useState(false);

  // Panel Resizing logic
  const [isDraggingResize, setIsDraggingResize] = useState(false);
  const startYRef = useRef<number>(0);
  const startHeightRef = useRef<number>(0);
  const panelHeight = activeTab.sftpHeight || 280;

  // Sync with terminal CWD if follow mode is active
  useEffect(() => {
    if (followTerminalCwd && terminalCwd && terminalCwd !== currentRemotePath) {
      handleNavigateRemote(terminalCwd, false);
    }
  }, [terminalCwd, followTerminalCwd]);

  // Keep path input in sync with tab remote path
  useEffect(() => {
    setRemotePathInput(currentRemotePath);
  }, [currentRemotePath]);

  // Close context menu on outside click
  useEffect(() => {
    const handleGlobalClick = () => setContextMenuFile(null);
    window.addEventListener('click', handleGlobalClick);
    return () => window.removeEventListener('click', handleGlobalClick);
  }, []);

  // Resize handler
  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!isDraggingResize) return;
      const deltaY = startYRef.current - e.clientY;
      const newHeight = Math.min(650, Math.max(140, startHeightRef.current + deltaY));
      onUpdateTabState(activeTab.id, { sftpHeight: newHeight });
    };

    const handleMouseUp = () => {
      setIsDraggingResize(false);
    };

    if (isDraggingResize) {
      window.addEventListener('mousemove', handleMouseMove);
      window.addEventListener('mouseup', handleMouseUp);
    }
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isDraggingResize, activeTab.id]);

  // Simulated local workstation files
  const localFiles = [
    { name: 'deploy_v2_bundle.tar.gz', size: 14820000, modified: 'Today, 10:14', isDir: false },
    { name: 'id_rsa_backup.pem', size: 3240, modified: 'Yesterday, 18:22', isDir: false },
    { name: 'app_config.json', size: 4120, modified: 'Feb 24, 14:00', isDir: false },
    { name: 'docker-compose.prod.yml', size: 1890, modified: 'Feb 22, 11:20', isDir: false },
    { name: 'certificates', size: 4096, modified: 'Feb 20, 09:30', isDir: true },
    { name: 'kubernetes-manifests', size: 4096, modified: 'Feb 18, 16:10', isDir: true },
  ];

  // Remote files for current directory
  const currentRemoteFiles = sftpFiles[currentRemotePath] || INITIAL_SFTP_FILES[currentRemotePath] || [];
  const filteredRemoteFiles = currentRemoteFiles
    .filter((f) => (showHidden ? true : !f.name.startsWith('.')))
    .filter((f) =>
      searchFilter ? f.name.toLowerCase().includes(searchFilter.toLowerCase()) : true
    );

  const handleNavigateRemote = (targetDir: string, shouldSyncTab: boolean = true) => {
    let resolved = targetDir;
    if (targetDir === '..') {
      if (currentRemotePath === '/' || currentRemotePath === '') {
        resolved = '/';
      } else {
        const segs = currentRemotePath.split('/').filter(Boolean);
        segs.pop();
        resolved = '/' + segs.join('/');
        if (resolved === '') resolved = '/';
      }
    }

    setRemotePathInput(resolved);
    setSelectedRemoteFile(null);

    if (shouldSyncTab) {
      onUpdateTabState(activeTab.id, {
        sftpRemotePath: resolved,
      });
    }
  };

  const handleSimulateTransfer = (
    direction: 'upload' | 'download',
    fileName: string,
    size: number
  ) => {
    const newTxId = `tx-${Date.now()}-${Math.random().toString(36).substring(7)}`;
    const newTx: SFTPTransferItem = {
      id: newTxId,
      filename: fileName,
      direction,
      size,
      transferred: 0,
      speed: '24.2 MB/s',
      status: 'transferring',
      progress: 15,
      hostName: activeHost.name,
    };

    setTransfers((prev) => [newTx, ...prev]);
    setShowTransferQueue(true);

    // Simulated animated progress
    let p = 15;
    const interval = setInterval(() => {
      p += 30;
      if (p >= 100) {
        clearInterval(interval);
        setTransfers((prev) =>
          prev.map((t) =>
            t.id === newTxId
              ? { ...t, progress: 100, transferred: size, status: 'completed' }
              : t
          )
        );

        if (direction === 'upload') {
          const fullPath = `${currentRemotePath === '/' ? '' : currentRemotePath}/${fileName}`;
          const newRemoteFile: SFTPFile = {
            name: fileName,
            path: fullPath,
            isDir: false,
            size: size || 1024,
            permissions: '-rw-r--r--',
            modified: 'Just now',
            owner: activeHost.username,
            group: 'root',
            content: `# Uploaded via WinSSH Pro SFTP\n# Path: ${fullPath}\n# Host: ${activeHost.host}\n# Timestamp: ${new Date().toISOString()}\n`,
          };

          const existingList = sftpFiles[currentRemotePath] || INITIAL_SFTP_FILES[currentRemotePath] || [];
          const updatedList = [
            ...existingList.filter((f) => f.name !== fileName),
            newRemoteFile,
          ];

          onUpdateSftpFiles({
            ...sftpFiles,
            [currentRemotePath]: updatedList,
          });
        }
      } else {
        setTransfers((prev) =>
          prev.map((t) =>
            t.id === newTxId
              ? { ...t, progress: p, transferred: Math.floor((size * p) / 100) }
              : t
          )
        );
      }
    }, 350);
  };

  const handleCreateNewFile = () => {
    if (!newFileName.trim()) return;
    const fileName = newFileName.trim();
    const fullPath = `${currentRemotePath === '/' ? '' : currentRemotePath}/${fileName}`;
    const newFile: SFTPFile = {
      name: fileName,
      path: fullPath,
      isDir: false,
      size: 0,
      permissions: '-rw-r--r--',
      modified: 'Just now',
      owner: activeHost.username,
      group: 'root',
      content: `# New file created in ${activeHost.name}\n# Path: ${fullPath}\n\n`,
    };

    const existingList = sftpFiles[currentRemotePath] || INITIAL_SFTP_FILES[currentRemotePath] || [];
    onUpdateSftpFiles({
      ...sftpFiles,
      [currentRemotePath]: [...existingList.filter((f) => f.name !== fileName), newFile],
    });

    setShowNewFileModal(false);
    setNewFileName('');
    onOpenFileEditor({
      path: fullPath,
      name: newFile.name,
      content: newFile.content || '',
    });
  };

  const handleCreateNewFolder = () => {
    if (!newFolderName.trim()) return;
    const folderName = newFolderName.trim();
    const fullPath = `${currentRemotePath === '/' ? '' : currentRemotePath}/${folderName}`;
    const newFolder: SFTPFile = {
      name: folderName,
      path: fullPath,
      isDir: true,
      size: 4096,
      permissions: 'drwxr-xr-x',
      modified: 'Just now',
      owner: activeHost.username,
      group: 'root',
    };

    const existingList = sftpFiles[currentRemotePath] || INITIAL_SFTP_FILES[currentRemotePath] || [];
    onUpdateSftpFiles({
      ...sftpFiles,
      [currentRemotePath]: [...existingList.filter((f) => f.name !== folderName), newFolder],
      [fullPath]: [],
    });

    setShowNewFolderModal(false);
    setNewFolderName('');
  };

  const handleDeleteRemoteFile = (file: SFTPFile) => {
    const existingList = sftpFiles[currentRemotePath] || INITIAL_SFTP_FILES[currentRemotePath] || [];
    const updated = existingList.filter((f) => f.name !== file.name);
    onUpdateSftpFiles({
      ...sftpFiles,
      [currentRemotePath]: updated,
    });
    if (selectedRemoteFile?.name === file.name) {
      setSelectedRemoteFile(null);
    }
  };

  const handleApplyChmod = () => {
    if (!fileToChmod) return;
    const perms = chmodValue.trim() === '777' ? '-rwxrwxrwx' : chmodValue.trim() === '755' ? '-rwxr-xr-x' : '-rw-r--r--';
    const existingList = sftpFiles[currentRemotePath] || INITIAL_SFTP_FILES[currentRemotePath] || [];
    const updated = existingList.map((f) =>
      f.name === fileToChmod.name ? { ...f, permissions: perms } : f
    );
    onUpdateSftpFiles({
      ...sftpFiles,
      [currentRemotePath]: updated,
    });
    setShowChmodModal(false);
    setFileToChmod(null);
  };

  const handleApplyRename = () => {
    if (!fileToRename || !renameValue.trim()) return;
    const newName = renameValue.trim();
    const existingList = sftpFiles[currentRemotePath] || INITIAL_SFTP_FILES[currentRemotePath] || [];
    const updated = existingList.map((f) =>
      f.name === fileToRename.name
        ? {
            ...f,
            name: newName,
            path: `${currentRemotePath === '/' ? '' : currentRemotePath}/${newName}`,
          }
        : f
    );
    onUpdateSftpFiles({
      ...sftpFiles,
      [currentRemotePath]: updated,
    });
    setFileToRename(null);
    setRenameValue('');
  };

  const handleCopyPath = (file: SFTPFile) => {
    const fullPath = file.path || `${currentRemotePath}/${file.name}`;
    navigator.clipboard.writeText(fullPath);
    setCopiedNotification(file.name);
    setTimeout(() => setCopiedNotification(null), 2000);
  };

  const handleCdInTerminal = (path: string) => {
    if (onExecuteCommandInTerminal) {
      onExecuteCommandInTerminal(`cd ${path}`);
    }
  };

  const formatFileSize = (bytes: number) => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
  };

  const getFileIcon = (file: SFTPFile) => {
    if (file.isDir) {
      return <Folder className="w-3.5 h-3.5 text-[#CE9178] shrink-0" />;
    }
    const ext = file.name.split('.').pop()?.toLowerCase();
    if (ext === 'sh' || ext === 'bash' || ext === 'zsh') {
      return <FileCode className="w-3.5 h-3.5 text-[#4EC9B0] shrink-0" />;
    }
    if (ext === 'js' || ext === 'ts' || ext === 'jsx' || ext === 'tsx' || ext === 'py' || ext === 'go') {
      return <FileCode className="w-3.5 h-3.5 text-[#569CD6] shrink-0" />;
    }
    if (ext === 'yml' || ext === 'yaml' || ext === 'json' || ext === 'conf' || ext === 'toml' || ext === 'env') {
      return <FileText className="w-3.5 h-3.5 text-[#DCDCAA] shrink-0" />;
    }
    if (ext === 'tar' || ext === 'gz' || ext === 'zip' || ext === 'bz2' || ext === '7z') {
      return <FileArchive className="w-3.5 h-3.5 text-[#C586C0] shrink-0" />;
    }
    return <File className="w-3.5 h-3.5 text-[#9CDCFE] shrink-0" />;
  };

  // Breadcrumbs calculation
  const pathSegments = currentRemotePath.split('/').filter(Boolean);

  return (
    <div
      id="session-bottom-sftp-panel"
      style={{ height: `${panelHeight}px` }}
      className="border-t border-[#007ACC]/40 bg-[#181818] flex flex-col overflow-hidden relative select-none font-sans text-xs shadow-2xl z-20 transition-colors"
    >
      {/* Top Resize Drag Handle */}
      <div
        onMouseDown={(e) => {
          setIsDraggingResize(true);
          startYRef.current = e.clientY;
          startHeightRef.current = panelHeight;
        }}
        className="h-1.5 w-full bg-[#252526] hover:bg-[#007ACC] cursor-ns-resize flex items-center justify-center group transition-colors select-none"
        title="Drag up/down to resize SFTP Panel"
      >
        <div className="w-12 h-0.5 rounded-full bg-[#454545] group-hover:bg-white transition-colors" />
      </div>

      {/* Main SFTP Top Navigation Bar */}
      <div className="h-8 bg-[#252526] border-b border-[#333333] px-2 flex items-center justify-between gap-2 shrink-0">
        {/* Left: Host Badge & Follow Terminal Button */}
        <div className="flex items-center gap-1.5 min-w-0">
          <div className="flex items-center gap-1.5 px-2 py-0.5 rounded bg-[#1E1E1E] border border-[#333333] font-mono text-[11px] text-[#D4D4D4] shrink-0">
            <span
              className="w-2 h-2 rounded-full shrink-0 animate-pulse"
              style={{ backgroundColor: activeHost.color || '#27C93F' }}
            />
            <span className="font-semibold text-[#4EC9B0]">{activeHost.name}</span>
            <span className="text-[#6D6D6D]">SFTP</span>
          </div>

          {/* Sync / Follow Terminal CWD button */}
          <button
            onClick={() => {
              const next = !followTerminalCwd;
              setFollowTerminalCwd(next);
              onUpdateTabState(activeTab.id, { sftpFollowTerminalCwd: next });
              if (next && terminalCwd) {
                handleNavigateRemote(terminalCwd);
              }
            }}
            title={
              followTerminalCwd
                ? 'Follow Terminal CWD is ENABLED (Auto-syncs with cd commands)'
                : 'Click to enable auto-follow Terminal CWD'
            }
            className={`flex items-center gap-1 px-2 py-0.5 rounded text-[11px] transition-all border shrink-0 ${
              followTerminalCwd
                ? 'bg-[#007ACC]/20 text-[#4EC9B0] border-[#007ACC]/60 font-medium shadow-xs'
                : 'bg-[#1E1E1E] text-[#858585] hover:text-[#D4D4D4] border-[#333333]'
            }`}
          >
            <FolderSync className={`w-3 h-3 ${followTerminalCwd ? 'text-[#4EC9B0] animate-spin-slow' : ''}`} />
            <span className="hidden sm:inline">
              {followTerminalCwd ? 'Sync CWD: ON' : 'Sync CWD: OFF'}
            </span>
          </button>

          {/* Jump to Terminal CWD */}
          {terminalCwd && terminalCwd !== currentRemotePath && (
            <button
              onClick={() => handleNavigateRemote(terminalCwd)}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[10px] text-[#DCDCAA] border border-[#454545] transition-colors shrink-0"
              title={`Jump to Terminal CWD: ${terminalCwd}`}
            >
              <TerminalIcon className="w-2.5 h-2.5 text-[#4EC9B0]" />
              <span className="truncate max-w-[110px] font-mono">cd {terminalCwd}</span>
            </button>
          )}
        </div>

        {/* Center: Remote Path Bar & Quick Jump */}
        <div className="flex-1 flex items-center gap-1 min-w-[200px] max-w-xl">
          <button
            onClick={() => handleNavigateRemote('..')}
            disabled={currentRemotePath === '/' || currentRemotePath === ''}
            title="Go to Parent Directory"
            className="p-1 rounded bg-[#1E1E1E] hover:bg-[#3C3C3C] disabled:opacity-30 text-[#CCCCCC] border border-[#333333] transition-colors"
          >
            <ChevronUp className="w-3.5 h-3.5" />
          </button>

          <button
            onClick={() => handleNavigateRemote('/root')}
            title="Go to /root"
            className="p-1 rounded bg-[#1E1E1E] hover:bg-[#3C3C3C] text-[#CCCCCC] border border-[#333333] transition-colors"
          >
            <Home className="w-3.5 h-3.5 text-[#569CD6]" />
          </button>

          {/* Editable Path Input */}
          <div className="flex-1 flex items-center bg-[#1E1E1E] border border-[#3E3E42] rounded px-2 py-0.5 text-[11px] font-mono text-[#D4D4D4] focus-within:border-[#007ACC] transition-colors">
            <span className="text-[#858585] mr-1">/</span>
            <input
              type="text"
              value={remotePathInput}
              onChange={(e) => setRemotePathInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleNavigateRemote(remotePathInput);
              }}
              className="w-full bg-transparent focus:outline-none text-[#9CDCFE] selection:bg-[#007ACC]/50"
              placeholder="/root or /var/www"
              spellCheck={false}
            />
          </div>

          <button
            onClick={() => handleNavigateRemote(remotePathInput)}
            className="px-2 py-0.5 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-[11px] font-medium transition-colors"
          >
            Go
          </button>

          <button
            onClick={() => handleNavigateRemote(currentRemotePath)}
            title="Refresh Remote Directory"
            className="p-1 rounded bg-[#1E1E1E] hover:bg-[#3C3C3C] text-[#858585] hover:text-[#D4D4D4] border border-[#333333] transition-colors"
          >
            <RefreshCw className="w-3 h-3" />
          </button>
        </div>

        {/* Right: Actions, View Mode Toggle, Presets, Close */}
        <div className="flex items-center gap-1.5 shrink-0">
          {/* Search Filter */}
          <div className="hidden md:flex items-center bg-[#1E1E1E] border border-[#333333] rounded px-1.5 py-0.5 text-[10px] w-28">
            <Search className="w-3 h-3 text-[#858585] mr-1" />
            <input
              type="text"
              placeholder="Filter..."
              value={searchFilter}
              onChange={(e) => setSearchFilter(e.target.value)}
              className="w-full bg-transparent focus:outline-none text-[#D4D4D4]"
            />
            {searchFilter && (
              <button onClick={() => setSearchFilter('')} className="text-[#858585] hover:text-white">
                <X className="w-2.5 h-2.5" />
              </button>
            )}
          </div>

          {/* New File & New Folder Buttons */}
          <button
            onClick={() => setShowNewFileModal(true)}
            className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#D4D4D4] border border-[#454545] transition-colors text-[11px]"
            title="Create New File"
          >
            <Plus className="w-3 h-3 text-[#4EC9B0]" />
            <span className="hidden lg:inline">File</span>
          </button>

          <button
            onClick={() => setShowNewFolderModal(true)}
            className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#D4D4D4] border border-[#454545] transition-colors text-[11px]"
            title="Create New Folder"
          >
            <FolderPlus className="w-3 h-3 text-[#CE9178]" />
            <span className="hidden lg:inline">Folder</span>
          </button>

          {/* Toggle Dual/Single Pane */}
          <button
            onClick={() => {
              const nextMode = viewMode === 'dual' ? 'remote-only' : 'dual';
              setViewMode(nextMode);
              onUpdateTabState(activeTab.id, { sftpViewMode: nextMode });
            }}
            title={viewMode === 'dual' ? 'Switch to Remote Wide View' : 'Switch to Dual Local/Remote View'}
            className={`p-1 rounded border transition-colors ${
              viewMode === 'dual'
                ? 'bg-[#37373D] text-[#4EC9B0] border-[#007ACC]'
                : 'bg-[#1E1E1E] text-[#858585] hover:text-[#D4D4D4] border-[#333333]'
            }`}
          >
            <Columns className="w-3.5 h-3.5" />
          </button>

          {/* Toggle Hidden Files */}
          <button
            onClick={() => setShowHidden(!showHidden)}
            title={showHidden ? 'Hide dotfiles' : 'Show hidden dotfiles'}
            className={`p-1 rounded border transition-colors ${
              showHidden
                ? 'bg-[#37373D] text-[#DCDCAA] border-[#454545]'
                : 'bg-[#1E1E1E] text-[#858585] border-[#333333]'
            }`}
          >
            {showHidden ? <Eye className="w-3.5 h-3.5" /> : <EyeOff className="w-3.5 h-3.5" />}
          </button>

          {/* Height Presets */}
          <button
            onClick={() => {
              const nextHeight = panelHeight > 350 ? 200 : 420;
              onUpdateTabState(activeTab.id, { sftpHeight: nextHeight });
            }}
            title={panelHeight > 350 ? 'Compact SFTP View' : 'Expand SFTP View'}
            className="p-1 rounded bg-[#1E1E1E] hover:bg-[#3C3C3C] text-[#858585] hover:text-[#D4D4D4] border border-[#333333] transition-colors"
          >
            {panelHeight > 350 ? (
              <Minimize2 className="w-3.5 h-3.5" />
            ) : (
              <Maximize2 className="w-3.5 h-3.5" />
            )}
          </button>

          {/* Close Panel Button */}
          <button
            onClick={onClose}
            title="Collapse SFTP Panel"
            className="p-1 rounded bg-[#1E1E1E] hover:bg-[#FF5F56]/20 text-[#858585] hover:text-[#FF5F56] border border-[#333333] transition-colors ml-1"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Main File Browser Area */}
      <div className="flex-1 flex overflow-hidden">
        {/* Left Side: Local Workstation (When in Dual View Mode) */}
        {viewMode === 'dual' && (
          <div className="w-2/5 flex flex-col border-r border-[#333333] bg-[#141414]">
            {/* Local Header */}
            <div className="h-7 bg-[#1E1E1E] border-b border-[#2D2D2D] px-2.5 flex items-center justify-between text-[11px] text-[#858585]">
              <div className="flex items-center gap-1.5">
                <Laptop className="w-3.5 h-3.5 text-[#569CD6]" />
                <span className="font-medium text-[#D4D4D4]">Local Workstation</span>
              </div>
              <span className="font-mono text-[10px] text-[#6D6D6D] truncate max-w-[160px]">
                {localPath}
              </span>
            </div>

            {/* Local File Table */}
            <div className="flex-1 overflow-y-auto p-1 font-mono text-[11px]">
              <table className="w-full text-left text-[#D4D4D4]">
                <thead className="text-[10px] uppercase text-[#6D6D6D] border-b border-[#2D2D2D]">
                  <tr>
                    <th className="pb-1 pl-1 font-semibold">Name</th>
                    <th className="pb-1 w-20 font-semibold">Size</th>
                    <th className="pb-1 w-24 font-semibold">Modified</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-[#1E1E1E]">
                  {localFiles.map((file) => {
                    const isSelected = selectedLocalFile === file.name;
                    return (
                      <tr
                        key={file.name}
                        onClick={() => setSelectedLocalFile(file.name)}
                        onDoubleClick={() => {
                          if (!file.isDir) {
                            handleSimulateTransfer('upload', file.name, file.size);
                          }
                        }}
                        className={`hover:bg-[#2A2D2E] cursor-pointer transition-colors ${
                          isSelected ? 'bg-[#007ACC]/25 text-[#4EC9B0]' : ''
                        }`}
                      >
                        <td className="py-1 pl-1 flex items-center gap-1.5 font-sans text-xs">
                          {file.isDir ? (
                            <Folder className="w-3.5 h-3.5 text-[#CE9178] shrink-0" />
                          ) : (
                            <File className="w-3.5 h-3.5 text-[#9CDCFE] shrink-0" />
                          )}
                          <span className="truncate max-w-[140px]">{file.name}</span>
                        </td>
                        <td className="py-1 text-[#858585]">
                          {file.isDir ? '-' : formatFileSize(file.size)}
                        </td>
                        <td className="py-1 text-[#6D6D6D]">{file.modified}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>

            {/* Local Footer Bar with Upload button */}
            <div className="h-7 bg-[#1E1E1E] border-t border-[#2D2D2D] px-2 flex items-center justify-between text-[11px]">
              <span className="text-[#6D6D6D] text-[10px] font-mono">
                {localFiles.length} items
              </span>
              <button
                disabled={!selectedLocalFile}
                onClick={() => {
                  const target = localFiles.find((f) => f.name === selectedLocalFile);
                  if (target && !target.isDir) {
                    handleSimulateTransfer('upload', target.name, target.size);
                  }
                }}
                className="flex items-center gap-1 px-2.5 py-0.5 bg-[#007ACC] hover:bg-[#0062a3] disabled:opacity-40 text-white rounded text-[11px] font-medium transition-colors"
              >
                <span>Upload to Remote</span>
                <ArrowRight className="w-3 h-3" />
              </button>
            </div>
          </div>
        )}

        {/* Right Side: Remote SSH Server File System */}
        <div
          onDragOver={(e) => {
            e.preventDefault();
            setIsDragOver(true);
          }}
          onDragLeave={() => setIsDragOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setIsDragOver(false);
            if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {
              const file = e.dataTransfer.files[0];
              handleSimulateTransfer('upload', file.name, file.size);
            }
          }}
          className={`flex-1 flex flex-col bg-[#141414] overflow-hidden relative ${
            isDragOver ? 'ring-2 ring-inset ring-[#007ACC] bg-[#007ACC]/5' : ''
          }`}
        >
          {/* Breadcrumbs Sub-Header */}
          <div className="h-7 bg-[#1E1E1E] border-b border-[#2D2D2D] px-2.5 flex items-center justify-between text-[11px] text-[#858585]">
            <div className="flex items-center gap-1 overflow-x-auto scrollbar-none font-mono">
              <Server className="w-3.5 h-3.5 text-[#27C93F] shrink-0 mr-1" />
              <button
                onClick={() => handleNavigateRemote('/')}
                className="hover:text-[#4EC9B0] transition-colors"
              >
                /
              </button>
              {pathSegments.map((seg, idx) => {
                const subPath = '/' + pathSegments.slice(0, idx + 1).join('/');
                const isLast = idx === pathSegments.length - 1;
                return (
                  <React.Fragment key={subPath}>
                    <ChevronRight className="w-3 h-3 text-[#6D6D6D] shrink-0" />
                    <button
                      onClick={() => handleNavigateRemote(subPath)}
                      className={`hover:text-[#4EC9B0] transition-colors ${
                        isLast ? 'text-[#D4D4D4] font-semibold' : 'text-[#858585]'
                      }`}
                    >
                      {seg}
                    </button>
                  </React.Fragment>
                );
              })}
            </div>

            <div className="flex items-center gap-2 text-[10px] text-[#6D6D6D] font-mono">
              <span>{filteredRemoteFiles.length} items</span>
              {copiedNotification && (
                <span className="text-[#4EC9B0] bg-[#4EC9B0]/20 px-1.5 py-0.2 rounded font-sans">
                  Copied path!
                </span>
              )}
            </div>
          </div>

          {/* Remote Files Table */}
          <div className="flex-1 overflow-y-auto p-1 font-mono text-[11px]">
            <table className="w-full text-left text-[#D4D4D4]">
              <thead className="text-[10px] uppercase text-[#6D6D6D] border-b border-[#2D2D2D]">
                <tr>
                  <th className="pb-1 pl-1 font-semibold">Name</th>
                  <th className="pb-1 w-20 font-semibold">Size</th>
                  <th className="pb-1 w-24 font-semibold">Perms</th>
                  <th className="pb-1 w-20 font-semibold hidden sm:table-cell">Owner</th>
                  <th className="pb-1 w-24 font-semibold">Modified</th>
                  <th className="pb-1 w-24 text-right pr-1 font-semibold">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-[#1E1E1E]">
                {/* Parent Directory Link */}
                {currentRemotePath !== '/' && (
                  <tr
                    onClick={() => handleNavigateRemote('..')}
                    className="hover:bg-[#2A2D2E] cursor-pointer text-[#858585] transition-colors"
                  >
                    <td className="py-1 pl-1 flex items-center gap-1.5 font-sans text-xs">
                      <Folder className="w-3.5 h-3.5 text-[#6D6D6D]" />
                      <span>.. [Parent Directory]</span>
                    </td>
                    <td className="py-1">-</td>
                    <td className="py-1">drwxr-xr-x</td>
                    <td className="py-1 hidden sm:table-cell">root</td>
                    <td className="py-1">-</td>
                    <td className="py-1 text-right pr-1"></td>
                  </tr>
                )}

                {filteredRemoteFiles.map((file) => {
                  const isSelected = selectedRemoteFile?.name === file.name;
                  return (
                    <tr
                      key={file.name}
                      onClick={() => setSelectedRemoteFile(file)}
                      onDoubleClick={() => {
                        if (file.isDir) {
                          handleNavigateRemote(
                            currentRemotePath === '/'
                              ? `/${file.name}`
                              : `${currentRemotePath}/${file.name}`
                          );
                        } else {
                          onOpenFileEditor({
                            path: file.path || `${currentRemotePath}/${file.name}`,
                            name: file.name,
                            content: file.content || `# ${file.name}\n`,
                          });
                        }
                      }}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        setSelectedRemoteFile(file);
                        setContextMenuFile({ file, x: e.clientX, y: e.clientY });
                      }}
                      className={`hover:bg-[#2A2D2E] cursor-pointer transition-colors group ${
                        isSelected ? 'bg-[#007ACC]/25 text-[#4EC9B0]' : ''
                      }`}
                    >
                      <td className="py-1 pl-1 flex items-center gap-1.5 font-sans text-xs">
                        {getFileIcon(file)}
                        <span className="truncate max-w-[180px] sm:max-w-xs">{file.name}</span>
                      </td>
                      <td className="py-1 text-[#858585]">
                        {file.isDir ? '-' : formatFileSize(file.size)}
                      </td>
                      <td className="py-1 text-[#6D6D6D] text-[10px]">{file.permissions}</td>
                      <td className="py-1 text-[#6D6D6D] hidden sm:table-cell">
                        {file.owner}:{file.group}
                      </td>
                      <td className="py-1 text-[#6D6D6D] text-[10px]">{file.modified}</td>
                      <td className="py-1 text-right pr-1">
                        <div className="flex items-center justify-end gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                          {!file.isDir && (
                            <button
                              onClick={(e) => {
                                e.stopPropagation();
                                handleSimulateTransfer('download', file.name, file.size);
                              }}
                              title="Download file"
                              className="p-1 hover:text-[#4EC9B0] hover:bg-[#3C3C3C] rounded transition-colors"
                            >
                              <Download className="w-3 h-3 text-[#DCDCAA]" />
                            </button>
                          )}
                          {!file.isDir && (
                            <button
                              onClick={(e) => {
                                e.stopPropagation();
                                onOpenFileEditor({
                                  path: file.path || `${currentRemotePath}/${file.name}`,
                                  name: file.name,
                                  content: file.content || '',
                                });
                              }}
                              title="Edit in Code Editor"
                              className="p-1 hover:text-[#569CD6] hover:bg-[#3C3C3C] rounded transition-colors"
                            >
                              <Edit2 className="w-3 h-3 text-[#569CD6]" />
                            </button>
                          )}
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              handleDeleteRemoteFile(file);
                            }}
                            title="Delete"
                            className="p-1 hover:text-[#FF5F56] hover:bg-[#FF5F56]/20 rounded transition-colors"
                          >
                            <Trash2 className="w-3 h-3 text-[#858585]" />
                          </button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          {/* Remote Bottom Action & Selection Bar */}
          <div className="h-7 bg-[#1E1E1E] border-t border-[#2D2D2D] px-2 flex items-center justify-between text-[11px]">
            <div className="flex items-center gap-1.5">
              {/* Selected File Quick Actions */}
              {selectedRemoteFile ? (
                <>
                  <span className="text-[#D4D4D4] font-medium truncate max-w-[120px] font-mono">
                    {selectedRemoteFile.name}
                  </span>
                  {!selectedRemoteFile.isDir && (
                    <button
                      onClick={() =>
                        handleSimulateTransfer(
                          'download',
                          selectedRemoteFile.name,
                          selectedRemoteFile.size
                        )
                      }
                      className="flex items-center gap-1 px-2 py-0.5 bg-[#DCDCAA]/20 text-[#DCDCAA] hover:bg-[#DCDCAA]/30 rounded text-[10px] font-medium"
                    >
                      <Download className="w-2.5 h-2.5" />
                      <span>Download</span>
                    </button>
                  )}
                  {!selectedRemoteFile.isDir && (
                    <button
                      onClick={() =>
                        onOpenFileEditor({
                          path: selectedRemoteFile.path || `${currentRemotePath}/${selectedRemoteFile.name}`,
                          name: selectedRemoteFile.name,
                          content: selectedRemoteFile.content || '',
                        })
                      }
                      className="flex items-center gap-1 px-2 py-0.5 bg-[#569CD6]/20 text-[#569CD6] hover:bg-[#569CD6]/30 rounded text-[10px] font-medium"
                    >
                      <Edit2 className="w-2.5 h-2.5" />
                      <span>Edit</span>
                    </button>
                  )}
                  {selectedRemoteFile.isDir && (
                    <button
                      onClick={() =>
                        handleCdInTerminal(
                          selectedRemoteFile.path || `${currentRemotePath}/${selectedRemoteFile.name}`
                        )
                      }
                      className="flex items-center gap-1 px-2 py-0.5 bg-[#4EC9B0]/20 text-[#4EC9B0] hover:bg-[#4EC9B0]/30 rounded text-[10px] font-medium"
                    >
                      <TerminalIcon className="w-2.5 h-2.5" />
                      <span>cd in shell</span>
                    </button>
                  )}
                  <button
                    onClick={() => handleCopyPath(selectedRemoteFile)}
                    className="flex items-center gap-1 px-1.5 py-0.5 bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#858585] hover:text-[#D4D4D4] rounded text-[10px]"
                    title="Copy full remote path"
                  >
                    <Copy className="w-2.5 h-2.5" />
                  </button>
                </>
              ) : (
                <span className="text-[#6D6D6D] text-[10px] font-mono">
                  Double click file to open/edit, or drag-drop file here to upload
                </span>
              )}
            </div>

            {/* Transfer Queue Trigger */}
            <div className="flex items-center gap-2 font-mono text-[10px]">
              <button
                onClick={() => setShowTransferQueue(!showTransferQueue)}
                className={`flex items-center gap-1.5 px-2 py-0.5 rounded transition-colors ${
                  transfers.some((t) => t.status === 'transferring')
                    ? 'bg-[#007ACC] text-white animate-pulse'
                    : 'bg-[#252526] hover:bg-[#37373D] text-[#858585] hover:text-[#D4D4D4] border border-[#333333]'
                }`}
              >
                <FolderSync className="w-3 h-3 text-[#4EC9B0]" />
                <span>Transfers ({transfers.length})</span>
                {showTransferQueue ? (
                  <ChevronDown className="w-2.5 h-2.5" />
                ) : (
                  <ChevronUp className="w-2.5 h-2.5" />
                )}
              </button>
            </div>
          </div>
        </div>
      </div>

      {/* Expandable Bottom Transfer Queue Drawer */}
      {showTransferQueue && (
        <div className="h-20 bg-[#1A1A1A] border-t border-[#333333] flex flex-col shrink-0">
          <div className="h-5 bg-[#252526] px-2 flex items-center justify-between text-[10px] text-[#858585]">
            <span>Active Transfers ({transfers.length})</span>
            <div className="flex items-center gap-2">
              <button
                onClick={() => setTransfers([])}
                className="hover:text-[#D4D4D4] transition-colors"
              >
                Clear All
              </button>
              <button
                onClick={() => setShowTransferQueue(false)}
                className="hover:text-white"
              >
                <X className="w-2.5 h-2.5" />
              </button>
            </div>
          </div>

          <div className="flex-1 overflow-y-auto p-1.5 space-y-1 font-mono text-[10px]">
            {transfers.length === 0 ? (
              <div className="text-[#6D6D6D] text-center py-2">No active transfers in queue</div>
            ) : (
              transfers.map((tx) => (
                <div
                  key={tx.id}
                  className="flex items-center justify-between p-1 bg-[#141414] border border-[#2D2D2D] rounded"
                >
                  <div className="flex items-center gap-1.5 min-w-0">
                    {tx.direction === 'upload' ? (
                      <Upload className="w-3 h-3 text-[#4EC9B0] shrink-0" />
                    ) : (
                      <Download className="w-3 h-3 text-[#DCDCAA] shrink-0" />
                    )}
                    <span className="text-[#D4D4D4] truncate max-w-[180px]">{tx.filename}</span>
                    <span className="text-[#6D6D6D]">({formatFileSize(tx.size)})</span>
                  </div>

                  <div className="flex items-center gap-2">
                    <div className="w-24 bg-[#2D2D2D] rounded-full h-1.5 overflow-hidden">
                      <div
                        className={`h-full transition-all duration-300 ${
                          tx.status === 'completed' ? 'bg-[#27C93F]' : 'bg-[#007ACC]'
                        }`}
                        style={{ width: `${tx.progress}%` }}
                      />
                    </div>
                    <span className="text-[#858585] w-14 text-right">{tx.speed}</span>
                    <span
                      className={`text-[9px] px-1 py-0.2 rounded font-sans uppercase ${
                        tx.status === 'completed'
                          ? 'bg-[#27C93F]/20 text-[#27C93F]'
                          : 'bg-[#007ACC]/20 text-[#007ACC]'
                      }`}
                    >
                      {tx.status}
                    </span>
                  </div>
                </div>
              ))
            )}
          </div>
        </div>
      )}

      {/* Right Click Context Menu on Remote Files */}
      {contextMenuFile && (
        <div
          style={{
            top: `${Math.min(window.innerHeight - 240, contextMenuFile.y - 120)}px`,
            left: `${Math.min(window.innerWidth - 220, contextMenuFile.x)}px`,
          }}
          className="fixed bg-[#252526] border border-[#454545] rounded-md shadow-2xl py-1 z-50 text-xs font-sans text-[#D4D4D4] w-48 select-none"
          onClick={(e) => e.stopPropagation()}
        >
          <div className="px-3 py-1 font-semibold text-[11px] text-[#4EC9B0] border-b border-[#333333] truncate">
            {contextMenuFile.file.name}
          </div>

          {!contextMenuFile.file.isDir && (
            <button
              onClick={() => {
                onOpenFileEditor({
                  path: contextMenuFile.file.path || `${currentRemotePath}/${contextMenuFile.file.name}`,
                  name: contextMenuFile.file.name,
                  content: contextMenuFile.file.content || '',
                });
                setContextMenuFile(null);
              }}
              className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
            >
              <Edit2 className="w-3.5 h-3.5 text-[#569CD6]" />
              <span>Edit File</span>
            </button>
          )}

          {!contextMenuFile.file.isDir && (
            <button
              onClick={() => {
                handleSimulateTransfer('download', contextMenuFile.file.name, contextMenuFile.file.size);
                setContextMenuFile(null);
              }}
              className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
            >
              <Download className="w-3.5 h-3.5 text-[#DCDCAA]" />
              <span>Download File</span>
            </button>
          )}

          {contextMenuFile.file.isDir && (
            <button
              onClick={() => {
                handleNavigateRemote(
                  currentRemotePath === '/'
                    ? `/${contextMenuFile.file.name}`
                    : `${currentRemotePath}/${contextMenuFile.file.name}`
                );
                setContextMenuFile(null);
              }}
              className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
            >
              <Folder className="w-3.5 h-3.5 text-[#CE9178]" />
              <span>Open Folder</span>
            </button>
          )}

          <button
            onClick={() => {
              handleCdInTerminal(
                contextMenuFile.file.isDir
                  ? contextMenuFile.file.path || `${currentRemotePath}/${contextMenuFile.file.name}`
                  : currentRemotePath
              );
              setContextMenuFile(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
          >
            <TerminalIcon className="w-3.5 h-3.5 text-[#4EC9B0]" />
            <span>cd here in Terminal</span>
          </button>

          <button
            onClick={() => {
              handleCopyPath(contextMenuFile.file);
              setContextMenuFile(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
          >
            <Copy className="w-3.5 h-3.5 text-[#858585]" />
            <span>Copy Remote Path</span>
          </button>

          <button
            onClick={() => {
              setFileToRename(contextMenuFile.file);
              setRenameValue(contextMenuFile.file.name);
              setContextMenuFile(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
          >
            <ArrowUpDown className="w-3.5 h-3.5 text-[#C586C0]" />
            <span>Rename</span>
          </button>

          <button
            onClick={() => {
              setFileToChmod(contextMenuFile.file);
              setChmodValue(contextMenuFile.file.permissions.includes('x') ? '755' : '644');
              setShowChmodModal(true);
              setContextMenuFile(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
          >
            <Shield className="w-3.5 h-3.5 text-[#DCDCAA]" />
            <span>Permissions (chmod)</span>
          </button>

          <hr className="my-1 border-[#333333]" />

          <button
            onClick={() => {
              handleDeleteRemoteFile(contextMenuFile.file);
              setContextMenuFile(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#FF5F56]/20 text-[#FF5F56] text-left"
          >
            <Trash2 className="w-3.5 h-3.5" />
            <span>Delete</span>
          </button>
        </div>
      )}

      {/* Modal: Create New File */}
      {showNewFileModal && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-xs z-50 flex items-center justify-center p-4">
          <div className="bg-[#252526] border border-[#454545] rounded-xl max-w-sm w-full p-4 space-y-3 shadow-2xl font-sans">
            <h4 className="text-sm font-semibold text-white flex items-center gap-2">
              <FileCode className="w-4 h-4 text-[#4EC9B0]" />
              <span>Create New Remote File</span>
            </h4>
            <div className="space-y-1">
              <label className="text-[11px] text-[#858585]">
                File Name in <span className="font-mono text-[#D4D4D4]">{currentRemotePath}</span>
              </label>
              <input
                type="text"
                placeholder="e.g. app.config.json or script.sh"
                value={newFileName}
                onChange={(e) => setNewFileName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') handleCreateNewFile();
                }}
                autoFocus
                className="w-full px-3 py-1.5 bg-[#1E1E1E] border border-[#454545] rounded text-xs text-white focus:outline-none focus:border-[#007ACC]"
              />
            </div>
            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={() => setShowNewFileModal(false)}
                className="px-3 py-1 text-xs text-[#858585] hover:text-white"
              >
                Cancel
              </button>
              <button
                onClick={handleCreateNewFile}
                className="px-3.5 py-1 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs font-medium"
              >
                Create & Edit
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Modal: Create New Folder */}
      {showNewFolderModal && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-xs z-50 flex items-center justify-center p-4">
          <div className="bg-[#252526] border border-[#454545] rounded-xl max-w-sm w-full p-4 space-y-3 shadow-2xl font-sans">
            <h4 className="text-sm font-semibold text-white flex items-center gap-2">
              <FolderPlus className="w-4 h-4 text-[#CE9178]" />
              <span>Create New Remote Directory</span>
            </h4>
            <div className="space-y-1">
              <label className="text-[11px] text-[#858585]">
                Directory Name in <span className="font-mono text-[#D4D4D4]">{currentRemotePath}</span>
              </label>
              <input
                type="text"
                placeholder="e.g. backups or releases"
                value={newFolderName}
                onChange={(e) => setNewFolderName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') handleCreateNewFolder();
                }}
                autoFocus
                className="w-full px-3 py-1.5 bg-[#1E1E1E] border border-[#454545] rounded text-xs text-white focus:outline-none focus:border-[#007ACC]"
              />
            </div>
            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={() => setShowNewFolderModal(false)}
                className="px-3 py-1 text-xs text-[#858585] hover:text-white"
              >
                Cancel
              </button>
              <button
                onClick={handleCreateNewFolder}
                className="px-3.5 py-1 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs font-medium"
              >
                Create Folder
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Modal: Chmod Permissions */}
      {showChmodModal && fileToChmod && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-xs z-50 flex items-center justify-center p-4">
          <div className="bg-[#252526] border border-[#454545] rounded-xl max-w-sm w-full p-4 space-y-3 shadow-2xl font-sans">
            <h4 className="text-sm font-semibold text-white flex items-center gap-2">
              <Shield className="w-4 h-4 text-[#DCDCAA]" />
              <span>Change Permissions (chmod)</span>
            </h4>
            <p className="text-xs text-[#858585]">
              Target: <span className="font-mono text-[#D4D4D4]">{fileToChmod.name}</span>
            </p>
            <div className="grid grid-cols-3 gap-2">
              {['644', '755', '777'].map((val) => (
                <button
                  key={val}
                  onClick={() => setChmodValue(val)}
                  className={`py-1.5 px-2 rounded border text-xs font-mono font-medium transition-colors ${
                    chmodValue === val
                      ? 'bg-[#007ACC] text-white border-[#007ACC]'
                      : 'bg-[#1E1E1E] text-[#858585] hover:text-white border-[#333333]'
                  }`}
                >
                  {val}
                </button>
              ))}
            </div>
            <input
              type="text"
              value={chmodValue}
              onChange={(e) => setChmodValue(e.target.value)}
              className="w-full px-3 py-1.5 bg-[#1E1E1E] border border-[#454545] rounded text-xs font-mono text-white focus:outline-none focus:border-[#007ACC]"
            />
            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={() => {
                  setShowChmodModal(false);
                  setFileToChmod(null);
                }}
                className="px-3 py-1 text-xs text-[#858585] hover:text-white"
              >
                Cancel
              </button>
              <button
                onClick={handleApplyChmod}
                className="px-3.5 py-1 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs font-medium"
              >
                Apply (chmod {chmodValue})
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Modal: Rename File */}
      {fileToRename && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-xs z-50 flex items-center justify-center p-4">
          <div className="bg-[#252526] border border-[#454545] rounded-xl max-w-sm w-full p-4 space-y-3 shadow-2xl font-sans">
            <h4 className="text-sm font-semibold text-white flex items-center gap-2">
              <ArrowUpDown className="w-4 h-4 text-[#C586C0]" />
              <span>Rename Item</span>
            </h4>
            <input
              type="text"
              value={renameValue}
              onChange={(e) => setRenameValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleApplyRename();
              }}
              autoFocus
              className="w-full px-3 py-1.5 bg-[#1E1E1E] border border-[#454545] rounded text-xs text-white focus:outline-none focus:border-[#007ACC]"
            />
            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={() => setFileToRename(null)}
                className="px-3 py-1 text-xs text-[#858585] hover:text-white"
              >
                Cancel
              </button>
              <button
                onClick={handleApplyRename}
                className="px-3.5 py-1 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs font-medium"
              >
                Save
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
