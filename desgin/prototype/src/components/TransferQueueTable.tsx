import React, { useState } from 'react';
import {
  Upload,
  Download,
  Folder,
  File,
  FileCode,
  FileText,
  FileArchive,
  ChevronRight,
  ChevronDown,
  Play,
  Pause,
  RotateCcw,
  Trash2,
  CheckCircle2,
  AlertCircle,
  Clock,
  ExternalLink,
  Layers,
  FolderOpen,
  ArrowUpDown,
  RefreshCw,
  Search,
  Filter,
} from 'lucide-react';
import { SFTPTransferItem, SFTPTransferSubItem } from '../types';

interface TransferQueueTableProps {
  transfers: SFTPTransferItem[];
  onUpdateTransfers: (transfers: SFTPTransferItem[]) => void;
  onClearCompleted?: () => void;
  onOpenFileEditor?: (file: { path: string; name: string; content: string }) => void;
  compact?: boolean;
}

export const TransferQueueTable: React.FC<TransferQueueTableProps> = ({
  transfers,
  onUpdateTransfers,
  onClearCompleted,
  onOpenFileEditor,
  compact = false,
}) => {
  const [expandedRows, setExpandedRows] = useState<Set<string>>(new Set(['tx-dir-k8s', 'tx-dir-certs']));
  const [statusFilter, setStatusFilter] = useState<'all' | 'transferring' | 'completed' | 'failed' | 'paused'>('all');
  const [searchQuery, setSearchQuery] = useState<string>('');

  const toggleExpand = (id: string, e?: React.MouseEvent) => {
    if (e) e.stopPropagation();
    setExpandedRows((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const handlePauseResume = (id: string, e?: React.MouseEvent) => {
    if (e) e.stopPropagation();
    onUpdateTransfers(
      transfers.map((tx) => {
        if (tx.id === id) {
          const nextStatus = tx.status === 'transferring' ? 'paused' : 'transferring';
          return {
            ...tx,
            status: nextStatus,
            speed: nextStatus === 'paused' ? '0 KB/s' : '15.2 MB/s',
            children: tx.children?.map((c) =>
              c.status === 'transferring' || c.status === 'paused'
                ? { ...c, status: nextStatus, speed: nextStatus === 'paused' ? '0 KB/s' : '5.1 MB/s' }
                : c
            ),
          };
        }
        return tx;
      })
    );
  };

  const handleRetry = (id: string, e?: React.MouseEvent) => {
    if (e) e.stopPropagation();
    onUpdateTransfers(
      transfers.map((tx) => {
        if (tx.id === id) {
          return {
            ...tx,
            status: 'transferring',
            progress: 10,
            transferred: Math.floor(tx.size * 0.1),
            speed: '18.4 MB/s',
            errorMsg: undefined,
            children: tx.children?.map((c) => ({
              ...c,
              status: 'transferring',
              progress: 15,
              transferred: Math.floor(c.size * 0.15),
              errorMsg: undefined,
            })),
          };
        }
        return tx;
      })
    );
  };

  const handleRemove = (id: string, e?: React.MouseEvent) => {
    if (e) e.stopPropagation();
    onUpdateTransfers(transfers.filter((tx) => tx.id !== id));
  };

  const handlePauseAll = () => {
    onUpdateTransfers(
      transfers.map((tx) =>
        tx.status === 'transferring'
          ? {
              ...tx,
              status: 'paused',
              speed: '0 KB/s',
              children: tx.children?.map((c) =>
                c.status === 'transferring' ? { ...c, status: 'paused', speed: '0 KB/s' } : c
              ),
            }
          : tx
      )
    );
  };

  const handleResumeAll = () => {
    onUpdateTransfers(
      transfers.map((tx) =>
        tx.status === 'paused'
          ? {
              ...tx,
              status: 'transferring',
              speed: '21.0 MB/s',
              children: tx.children?.map((c) =>
                c.status === 'paused' ? { ...c, status: 'transferring', speed: '6.5 MB/s' } : c
              ),
            }
          : tx
      )
    );
  };

  const formatFileSize = (bytes: number) => {
    if (!bytes || bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
  };

  const getFileIcon = (name: string, isDir?: boolean) => {
    if (isDir) {
      return <Folder className="w-3.5 h-3.5 text-[#CE9178] shrink-0" />;
    }
    const ext = name.split('.').pop()?.toLowerCase();
    if (ext === 'sh' || ext === 'bash' || ext === 'zsh') {
      return <FileCode className="w-3.5 h-3.5 text-[#4EC9B0] shrink-0" />;
    }
    if (ext === 'js' || ext === 'ts' || ext === 'jsx' || ext === 'tsx' || ext === 'py' || ext === 'go') {
      return <FileCode className="w-3.5 h-3.5 text-[#569CD6] shrink-0" />;
    }
    if (ext === 'yml' || ext === 'yaml' || ext === 'json' || ext === 'conf' || ext === 'toml' || ext === 'env') {
      return <FileText className="w-3.5 h-3.5 text-[#DCDCAA] shrink-0" />;
    }
    if (ext === 'tar' || ext === 'gz' || ext === 'zip' || ext === 'bz2' || ext === '7z' || ext === 'rar') {
      return <FileArchive className="w-3.5 h-3.5 text-[#C586C0] shrink-0" />;
    }
    return <File className="w-3.5 h-3.5 text-[#9CDCFE] shrink-0" />;
  };

  // Filter transfers
  const filteredTransfers = transfers.filter((tx) => {
    if (statusFilter !== 'all' && tx.status !== statusFilter) return false;
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      const matchParent = tx.filename.toLowerCase().includes(q) || tx.hostName.toLowerCase().includes(q);
      const matchChild = tx.children?.some((c) => c.filename.toLowerCase().includes(q));
      return matchParent || matchChild;
    }
    return true;
  });

  const counts = {
    all: transfers.length,
    transferring: transfers.filter((t) => t.status === 'transferring').length,
    completed: transfers.filter((t) => t.status === 'completed').length,
    failed: transfers.filter((t) => t.status === 'failed').length,
    paused: transfers.filter((t) => t.status === 'paused').length,
  };

  return (
    <div className="flex flex-col h-full bg-[#181818] text-[#D4D4D4] font-sans text-xs select-none">
      {/* Table Control / Filter Toolbar */}
      <div className="h-8 bg-[#222222] border-b border-[#333333] px-2.5 flex items-center justify-between gap-2 shrink-0">
        {/* Filter Badges */}
        <div className="flex items-center gap-1 overflow-x-auto scrollbar-none font-mono text-[11px]">
          <button
            onClick={() => setStatusFilter('all')}
            className={`px-2 py-0.5 rounded transition-colors ${
              statusFilter === 'all'
                ? 'bg-[#007ACC] text-white font-medium'
                : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#858585] hover:text-[#D4D4D4]'
            }`}
          >
            All ({counts.all})
          </button>
          <button
            onClick={() => setStatusFilter('transferring')}
            className={`flex items-center gap-1 px-2 py-0.5 rounded transition-colors ${
              statusFilter === 'transferring'
                ? 'bg-[#007ACC]/30 text-[#4EC9B0] border border-[#007ACC]'
                : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#858585]'
            }`}
          >
            <span className="w-1.5 h-1.5 rounded-full bg-[#4EC9B0] animate-pulse" />
            <span>Active ({counts.transferring})</span>
          </button>
          <button
            onClick={() => setStatusFilter('completed')}
            className={`flex items-center gap-1 px-2 py-0.5 rounded transition-colors ${
              statusFilter === 'completed'
                ? 'bg-[#27C93F]/20 text-[#27C93F] border border-[#27C93F]/60'
                : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#858585]'
            }`}
          >
            <span>Done ({counts.completed})</span>
          </button>
          {counts.failed > 0 && (
            <button
              onClick={() => setStatusFilter('failed')}
              className={`flex items-center gap-1 px-2 py-0.5 rounded transition-colors ${
                statusFilter === 'failed'
                  ? 'bg-[#FF5F56]/20 text-[#FF5F56] border border-[#FF5F56]/60'
                  : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#FF5F56]'
              }`}
            >
              <span>Failed ({counts.failed})</span>
            </button>
          )}
          {counts.paused > 0 && (
            <button
              onClick={() => setStatusFilter('paused')}
              className={`flex items-center gap-1 px-2 py-0.5 rounded transition-colors ${
                statusFilter === 'paused'
                  ? 'bg-[#CCA700]/20 text-[#CCA700] border border-[#CCA700]/60'
                  : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#CCA700]'
              }`}
            >
              <span>Paused ({counts.paused})</span>
            </button>
          )}
        </div>

        {/* Search and Action Buttons */}
        <div className="flex items-center gap-1.5 shrink-0">
          <div className="flex items-center bg-[#181818] border border-[#333333] rounded px-1.5 py-0.5 text-[11px] w-32 focus-within:w-44 transition-all">
            <Search className="w-3 h-3 text-[#858585] mr-1 shrink-0" />
            <input
              type="text"
              placeholder="Search file..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="w-full bg-transparent focus:outline-none text-[#D4D4D4] font-mono text-[10px]"
            />
          </div>

          <div className="w-[1px] h-3.5 bg-[#333333] mx-0.5" />

          {counts.transferring > 0 ? (
            <button
              onClick={handlePauseAll}
              title="Pause all active transfers"
              className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#CCA700] border border-[#3E3E42] transition-colors text-[11px]"
            >
              <Pause className="w-3 h-3" />
              <span className="hidden sm:inline">Pause All</span>
            </button>
          ) : counts.paused > 0 ? (
            <button
              onClick={handleResumeAll}
              title="Resume all paused transfers"
              className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#4EC9B0] border border-[#3E3E42] transition-colors text-[11px]"
            >
              <Play className="w-3 h-3" />
              <span className="hidden sm:inline">Resume All</span>
            </button>
          ) : null}

          {counts.completed > 0 && (
            <button
              onClick={() => {
                if (onClearCompleted) onClearCompleted();
                else onUpdateTransfers(transfers.filter((t) => t.status !== 'completed'));
              }}
              title="Clear completed transfers"
              className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#858585] hover:text-[#D4D4D4] border border-[#3E3E42] transition-colors text-[11px]"
            >
              <Trash2 className="w-3 h-3" />
              <span className="hidden sm:inline">Clear Done</span>
            </button>
          )}
        </div>
      </div>

      {/* Main Table Content */}
      <div className="flex-1 overflow-y-auto font-mono text-[11px]">
        {filteredTransfers.length === 0 ? (
          <div className="h-full min-h-[100px] flex flex-col items-center justify-center text-[#6D6D6D] p-6 space-y-1.5">
            <FolderOpen className="w-7 h-7 text-[#454545]" />
            <p className="text-xs">No transfer items matching the current filter</p>
            <p className="text-[10px] text-[#555555]">
              Upload or download files/folders from the SFTP file explorer above
            </p>
          </div>
        ) : (
          <table className="w-full text-left border-collapse">
            <thead className="text-[10px] uppercase text-[#6D6D6D] bg-[#1E1E1E] sticky top-0 border-b border-[#333333] z-10 select-none">
              <tr>
                <th className="py-1.5 pl-2 font-semibold w-10 text-center">Type</th>
                <th className="py-1.5 pl-1 font-semibold min-w-[160px]">File / Directory Name</th>
                <th className="py-1.5 font-semibold w-24">Direction</th>
                <th className="py-1.5 font-semibold w-32">Size & Transferred</th>
                <th className="py-1.5 font-semibold w-40">Progress</th>
                <th className="py-1.5 font-semibold w-20">Speed</th>
                <th className="py-1.5 font-semibold w-24">Status</th>
                <th className="py-1.5 pr-2 font-semibold w-24 text-right">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[#262626]">
              {filteredTransfers.map((tx) => {
                const hasChildren = (tx.isDir || (tx.children && tx.children.length > 0));
                const isExpanded = expandedRows.has(tx.id);
                const isCompleted = tx.status === 'completed';
                const isTransferring = tx.status === 'transferring';
                const isPaused = tx.status === 'paused';
                const isFailed = tx.status === 'failed';

                // Children stats
                const childCount = tx.children ? tx.children.length : 0;
                const completedChildCount = tx.children ? tx.children.filter((c) => c.status === 'completed').length : 0;

                return (
                  <React.Fragment key={tx.id}>
                    {/* Main Row */}
                    <tr
                      onClick={() => hasChildren && toggleExpand(tx.id)}
                      className={`hover:bg-[#252526] transition-colors group ${
                        hasChildren ? 'cursor-pointer' : ''
                      } ${isExpanded ? 'bg-[#1F1F1F]' : ''}`}
                    >
                      {/* Expand / Type Icon */}
                      <td className="py-1.5 pl-2 text-center">
                        <div className="flex items-center justify-center gap-1">
                          {hasChildren ? (
                            <button
                              onClick={(e) => toggleExpand(tx.id, e)}
                              className="p-0.5 hover:bg-[#333333] rounded text-[#858585] hover:text-[#D4D4D4] transition-colors"
                              title={isExpanded ? 'Collapse folder details' : 'Expand folder sub-items'}
                            >
                              {isExpanded ? (
                                <ChevronDown className="w-3.5 h-3.5 text-[#569CD6]" />
                              ) : (
                                <ChevronRight className="w-3.5 h-3.5 text-[#858585]" />
                              )}
                            </button>
                          ) : (
                            <span className="w-3.5 h-3.5 block" />
                          )}
                          {getFileIcon(tx.filename, tx.isDir)}
                        </div>
                      </td>

                      {/* Name & Folder Tag */}
                      <td className="py-1.5 pl-1 font-sans text-xs">
                        <div className="flex items-center gap-2 min-w-0">
                          <span className="font-medium text-[#D4D4D4] truncate max-w-[200px] sm:max-w-xs group-hover:text-white">
                            {tx.filename}
                          </span>

                          {hasChildren && (
                            <span className="flex items-center gap-1 px-1.5 py-0.2 rounded bg-[#007ACC]/15 text-[#569CD6] border border-[#007ACC]/30 text-[10px] font-mono shrink-0">
                              <Folder className="w-2.5 h-2.5 text-[#CE9178]" />
                              <span>
                                {completedChildCount}/{childCount} items
                              </span>
                            </span>
                          )}

                          {tx.hostName && (
                            <span className="text-[10px] text-[#6D6D6D] font-mono truncate max-w-[100px] hidden md:inline">
                              @{tx.hostName}
                            </span>
                          )}
                        </div>
                      </td>

                      {/* Direction */}
                      <td className="py-1.5">
                        <span
                          className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium ${
                            tx.direction === 'upload'
                              ? 'bg-[#007ACC]/15 text-[#4EC9B0] border border-[#007ACC]/40'
                              : 'bg-[#DCDCAA]/15 text-[#DCDCAA] border border-[#DCDCAA]/40'
                          }`}
                        >
                          {tx.direction === 'upload' ? (
                            <Upload className="w-2.5 h-2.5" />
                          ) : (
                            <Download className="w-2.5 h-2.5" />
                          )}
                          <span>{tx.direction === 'upload' ? 'Upload' : 'Download'}</span>
                        </span>
                      </td>

                      {/* Size & Transferred */}
                      <td className="py-1.5 text-[10px] text-[#858585]">
                        <span className="text-[#D4D4D4]">{formatFileSize(tx.transferred)}</span>
                        <span className="text-[#555555]"> / </span>
                        <span>{formatFileSize(tx.size)}</span>
                      </td>

                      {/* Progress Bar & Percentage */}
                      <td className="py-1.5">
                        <div className="flex items-center gap-2 pr-2">
                          <div className="flex-1 bg-[#2D2D2D] rounded-full h-1.5 overflow-hidden">
                            <div
                              className={`h-full transition-all duration-300 ${
                                isCompleted
                                  ? 'bg-[#27C93F]'
                                  : isFailed
                                  ? 'bg-[#FF5F56]'
                                  : isPaused
                                  ? 'bg-[#CCA700]'
                                  : 'bg-[#007ACC]'
                              }`}
                              style={{ width: `${Math.min(100, Math.max(0, tx.progress))}%` }}
                            />
                          </div>
                          <span
                            className={`w-9 text-right font-mono text-[10px] font-medium ${
                              isCompleted
                                ? 'text-[#27C93F]'
                                : isFailed
                                ? 'text-[#FF5F56]'
                                : isPaused
                                ? 'text-[#CCA700]'
                                : 'text-[#4EC9B0]'
                            }`}
                          >
                            {tx.progress}%
                          </span>
                        </div>
                      </td>

                      {/* Speed */}
                      <td className="py-1.5 text-[10px]">
                        <span
                          className={`font-mono ${
                            isTransferring ? 'text-[#4EC9B0] font-semibold' : 'text-[#6D6D6D]'
                          }`}
                        >
                          {isCompleted ? '-' : tx.speed || '0 KB/s'}
                        </span>
                      </td>

                      {/* Status */}
                      <td className="py-1.5">
                        <span
                          className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[9px] font-sans font-semibold uppercase tracking-wider ${
                            isCompleted
                              ? 'bg-[#27C93F]/20 text-[#27C93F] border border-[#27C93F]/40'
                              : isTransferring
                              ? 'bg-[#007ACC]/25 text-[#4EC9B0] border border-[#007ACC]/50'
                              : isPaused
                              ? 'bg-[#CCA700]/20 text-[#CCA700] border border-[#CCA700]/40'
                              : isFailed
                              ? 'bg-[#FF5F56]/20 text-[#FF5F56] border border-[#FF5F56]/40'
                              : 'bg-[#333333] text-[#858585]'
                          }`}
                        >
                          {isTransferring && (
                            <span className="w-1.5 h-1.5 rounded-full bg-[#4EC9B0] animate-pulse shrink-0" />
                          )}
                          {isCompleted && <CheckCircle2 className="w-2.5 h-2.5 shrink-0" />}
                          {isFailed && <AlertCircle className="w-2.5 h-2.5 shrink-0" />}
                          {isPaused && <Pause className="w-2.5 h-2.5 shrink-0" />}
                          <span>{tx.status}</span>
                        </span>
                      </td>

                      {/* Actions */}
                      <td className="py-1.5 pr-2 text-right">
                        <div className="flex items-center justify-end gap-1">
                          {/* Pause / Resume */}
                          {(isTransferring || isPaused) && (
                            <button
                              onClick={(e) => handlePauseResume(tx.id, e)}
                              title={isTransferring ? 'Pause' : 'Resume'}
                              className="p-1 hover:bg-[#333333] text-[#858585] hover:text-[#D4D4D4] rounded transition-colors"
                            >
                              {isTransferring ? (
                                <Pause className="w-3 h-3 text-[#CCA700]" />
                              ) : (
                                <Play className="w-3 h-3 text-[#4EC9B0]" />
                              )}
                            </button>
                          )}

                          {/* Retry on failure */}
                          {isFailed && (
                            <button
                              onClick={(e) => handleRetry(tx.id, e)}
                              title="Retry transfer"
                              className="p-1 hover:bg-[#333333] text-[#4EC9B0] rounded transition-colors"
                            >
                              <RotateCcw className="w-3 h-3" />
                            </button>
                          )}

                          {/* Open File Editor if single file completed */}
                          {isCompleted && !tx.isDir && onOpenFileEditor && (
                            <button
                              onClick={(e) => {
                                e.stopPropagation();
                                onOpenFileEditor({
                                  path: tx.path || tx.remotePath || `/root/${tx.filename}`,
                                  name: tx.filename,
                                  content: `# ${tx.filename}\n# Transferred via WinSSH Pro\n`,
                                });
                              }}
                              title="Open in Code Editor"
                              className="p-1 hover:bg-[#333333] text-[#569CD6] rounded transition-colors"
                            >
                              <ExternalLink className="w-3 h-3" />
                            </button>
                          )}

                          {/* Remove */}
                          <button
                            onClick={(e) => handleRemove(tx.id, e)}
                            title="Remove from queue"
                            className="p-1 hover:bg-[#FF5F56]/20 text-[#6D6D6D] hover:text-[#FF5F56] rounded transition-colors"
                          >
                            <Trash2 className="w-3 h-3" />
                          </button>
                        </div>
                      </td>
                    </tr>

                    {/* Expandable Sub-Table for Folder Child Items */}
                    {hasChildren && isExpanded && tx.children && (
                      <tr className="bg-[#141414]">
                        <td colSpan={8} className="p-0">
                          <div className="py-2 pl-8 pr-3 border-y border-[#2D2D2D] bg-[#141414] shadow-inner">
                            <div className="flex items-center justify-between pb-1.5 mb-1.5 border-b border-[#252526] text-[10px] text-[#858585]">
                              <div className="flex items-center gap-1.5 font-medium">
                                <FolderOpen className="w-3 h-3 text-[#CE9178]" />
                                <span className="text-[#D4D4D4] font-semibold">{tx.filename}</span>
                                <span>Directory Contents ({tx.children.length} items)</span>
                              </div>
                              <span className="font-mono text-[#6D6D6D]">
                                Transferred: {formatFileSize(tx.transferred)} of {formatFileSize(tx.size)}
                              </span>
                            </div>

                            {/* Sub Table */}
                            <table className="w-full text-left font-mono text-[10px]">
                              <thead className="text-[9px] uppercase text-[#666666] border-b border-[#262626]">
                                <tr>
                                  <th className="pb-1 pl-1 font-medium min-w-[140px]">Sub-Item File</th>
                                  <th className="pb-1 w-28 font-medium">Size</th>
                                  <th className="pb-1 w-36 font-medium">Progress</th>
                                  <th className="pb-1 w-20 font-medium">Speed</th>
                                  <th className="pb-1 w-20 font-medium">Status</th>
                                </tr>
                              </thead>
                              <tbody className="divide-y divide-[#1D1D1D]">
                                {tx.children.map((child) => {
                                  const isChildCompleted = child.status === 'completed';
                                  const isChildTransferring = child.status === 'transferring';
                                  const isChildPaused = child.status === 'paused';
                                  const isChildFailed = child.status === 'failed';

                                  return (
                                    <tr
                                      key={child.id}
                                      className="hover:bg-[#1C1C1C] transition-colors"
                                    >
                                      {/* Sub item filename */}
                                      <td className="py-1 pl-1 font-sans text-[11px]">
                                        <div className="flex items-center gap-1.5">
                                          {getFileIcon(child.filename, child.isDir)}
                                          <span className="text-[#CCCCCC] font-mono text-[10px] truncate max-w-[220px]">
                                            {child.relativePath || child.filename}
                                          </span>
                                        </div>
                                      </td>

                                      {/* Sub item size */}
                                      <td className="py-1 text-[#777777]">
                                        <span className="text-[#BBBBBB]">
                                          {formatFileSize(child.transferred)}
                                        </span>
                                        <span> / {formatFileSize(child.size)}</span>
                                      </td>

                                      {/* Sub item progress */}
                                      <td className="py-1">
                                        <div className="flex items-center gap-1.5 pr-2">
                                          <div className="flex-1 bg-[#252525] rounded-full h-1 overflow-hidden">
                                            <div
                                              className={`h-full transition-all duration-300 ${
                                                isChildCompleted
                                                  ? 'bg-[#27C93F]'
                                                  : isChildFailed
                                                  ? 'bg-[#FF5F56]'
                                                  : isChildPaused
                                                  ? 'bg-[#CCA700]'
                                                  : 'bg-[#007ACC]'
                                              }`}
                                              style={{ width: `${Math.min(100, Math.max(0, child.progress))}%` }}
                                            />
                                          </div>
                                          <span
                                            className={`w-7 text-right text-[9px] font-mono ${
                                              isChildCompleted
                                                ? 'text-[#27C93F]'
                                                : isChildFailed
                                                ? 'text-[#FF5F56]'
                                                : 'text-[#858585]'
                                            }`}
                                          >
                                            {child.progress}%
                                          </span>
                                        </div>
                                      </td>

                                      {/* Sub item speed */}
                                      <td className="py-1 text-[#777777]">
                                        {isChildCompleted ? '-' : child.speed || '0 KB/s'}
                                      </td>

                                      {/* Sub item status */}
                                      <td className="py-1">
                                        <span
                                          className={`inline-flex items-center gap-1 px-1 py-0.2 rounded text-[8.5px] uppercase ${
                                            isChildCompleted
                                              ? 'bg-[#27C93F]/15 text-[#27C93F]'
                                              : isChildTransferring
                                              ? 'bg-[#007ACC]/20 text-[#4EC9B0]'
                                              : isChildPaused
                                              ? 'bg-[#CCA700]/15 text-[#CCA700]'
                                              : isChildFailed
                                              ? 'bg-[#FF5F56]/15 text-[#FF5F56]'
                                              : 'bg-[#262626] text-[#777777]'
                                          }`}
                                        >
                                          {isChildTransferring && (
                                            <span className="w-1 h-1 rounded-full bg-[#4EC9B0] animate-pulse" />
                                          )}
                                          <span>{child.status}</span>
                                        </span>
                                      </td>
                                    </tr>
                                  );
                                })}
                              </tbody>
                            </table>
                          </div>
                        </td>
                      </tr>
                    )}
                  </React.Fragment>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
};
