import React, { useState } from 'react';
import {
  FolderSync,
  Upload,
  Download,
  FileCode,
  Folder,
  File,
  FileText,
  Trash2,
  Edit2,
  RefreshCw,
  Plus,
  ArrowRight,
  ArrowLeft,
  ChevronRight,
  Home,
  Eye,
  CheckCircle2,
  Clock,
  HardDrive,
  Laptop,
  Server,
  KeyRound,
  Lock,
} from 'lucide-react';
import { SFTPFile, SFTPTransferItem, ServerHost, AppSettings } from '../types';
import { INITIAL_SFTP_FILES } from '../data/defaultData';

interface SFTPExplorerViewProps {
  host: ServerHost;
  sftpFiles: Record<string, SFTPFile[]>;
  onUpdateSftpFiles: (updated: Record<string, SFTPFile[]>) => void;
  onOpenFileEditor: (file: { path: string; name: string; content: string }) => void;
  settings: AppSettings;
}

export const SFTPExplorerView: React.FC<SFTPExplorerViewProps> = ({
  host,
  sftpFiles,
  onUpdateSftpFiles,
  onOpenFileEditor,
  settings,
}) => {
  // Remote path state
  const [remotePath, setRemotePath] = useState<string>('/root');
  const [remotePathInput, setRemotePathInput] = useState<string>('/root');
  const [selectedRemoteFile, setSelectedRemoteFile] = useState<SFTPFile | null>(null);

  // Local path state
  const [localPath, setLocalPath] = useState<string>('/Users/developer/Downloads');
  const [selectedLocalFile, setSelectedLocalFile] = useState<string | null>(null);

  // Transfer Queue state
  const [transfers, setTransfers] = useState<SFTPTransferItem[]>([
    {
      id: 'tx-1',
      filename: 'nginx.conf.bak',
      direction: 'download',
      size: 1840,
      transferred: 1840,
      speed: '12.4 MB/s',
      status: 'completed',
      progress: 100,
      hostName: host.name,
    },
  ]);

  // Modals for New File / Folder / Chmod
  const [showNewFileModal, setShowNewFileModal] = useState(false);
  const [newFileName, setNewFileName] = useState('');
  const [showChmodModal, setShowChmodModal] = useState(false);
  const [chmodValue, setChmodValue] = useState('755');

  // Simulated local directory files
  const localFiles = [
    { name: 'deploy_v2_bundle.tar.gz', size: 14820000, modified: 'Today, 10:14', isDir: false },
    { name: 'id_rsa_backup.pem', size: 3240, modified: 'Yesterday, 18:22', isDir: false },
    { name: 'app_config.json', size: 4120, modified: 'Feb 22, 14:00', isDir: false },
    { name: 'certificates', size: 4096, modified: 'Feb 20, 09:30', isDir: true },
    { name: 'kubernetes-manifests', size: 4096, modified: 'Feb 18, 16:10', isDir: true },
  ];

  const currentRemoteFiles = sftpFiles[remotePath] || INITIAL_SFTP_FILES[remotePath] || [];
  const displayRemoteFiles = currentRemoteFiles.filter((f) =>
    settings.sftpShowHidden ? true : !f.name.startsWith('.')
  );

  const handleNavigateRemote = (targetDir: string) => {
    let resolved = targetDir;
    if (targetDir === '..') {
      if (remotePath === '/' || remotePath === '') {
        resolved = '/';
      } else {
        const segs = remotePath.split('/').filter(Boolean);
        segs.pop();
        resolved = '/' + segs.join('/');
        if (resolved === '') resolved = '/';
      }
    }
    setRemotePath(resolved);
    setRemotePathInput(resolved);
    setSelectedRemoteFile(null);
  };

  const handleSimulateTransfer = (direction: 'upload' | 'download', fileName: string, size: number) => {
    const newTxId = `tx-${Date.now()}`;
    const newTx: SFTPTransferItem = {
      id: newTxId,
      filename: fileName,
      direction,
      size,
      transferred: 0,
      speed: '18.5 MB/s',
      status: 'transferring',
      progress: 10,
      hostName: host.name,
    };

    setTransfers((prev) => [newTx, ...prev]);

    // Animate progress
    let p = 10;
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

        // If upload, add file to current remote directory
        if (direction === 'upload') {
          const newRemoteFile: SFTPFile = {
            name: fileName,
            path: `${remotePath === '/' ? '' : remotePath}/${fileName}`,
            isDir: false,
            size,
            permissions: 'rw-r--r--',
            modified: 'Just now',
            owner: host.username,
            group: 'root',
            content: `# Uploaded via WinSSH Pro SFTP\n# Date: ${new Date().toISOString()}\n`,
          };
          onUpdateSftpFiles({
            ...sftpFiles,
            [remotePath]: [...(sftpFiles[remotePath] || []), newRemoteFile],
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
    }, 400);
  };

  const handleCreateNewFile = () => {
    if (!newFileName.trim()) return;
    const fullPath = `${remotePath === '/' ? '' : remotePath}/${newFileName.trim()}`;
    const newFile: SFTPFile = {
      name: newFileName.trim(),
      path: fullPath,
      isDir: false,
      size: 0,
      permissions: 'rw-r--r--',
      modified: 'Just now',
      owner: host.username,
      group: 'root',
      content: `# New file created with WinSSH Pro\n# Path: ${fullPath}\n\n`,
    };

    onUpdateSftpFiles({
      ...sftpFiles,
      [remotePath]: [...(sftpFiles[remotePath] || []), newFile],
    });

    setShowNewFileModal(false);
    setNewFileName('');
    onOpenFileEditor({
      path: fullPath,
      name: newFile.name,
      content: newFile.content || '',
    });
  };

  const handleDeleteRemoteFile = (file: SFTPFile) => {
    const updated = (sftpFiles[remotePath] || []).filter((f) => f.name !== file.name);
    onUpdateSftpFiles({
      ...sftpFiles,
      [remotePath]: updated,
    });
    setSelectedRemoteFile(null);
  };

  const formatFileSize = (bytes: number) => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
  };

  return (
    <div className="flex-1 flex flex-col bg-zinc-950 overflow-hidden font-sans select-none">
      {/* Top SFTP Control Bar */}
      <div className="h-10 bg-zinc-900 border-b border-zinc-800 px-3 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <FolderSync className="w-4 h-4 text-amber-400" />
          <span className="text-xs font-semibold text-zinc-200">
            SFTP File Transfer Engine
          </span>
          <span className="text-[11px] px-2 py-0.5 rounded bg-zinc-800 text-zinc-300 font-mono">
            {host.name} ({host.host})
          </span>
        </div>

        {/* Global Toolbar Buttons */}
        <div className="flex items-center gap-1.5 text-xs">
          <button
            onClick={() => setShowNewFileModal(true)}
            className="flex items-center gap-1 px-2.5 py-1 bg-zinc-800 hover:bg-zinc-750 text-zinc-200 rounded-md transition-colors font-medium border border-zinc-700"
          >
            <Plus className="w-3.5 h-3.5 text-cyan-400" />
            <span>New File</span>
          </button>
          <button
            onClick={() => handleNavigateRemote(remotePath)}
            className="p-1.5 bg-zinc-800 hover:bg-zinc-750 text-zinc-300 rounded-md transition-colors border border-zinc-700"
            title="Refresh Directory"
          >
            <RefreshCw className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Dual Pane Layout Container */}
      <div className="flex-1 flex overflow-hidden">
        {/* Left Pane: Local Workstation */}
        <div className="w-1/2 flex flex-col border-r border-zinc-800 bg-zinc-950">
          <div className="h-9 bg-zinc-900/70 border-b border-zinc-850 px-3 flex items-center justify-between text-xs text-zinc-400">
            <div className="flex items-center gap-2">
              <Laptop className="w-3.5 h-3.5 text-blue-400" />
              <span className="font-medium text-zinc-300">Local Workstation</span>
            </div>
            <span className="font-mono text-[11px] text-zinc-500 truncate max-w-[200px]">
              {localPath}
            </span>
          </div>

          {/* Local File Table */}
          <div className="flex-1 overflow-y-auto p-2">
            <table className="w-full text-xs text-left text-zinc-300 font-sans">
              <thead className="text-[11px] text-zinc-500 uppercase border-b border-zinc-850">
                <tr>
                  <th className="pb-2 font-medium">Name</th>
                  <th className="pb-2 font-medium w-24">Size</th>
                  <th className="pb-2 font-medium w-28">Modified</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-900 font-mono text-[11px]">
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
                      className={`hover:bg-zinc-850/80 cursor-pointer transition-colors ${
                        isSelected ? 'bg-zinc-850 text-cyan-300' : ''
                      }`}
                    >
                      <td className="py-1.5 flex items-center gap-2 font-sans text-xs">
                        {file.isDir ? (
                          <Folder className="w-3.5 h-3.5 text-amber-400 shrink-0" />
                        ) : (
                          <FileText className="w-3.5 h-3.5 text-zinc-400 shrink-0" />
                        )}
                        <span className="truncate">{file.name}</span>
                      </td>
                      <td className="py-1.5 text-zinc-400">
                        {file.isDir ? '-' : formatFileSize(file.size)}
                      </td>
                      <td className="py-1.5 text-zinc-500">{file.modified}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          {/* Local Bottom Action */}
          <div className="p-2 bg-zinc-900/50 border-t border-zinc-850 flex items-center justify-between">
            <span className="text-[11px] text-zinc-500 font-mono">
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
              className="flex items-center gap-1.5 px-3 py-1 bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:hover:bg-cyan-600 text-white rounded-md text-xs font-medium transition-colors"
            >
              <span>Upload to Remote</span>
              <ArrowRight className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>

        {/* Right Pane: Remote SSH Server */}
        <div className="w-1/2 flex flex-col bg-zinc-950">
          {/* Remote Path Navigation Bar */}
          <div className="h-9 bg-zinc-900/70 border-b border-zinc-850 px-3 flex items-center gap-2 text-xs">
            <Server className="w-3.5 h-3.5 text-emerald-400 shrink-0" />
            <button
              onClick={() => handleNavigateRemote('/root')}
              title="Go to Home"
              className="p-1 text-zinc-400 hover:text-zinc-200"
            >
              <Home className="w-3.5 h-3.5" />
            </button>
            <div className="flex-1 flex items-center bg-zinc-950 border border-zinc-800 rounded px-2 py-0.5 font-mono text-[11px] text-zinc-200">
              <input
                type="text"
                value={remotePathInput}
                onChange={(e) => setRemotePathInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') handleNavigateRemote(remotePathInput);
                }}
                className="w-full bg-transparent focus:outline-none text-cyan-300"
              />
            </div>
            <button
              onClick={() => handleNavigateRemote(remotePathInput)}
              className="px-2 py-0.5 bg-zinc-800 hover:bg-zinc-700 text-zinc-200 rounded text-[11px] font-medium"
            >
              Go
            </button>
          </div>

          {/* Remote File Table */}
          <div className="flex-1 overflow-y-auto p-2">
            <table className="w-full text-xs text-left text-zinc-300 font-sans">
              <thead className="text-[11px] text-zinc-500 uppercase border-b border-zinc-850">
                <tr>
                  <th className="pb-2 font-medium">Name</th>
                  <th className="pb-2 font-medium w-20">Size</th>
                  <th className="pb-2 font-medium w-24">Perms</th>
                  <th className="pb-2 font-medium w-28">Modified</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-900 font-mono text-[11px]">
                {/* Parent Folder Button */}
                {remotePath !== '/' && (
                  <tr
                    onClick={() => handleNavigateRemote('..')}
                    className="hover:bg-zinc-850/80 cursor-pointer text-zinc-400"
                  >
                    <td className="py-1.5 flex items-center gap-2 font-sans text-xs">
                      <Folder className="w-3.5 h-3.5 text-zinc-500" />
                      <span>.. [Parent Directory]</span>
                    </td>
                    <td className="py-1.5">-</td>
                    <td className="py-1.5">drwxr-xr-x</td>
                    <td className="py-1.5">-</td>
                  </tr>
                )}

                {displayRemoteFiles.map((file) => {
                  const isSelected = selectedRemoteFile?.name === file.name;
                  return (
                    <tr
                      key={file.name}
                      onClick={() => setSelectedRemoteFile(file)}
                      onDoubleClick={() => {
                        if (file.isDir) {
                          handleNavigateRemote(
                            remotePath === '/' ? `/${file.name}` : `${remotePath}/${file.name}`
                          );
                        } else {
                          onOpenFileEditor({
                            path: file.path,
                            name: file.name,
                            content: file.content || `# ${file.name}\n`,
                          });
                        }
                      }}
                      className={`hover:bg-zinc-850/80 cursor-pointer transition-colors ${
                        isSelected ? 'bg-zinc-850 text-cyan-300' : ''
                      }`}
                    >
                      <td className="py-1.5 flex items-center gap-2 font-sans text-xs">
                        {file.isDir ? (
                          <Folder className="w-3.5 h-3.5 text-amber-400 shrink-0" />
                        ) : file.name.endsWith('.sh') || file.name.endsWith('.yml') || file.name.endsWith('.conf') ? (
                          <FileCode className="w-3.5 h-3.5 text-cyan-400 shrink-0" />
                        ) : (
                          <FileText className="w-3.5 h-3.5 text-zinc-400 shrink-0" />
                        )}
                        <span className="truncate">{file.name}</span>
                      </td>
                      <td className="py-1.5 text-zinc-400">
                        {file.isDir ? '-' : formatFileSize(file.size)}
                      </td>
                      <td className="py-1.5 text-zinc-500 font-mono text-[10px]">
                        {file.permissions}
                      </td>
                      <td className="py-1.5 text-zinc-500">{file.modified}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          {/* Remote Bottom Action Bar */}
          <div className="p-2 bg-zinc-900/50 border-t border-zinc-850 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <button
                disabled={!selectedRemoteFile}
                onClick={() => {
                  if (selectedRemoteFile && !selectedRemoteFile.isDir) {
                    handleSimulateTransfer('download', selectedRemoteFile.name, selectedRemoteFile.size);
                  }
                }}
                className="flex items-center gap-1.5 px-3 py-1 bg-amber-600 hover:bg-amber-500 disabled:opacity-40 disabled:hover:bg-amber-600 text-white rounded-md text-xs font-medium transition-colors"
              >
                <ArrowLeft className="w-3.5 h-3.5" />
                <span>Download</span>
              </button>

              <button
                disabled={!selectedRemoteFile || selectedRemoteFile.isDir}
                onClick={() => {
                  if (selectedRemoteFile) {
                    onOpenFileEditor({
                      path: selectedRemoteFile.path,
                      name: selectedRemoteFile.name,
                      content: selectedRemoteFile.content || '',
                    });
                  }
                }}
                className="flex items-center gap-1 px-2.5 py-1 bg-zinc-800 hover:bg-zinc-750 disabled:opacity-40 text-zinc-200 rounded-md text-xs transition-colors"
              >
                <Edit2 className="w-3.5 h-3.5 text-cyan-400" />
                <span>Edit File</span>
              </button>

              <button
                disabled={!selectedRemoteFile}
                onClick={() => {
                  if (selectedRemoteFile) handleDeleteRemoteFile(selectedRemoteFile);
                }}
                className="p-1.5 hover:bg-red-950/60 disabled:opacity-40 text-zinc-400 hover:text-red-400 rounded-md transition-colors"
                title="Delete File"
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
            </div>

            <span className="text-[11px] text-zinc-500 font-mono">
              {displayRemoteFiles.length} files in {remotePath}
            </span>
          </div>
        </div>
      </div>

      {/* Bottom SFTP Transfer Queue Drawer */}
      <div className="h-28 bg-zinc-900/95 border-t border-zinc-800 flex flex-col">
        <div className="h-6 bg-zinc-850 px-3 flex items-center justify-between text-[11px] text-zinc-400">
          <div className="flex items-center gap-2 font-medium">
            <FolderSync className="w-3 h-3 text-cyan-400" />
            <span>Transfer Queue & Speed Monitor ({transfers.length})</span>
          </div>
          <button
            onClick={() => setTransfers([])}
            className="hover:text-zinc-200 transition-colors"
          >
            Clear Completed
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-2 space-y-1.5 font-mono text-xs">
          {transfers.map((tx) => (
            <div
              key={tx.id}
              className="flex items-center justify-between p-1.5 bg-zinc-950/80 border border-zinc-800 rounded text-[11px]"
            >
              <div className="flex items-center gap-2 min-w-0">
                {tx.direction === 'upload' ? (
                  <Upload className="w-3.5 h-3.5 text-cyan-400 shrink-0" />
                ) : (
                  <Download className="w-3.5 h-3.5 text-amber-400 shrink-0" />
                )}
                <span className="text-zinc-200 font-medium truncate max-w-[200px]">
                  {tx.filename}
                </span>
                <span className="text-zinc-500 text-[10px]">({formatFileSize(tx.size)})</span>
              </div>

              {/* Progress bar and speed */}
              <div className="flex items-center gap-3">
                <div className="w-32 bg-zinc-800 rounded-full h-1.5 overflow-hidden">
                  <div
                    className={`h-full transition-all duration-300 ${
                      tx.status === 'completed' ? 'bg-emerald-500' : 'bg-cyan-500'
                    }`}
                    style={{ width: `${tx.progress}%` }}
                  />
                </div>
                <span className="text-zinc-400 w-16 text-right text-[10px]">
                  {tx.speed}
                </span>
                <span
                  className={`text-[10px] px-1.5 py-0.2 rounded font-sans uppercase ${
                    tx.status === 'completed'
                      ? 'bg-emerald-500/20 text-emerald-400 border border-emerald-500/30'
                      : 'bg-cyan-500/20 text-cyan-400'
                  }`}
                >
                  {tx.status}
                </span>
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Modal: New File Creation */}
      {showNewFileModal && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
          <div className="bg-zinc-900 border border-zinc-750 rounded-xl max-w-sm w-full p-4 space-y-4 shadow-2xl">
            <h4 className="text-sm font-semibold text-zinc-100 flex items-center gap-2">
              <FileCode className="w-4 h-4 text-cyan-400" />
              <span>Create New File</span>
            </h4>
            <div className="space-y-1">
              <label className="text-xs text-zinc-400">File Name</label>
              <input
                type="text"
                placeholder="e.g. custom_service.conf or run.sh"
                value={newFileName}
                onChange={(e) => setNewFileName(e.target.value)}
                autoFocus
                className="w-full px-3 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-xs text-zinc-100 focus:outline-none focus:border-cyan-500"
              />
            </div>
            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={() => setShowNewFileModal(false)}
                className="px-3 py-1.5 text-xs text-zinc-400 hover:text-zinc-200"
              >
                Cancel
              </button>
              <button
                onClick={handleCreateNewFile}
                className="px-3.5 py-1.5 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg text-xs font-medium"
              >
                Create & Edit
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
