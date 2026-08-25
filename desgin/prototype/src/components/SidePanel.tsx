import React, { useState, useRef, useEffect } from 'react';
import {
  Server,
  Code2,
  Network,
  KeyRound,
  Activity,
  Plus,
  Search,
  ChevronDown,
  ChevronRight,
  ChevronLeft,
  MoreVertical,
  Terminal,
  FolderSync,
  Folder,
  Trash2,
  Edit2,
  Copy,
  Check,
  Play,
  ArrowRight,
  Shield,
  Radio,
  ExternalLink,
  RefreshCw,
  Sliders,
  Filter,
  SlidersHorizontal,
  X,
} from 'lucide-react';
import { ServerHost, DevOpsSnippet, PortTunnel, SSHKeyItem, TabType } from '../types';
import { ActionPanelId } from './ActionBar';
import { SnippetModal } from './SnippetModal';

interface SidePanelProps {
  activePanel: ActionPanelId;
  isOpen: boolean;
  onClose: () => void;
  width: number;
  onResizeWidth: (w: number) => void;

  // Hosts
  hosts: ServerHost[];
  selectedHostId?: string;
  onConnectHost: (host: ServerHost, tabType?: TabType) => void;
  onAddNewHost: () => void;
  onEditHost: (host: ServerHost) => void;
  onDeleteHost: (hostId: string) => void;
  onDuplicateHost: (host: ServerHost) => void;

  // Snippets
  snippets: DevOpsSnippet[];
  onUpdateSnippets: (snippets: DevOpsSnippet[]) => void;
  onExecuteSnippet: (command: string) => void;

  // Tunnels
  tunnels: PortTunnel[];
  onUpdateTunnels: (tunnels: PortTunnel[]) => void;

  // Keys
  keys: SSHKeyItem[];
  onUpdateKeys: (keys: SSHKeyItem[]) => void;
}

export const SidePanel: React.FC<SidePanelProps> = ({
  activePanel,
  isOpen,
  onClose,
  width,
  onResizeWidth,
  hosts,
  selectedHostId,
  onConnectHost,
  onAddNewHost,
  onEditHost,
  onDeleteHost,
  onDuplicateHost,
  snippets,
  onUpdateSnippets,
  onExecuteSnippet,
  tunnels,
  onUpdateTunnels,
  keys,
  onUpdateKeys,
}) => {
  const [isResizing, setIsResizing] = useState(false);

  // Drag-to-resize panel
  const handleMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    setIsResizing(true);
  };

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!isResizing) return;
      // Subtract the actionbar width (48px)
      const newWidth = Math.max(220, Math.min(560, e.clientX - 48));
      onResizeWidth(newWidth);
    };

    const handleMouseUp = () => {
      setIsResizing(false);
    };

    if (isResizing) {
      window.addEventListener('mousemove', handleMouseMove);
      window.addEventListener('mouseup', handleMouseUp);
    }
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isResizing, onResizeWidth]);

  // Snippets modal state
  const [isSnippetModalOpen, setIsSnippetModalOpen] = useState(false);
  const [editingSnippet, setEditingSnippet] = useState<DevOpsSnippet | null>(null);

  const handleOpenNewSnippet = () => {
    setEditingSnippet(null);
    setIsSnippetModalOpen(true);
  };

  const handleOpenEditSnippet = (snippet: DevOpsSnippet) => {
    setEditingSnippet(snippet);
    setIsSnippetModalOpen(true);
  };

  const handleSaveSnippet = (snippet: DevOpsSnippet) => {
    const exists = snippets.some((s) => s.id === snippet.id);
    if (exists) {
      onUpdateSnippets(snippets.map((s) => (s.id === snippet.id ? snippet : s)));
    } else {
      onUpdateSnippets([snippet, ...snippets]);
    }
  };

  const handleDeleteSnippet = (snippetId: string) => {
    onUpdateSnippets(snippets.filter((s) => s.id !== snippetId));
  };

  if (!isOpen) return null;

  // Header Titles & Icons mapping
  const panelMeta: Record<ActionPanelId, { title: string; icon: React.ElementType }> = {
    hosts: { title: 'HOSTS & FLEET', icon: Server },
    snippets: { title: 'DEVOPS SNIPPETS', icon: Code2 },
    keys: { title: 'SSH KEY VAULT', icon: KeyRound },
    monitor: { title: 'FLEET TELEMETRY', icon: Activity },
  };

  const meta = panelMeta[activePanel] || panelMeta.hosts;
  const TitleIcon = meta.icon;

  return (
    <aside
      style={{ width: `${width}px` }}
      className="h-full bg-[#252526] border-r border-[#333333] flex flex-col relative select-none shrink-0 z-20"
    >
      {/* Panel Header */}
      <div className="h-9 px-3 flex items-center justify-between border-b border-[#333333] bg-[#1E1E1E] text-xs font-semibold text-[#D4D4D4]">
        <div className="flex items-center gap-2">
          <TitleIcon className="w-3.5 h-3.5 text-[#4EC9B0]" />
          <span className="tracking-wide text-[11px] font-bold text-[#CCCCCC]">{meta.title}</span>
        </div>

        <div className="flex items-center gap-1">
          {/* Context Action depending on panel */}
          {activePanel === 'hosts' && (
            <button
              onClick={onAddNewHost}
              title="Add New Server Host"
              className="p-1 hover:bg-[#333333] text-[#858585] hover:text-white rounded transition-colors"
            >
              <Plus className="w-3.5 h-3.5" />
            </button>
          )}

          {activePanel === 'snippets' && (
            <button
              onClick={handleOpenNewSnippet}
              title="Create New Command Snippet"
              className="p-1 hover:bg-[#333333] text-[#4EC9B0] hover:text-white rounded transition-colors"
            >
              <Plus className="w-3.5 h-3.5" />
            </button>
          )}

          {/* Collapse Panel Button */}
          <button
            onClick={onClose}
            title="Collapse Side Panel (Alt+B)"
            className="p-1 hover:bg-[#333333] text-[#858585] hover:text-white rounded transition-colors ml-0.5"
          >
            <ChevronLeft className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Dynamic Panel Content */}
      <div className="flex-1 flex flex-col overflow-hidden min-h-0">
        {activePanel === 'hosts' && (
          <HostsPanelContent
            hosts={hosts}
            selectedHostId={selectedHostId}
            onConnectHost={onConnectHost}
            onAddNewHost={onAddNewHost}
            onEditHost={onEditHost}
            onDeleteHost={onDeleteHost}
            onDuplicateHost={onDuplicateHost}
            tunnels={tunnels}
          />
        )}

        {activePanel === 'snippets' && (
          <SnippetsPanelContent
            snippets={snippets}
            onUpdateSnippets={onUpdateSnippets}
            onExecuteSnippet={onExecuteSnippet}
            onAddNewSnippet={handleOpenNewSnippet}
            onEditSnippet={handleOpenEditSnippet}
            onDeleteSnippet={handleDeleteSnippet}
          />
        )}

        {activePanel === 'keys' && (
          <KeysPanelContent keys={keys} onUpdateKeys={onUpdateKeys} />
        )}

        {activePanel === 'monitor' && (
          <TelemetryPanelContent
            hosts={hosts}
            onConnectHost={onConnectHost}
          />
        )}
      </div>

      {/* Snippet Create / Edit Modal */}
      {isSnippetModalOpen && (
        <SnippetModal
          isOpen={isSnippetModalOpen}
          onClose={() => {
            setIsSnippetModalOpen(false);
            setEditingSnippet(null);
          }}
          initialSnippet={editingSnippet}
          onSaveSnippet={handleSaveSnippet}
          onDeleteSnippet={handleDeleteSnippet}
        />
      )}

      {/* Drag Resize Handle on Right Border */}
      <div
        onMouseDown={handleMouseDown}
        className={`absolute right-0 top-0 bottom-0 w-1 cursor-col-resize hover:bg-[#007ACC] transition-colors z-30 ${
          isResizing ? 'bg-[#007ACC]' : 'bg-transparent'
        }`}
      />
    </aside>
  );
};

/* =========================================================================
   1. HOSTS & FLEET PANEL VIEW
   ========================================================================= */
const HostsPanelContent: React.FC<{
  hosts: ServerHost[];
  selectedHostId?: string;
  onConnectHost: (host: ServerHost, tabType?: TabType) => void;
  onAddNewHost: () => void;
  onEditHost: (host: ServerHost) => void;
  onDeleteHost: (hostId: string) => void;
  onDuplicateHost: (host: ServerHost) => void;
  tunnels?: PortTunnel[];
}> = ({
  hosts,
  selectedHostId,
  onConnectHost,
  onAddNewHost,
  onEditHost,
  onDeleteHost,
  onDuplicateHost,
  tunnels = [],
}) => {
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedGroup, setSelectedGroup] = useState<string>('ALL');
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const [contextMenuHostId, setContextMenuHostId] = useState<string | null>(null);

  const groups = Array.from(new Set(hosts.map((h) => h.group))) as string[];
  groups.sort();

  const filteredHosts = hosts.filter((h) => {
    const matchesSearch =
      h.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
      h.host.includes(searchQuery) ||
      h.username.toLowerCase().includes(searchQuery.toLowerCase()) ||
      h.tags.some((t) => t.toLowerCase().includes(searchQuery.toLowerCase()));

    const matchesGroup = selectedGroup === 'ALL' || h.group === selectedGroup;
    return matchesSearch && matchesGroup;
  });

  const toggleGroup = (grp: string) => {
    setCollapsedGroups((prev) => ({ ...prev, [grp]: !prev[grp] }));
  };

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* Search & Group Filter Bar */}
      <div className="p-2 border-b border-[#333333] space-y-2 bg-[#1E1E1E]">
        <div className="relative">
          <Search className="w-3 h-3 text-[#858585] absolute left-2.5 top-2.5" />
          <input
            type="text"
            placeholder="Search servers by name, IP, tag..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full pl-7 pr-2 py-1 text-xs bg-[#252526] border border-[#3C3C3C] rounded text-[#D4D4D4] placeholder-[#777777] focus:outline-none focus:border-[#007ACC]"
          />
        </div>

        {/* Group Filter Chips */}
        <div className="flex items-center gap-1 overflow-x-auto pb-0.5 scrollbar-none text-[10px]">
          <button
            onClick={() => setSelectedGroup('ALL')}
            className={`px-2 py-0.5 rounded whitespace-nowrap transition-colors ${
              selectedGroup === 'ALL'
                ? 'bg-[#37373D] text-white font-medium border border-[#007ACC]'
                : 'text-[#858585] hover:text-[#D4D4D4] bg-[#252526] border border-[#333333]'
            }`}
          >
            All ({hosts.length})
          </button>
          {groups.map((g) => (
            <button
              key={g}
              onClick={() => setSelectedGroup(g)}
              className={`px-2 py-0.5 rounded whitespace-nowrap transition-colors ${
                selectedGroup === g
                  ? 'bg-[#37373D] text-white font-medium border border-[#007ACC]'
                  : 'text-[#858585] hover:text-[#D4D4D4] bg-[#252526] border border-[#333333]'
              }`}
            >
              {g}
            </button>
          ))}
        </div>
      </div>

      {/* Host Accordions List */}
      <div className="flex-1 overflow-y-auto p-2 space-y-2.5">
        {groups.map((groupName) => {
          if (selectedGroup !== 'ALL' && selectedGroup !== groupName) return null;
          const groupHosts = filteredHosts.filter((h) => h.group === groupName);
          if (groupHosts.length === 0) return null;

          const isCollapsed = !!collapsedGroups[groupName];

          return (
            <div key={groupName} className="space-y-1">
              {/* Group Title Bar */}
              <button
                onClick={() => toggleGroup(groupName)}
                className="w-full flex items-center justify-between text-[11px] text-[#858585] hover:text-white px-1.5 py-0.5 rounded group font-semibold"
              >
                <div className="flex items-center gap-1.5 truncate">
                  {isCollapsed ? (
                    <ChevronRight className="w-3 h-3 text-[#6D6D6D]" />
                  ) : (
                    <ChevronDown className="w-3 h-3 text-[#6D6D6D]" />
                  )}
                  <Folder className="w-3.5 h-3.5 text-[#CE9178] shrink-0" />
                  <span className="text-[#D4D4D4] truncate">{groupName}</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono px-1 rounded bg-[#1E1E1E]">
                  {groupHosts.length}
                </span>
              </button>

              {/* Group Hosts Cards */}
              {!isCollapsed && (
                <div className="pl-1.5 space-y-1 border-l border-[#333333] ml-2">
                  {groupHosts.map((host) => {
                    const isSelected = selectedHostId === host.id;
                    return (
                      <div
                        key={host.id}
                        onContextMenu={(e) => {
                          e.preventDefault();
                          setContextMenuHostId(contextMenuHostId === host.id ? null : host.id);
                        }}
                        className={`group relative p-2 rounded border transition-all cursor-pointer ${
                          isSelected
                            ? 'bg-[#37373D] border-[#007ACC] text-white shadow-sm'
                            : 'bg-[#1E1E1E] hover:bg-[#2A2D2E] border-[#333333] text-[#CCCCCC]'
                        }`}
                      >
                        <div className="flex items-start justify-between">
                          <div
                            onClick={() => onConnectHost(host, 'terminal')}
                            className="flex-1 min-w-0 pr-1"
                          >
                            <div className="flex items-center gap-1.5">
                              <span
                                className="w-2 h-2 rounded-full shrink-0"
                                style={{ backgroundColor: host.color || '#27C93F' }}
                              />
                              <span className="font-medium text-xs truncate group-hover:text-[#4EC9B0] transition-colors">
                                {host.name}
                              </span>
                            </div>

                            <div className="flex items-center gap-2 mt-0.5 text-[10.5px] text-[#6D6D6D] font-mono truncate">
                              <span>
                                {host.username}@{host.host}:{host.port}
                              </span>
                            </div>

                            {/* Tags, OS & Ping Latency */}
                            <div className="flex items-center gap-1.5 mt-1.5 flex-wrap">
                              <span className="text-[9px] px-1 py-0.2 rounded bg-[#2D2D2D] text-[#858585] border border-[#454545] uppercase font-mono">
                                {host.osType}
                              </span>
                              {host.pingMs && (
                                <span className="text-[9px] text-[#27C93F] font-mono flex items-center gap-0.5">
                                  <span className="w-1 h-1 rounded-full bg-[#27C93F] animate-pulse" />
                                  {host.pingMs}ms
                                </span>
                              )}
                              {host.jumpHostId && (
                                <span className="text-[9px] px-1 py-0.2 rounded bg-[#3C3C3C] text-[#C586C0] border border-[#454545]">
                                  Proxy
                                </span>
                              )}
                              {tunnels.filter((t) => t.hostId === host.id && t.status === 'active').length > 0 && (
                                <span className="text-[9px] px-1 py-0.2 rounded bg-[#007ACC]/20 text-[#4EC9B0] border border-[#007ACC]/40 font-mono">
                                  {tunnels.filter((t) => t.hostId === host.id && t.status === 'active').length} Tunnel
                                </span>
                              )}
                            </div>
                          </div>

                          {/* Quick Action Button */}
                          <div className="relative">
                            <button
                              onClick={(e) => {
                                e.stopPropagation();
                                setContextMenuHostId(contextMenuHostId === host.id ? null : host.id);
                              }}
                              className="p-1 text-[#6D6D6D] hover:text-[#D4D4D4] rounded hover:bg-[#3C3C3C] transition-colors"
                            >
                              <MoreVertical className="w-3.5 h-3.5" />
                            </button>

                            {contextMenuHostId === host.id && (
                              <div
                                onClick={(e) => e.stopPropagation()}
                                className="absolute right-0 top-6 w-44 bg-[#252526] border border-[#454545] rounded shadow-2xl py-1 z-50 text-xs font-sans text-[#D4D4D4]"
                              >
                                <button
                                  onClick={() => {
                                    onConnectHost(host, 'terminal');
                                    setContextMenuHostId(null);
                                  }}
                                  className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] hover:text-[#4EC9B0] text-left"
                                >
                                  <Terminal className="w-3.5 h-3.5 text-[#4EC9B0]" />
                                  <span>Open Terminal</span>
                                </button>
                                <button
                                  onClick={() => {
                                    onConnectHost(host, 'sftp');
                                    setContextMenuHostId(null);
                                  }}
                                  className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] hover:text-[#4EC9B0] text-left"
                                >
                                  <FolderSync className="w-3.5 h-3.5 text-[#CE9178]" />
                                  <span>Connect with SFTP</span>
                                </button>
                                <button
                                  onClick={() => {
                                    onConnectHost(host, 'monitor');
                                    setContextMenuHostId(null);
                                  }}
                                  className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] hover:text-[#4EC9B0] text-left"
                                >
                                  <Activity className="w-3.5 h-3.5 text-[#27C93F]" />
                                  <span>Connect with Top</span>
                                </button>
                                <button
                                  onClick={() => {
                                    onConnectHost(host, 'tunnels');
                                    setContextMenuHostId(null);
                                  }}
                                  className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] hover:text-[#4EC9B0] text-left"
                                >
                                  <Network className="w-3.5 h-3.5 text-[#4EC9B0]" />
                                  <span>Port Forwarding (Alt+P)</span>
                                </button>
                                <hr className="my-1 border-[#333333]" />
                                <button
                                  onClick={() => {
                                    onEditHost(host);
                                    setContextMenuHostId(null);
                                  }}
                                  className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
                                >
                                  <Edit2 className="w-3.5 h-3.5 text-[#569CD6]" />
                                  <span>Edit Configuration</span>
                                </button>
                                <button
                                  onClick={() => {
                                    onDuplicateHost(host);
                                    setContextMenuHostId(null);
                                  }}
                                  className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
                                >
                                  <Copy className="w-3.5 h-3.5 text-[#858585]" />
                                  <span>Duplicate Node</span>
                                </button>
                                <button
                                  onClick={() => {
                                    onDeleteHost(host.id);
                                    setContextMenuHostId(null);
                                  }}
                                  className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#FF5F56]/20 hover:text-[#FF5F56] text-[#FF5F56] text-left"
                                >
                                  <Trash2 className="w-3.5 h-3.5" />
                                  <span>Delete Node</span>
                                </button>
                              </div>
                            )}
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Bottom Quick Add Node Button */}
      <div className="p-2 border-t border-[#333333] bg-[#1E1E1E]">
        <button
          onClick={onAddNewHost}
          className="w-full flex items-center justify-center gap-1.5 py-1.5 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs font-medium transition-colors"
        >
          <Plus className="w-3.5 h-3.5" />
          <span>New Server Host</span>
        </button>
      </div>
    </div>
  );
};

/* =========================================================================
   2. DEVOPS SNIPPETS PANEL VIEW
   ========================================================================= */
const SnippetsPanelContent: React.FC<{
  snippets: DevOpsSnippet[];
  onUpdateSnippets: (snippets: DevOpsSnippet[]) => void;
  onExecuteSnippet: (command: string) => void;
  onAddNewSnippet: () => void;
  onEditSnippet: (snippet: DevOpsSnippet) => void;
  onDeleteSnippet: (snippetId: string) => void;
}> = ({
  snippets,
  onUpdateSnippets,
  onExecuteSnippet,
  onAddNewSnippet,
  onEditSnippet,
  onDeleteSnippet,
}) => {
  const [search, setSearch] = useState('');
  const [category, setCategory] = useState('ALL');
  const [copiedId, setCopiedId] = useState<string | null>(null);

  // Parameter Prompt Modal state
  const [paramSnippet, setParamSnippet] = useState<DevOpsSnippet | null>(null);
  const [paramValues, setParamValues] = useState<Record<string, string>>({});

  const categories = [
    'ALL',
    'Docker & K8s',
    'System & Maintenance',
    'Nginx & Web',
    'Database & Storage',
    'Security & Firewall',
    'Network & Diagnostics',
    'Git & DevOps',
  ];

  const filtered = snippets.filter((s) => {
    const matchCat = category === 'ALL' || s.category === category;
    const matchQuery =
      s.title.toLowerCase().includes(search.toLowerCase()) ||
      s.command.toLowerCase().includes(search.toLowerCase()) ||
      (s.description && s.description.toLowerCase().includes(search.toLowerCase())) ||
      s.tags.some((t) => t.toLowerCase().includes(search.toLowerCase()));
    return matchCat && matchQuery;
  });

  const handleCopy = (id: string, cmd: string) => {
    navigator.clipboard.writeText(cmd);
    setCopiedId(id);
    setTimeout(() => setCopiedId(null), 1500);
  };

  const handleRunClick = (snippet: DevOpsSnippet) => {
    // Check if command has {{variables}}
    const varMatches = snippet.command.match(/\{\{([a-zA-Z0-9_]+)\}\}/g);
    if (varMatches && varMatches.length > 0) {
      const uniqueVars = Array.from(new Set(varMatches.map((m) => m.replace(/\{\{|\}\}/g, ''))));
      const initial: Record<string, string> = {};
      uniqueVars.forEach((v) => {
        const found = snippet.variables?.find((sv) => sv.name === v);
        initial[v] = found?.default || (v.includes('port') ? '8080' : v.includes('lines') ? '100' : '');
      });
      setParamValues(initial);
      setParamSnippet(snippet);
    } else {
      onExecuteSnippet(snippet.command);
    }
  };

  const handleExecuteWithParams = () => {
    if (!paramSnippet) return;
    let finalCommand = paramSnippet.command;
    Object.entries(paramValues).forEach(([k, v]) => {
      finalCommand = finalCommand.replace(new RegExp(`\\{\\{${k}\\}\\}`, 'g'), v);
    });
    onExecuteSnippet(finalCommand);
    setParamSnippet(null);
  };

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* Top Search & Filter Bar */}
      <div className="p-2 border-b border-[#333333] space-y-2 bg-[#1E1E1E]">
        <div className="flex items-center gap-1.5">
          <div className="relative flex-1">
            <Search className="w-3 h-3 text-[#858585] absolute left-2.5 top-2.5" />
            <input
              type="text"
              placeholder="Search snippets, tags & commands..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="w-full pl-7 pr-2 py-1 text-xs bg-[#252526] border border-[#3C3C3C] rounded text-[#D4D4D4] placeholder-[#777777] focus:outline-none focus:border-[#007ACC]"
            />
          </div>
          <button
            onClick={onAddNewSnippet}
            title="Create New Command Snippet"
            className="flex items-center gap-1 px-2.5 py-1 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs font-semibold transition-colors shrink-0 shadow-xs"
          >
            <Plus className="w-3 h-3" />
            <span>New</span>
          </button>
        </div>

        {/* Category Filter Pills */}
        <div className="flex items-center gap-1 overflow-x-auto pb-0.5 scrollbar-none text-[10px]">
          {categories.map((cat) => (
            <button
              key={cat}
              onClick={() => setCategory(cat)}
              className={`px-2 py-0.5 rounded whitespace-nowrap transition-colors ${
                category === cat
                  ? 'bg-[#007ACC]/25 text-[#4EC9B0] font-medium border border-[#007ACC]/60'
                  : 'text-[#858585] hover:text-[#D4D4D4] bg-[#252526] border border-[#333333]'
              }`}
            >
              {cat}
            </button>
          ))}
        </div>
      </div>

      {/* Snippet Items List */}
      <div className="flex-1 overflow-y-auto p-2 space-y-2">
        {filtered.length === 0 ? (
          <div className="h-48 flex flex-col items-center justify-center text-center p-4 space-y-2 border border-dashed border-[#333333] rounded-lg bg-[#181818] my-2">
            <Code2 className="w-7 h-7 text-[#6D6D6D]" />
            <p className="text-xs text-[#858585]">
              {search ? 'No snippets match your search criteria' : 'No snippets in this category yet'}
            </p>
            <button
              onClick={onAddNewSnippet}
              className="flex items-center gap-1 px-3 py-1 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs font-medium transition-colors"
            >
              <Plus className="w-3 h-3" />
              <span>Create Command Snippet</span>
            </button>
          </div>
        ) : (
          filtered.map((snippet) => (
            <div
              key={snippet.id}
              className="p-2.5 rounded bg-[#1E1E1E] hover:bg-[#252526] border border-[#333333] hover:border-[#454545] transition-all space-y-1.5 group"
            >
              {/* Card Header: Title & Category */}
              <div className="flex items-center justify-between gap-1">
                <span className="font-medium text-xs text-[#D4D4D4] group-hover:text-[#4EC9B0] transition-colors truncate">
                  {snippet.title}
                </span>
                <span className="text-[9px] px-1.5 py-0.2 rounded bg-[#2D2D2D] text-[#858585] border border-[#454545] font-mono shrink-0">
                  {snippet.category}
                </span>
              </div>

              {/* Description if present */}
              {snippet.description && (
                <p className="text-[10px] text-[#858585] line-clamp-1">
                  {snippet.description}
                </p>
              )}

              {/* Command Code Box */}
              <div className="bg-[#121212] p-1.5 rounded border border-[#2D2D2D] text-[10.5px] font-mono text-[#4EC9B0] truncate select-all">
                {snippet.command}
              </div>

              {/* Tags & Action Buttons */}
              <div className="flex items-center justify-between pt-0.5 text-[11px]">
                <div className="flex items-center gap-1 overflow-hidden">
                  {snippet.tags.slice(0, 3).map((t) => (
                    <span key={t} className="text-[9px] text-[#6D6D6D] truncate">
                      #{t}
                    </span>
                  ))}
                </div>

                <div className="flex items-center gap-1 shrink-0">
                  {/* Copy Button */}
                  <button
                    onClick={() => handleCopy(snippet.id, snippet.command)}
                    title="Copy command to clipboard"
                    className="p-1 hover:bg-[#3C3C3C] text-[#858585] hover:text-white rounded transition-colors"
                  >
                    {copiedId === snippet.id ? (
                      <Check className="w-3.5 h-3.5 text-[#27C93F]" />
                    ) : (
                      <Copy className="w-3.5 h-3.5" />
                    )}
                  </button>

                  {/* Edit Snippet Button */}
                  <button
                    onClick={() => onEditSnippet(snippet)}
                    title="Edit snippet details"
                    className="p-1 hover:bg-[#3C3C3C] text-[#858585] hover:text-[#9CDCFE] rounded transition-colors"
                  >
                    <Edit2 className="w-3.5 h-3.5" />
                  </button>

                  {/* Delete Snippet Button */}
                  <button
                    onClick={() => onDeleteSnippet(snippet.id)}
                    title="Delete snippet"
                    className="p-1 hover:bg-[#3C3C3C] text-[#858585] hover:text-[#FF5F56] rounded transition-colors"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>

                  {/* Run in Terminal Button */}
                  <button
                    onClick={() => handleRunClick(snippet)}
                    title="Execute in active terminal session"
                    className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#007ACC]/20 hover:bg-[#007ACC] text-[#4EC9B0] hover:text-white border border-[#007ACC]/40 transition-colors text-[10px] font-semibold ml-0.5"
                  >
                    <Play className="w-3 h-3 fill-current" />
                    <span>Run</span>
                  </button>
                </div>
              </div>
            </div>
          ))
        )}
      </div>

      {/* Bottom Quick Add Snippet Action */}
      <div className="p-2 border-t border-[#333333] bg-[#1E1E1E]">
        <button
          onClick={onAddNewSnippet}
          className="w-full flex items-center justify-center gap-1.5 py-1.5 bg-[#2D2D2D] hover:bg-[#3C3C3C] border border-[#3C3C3C] text-[#D4D4D4] hover:text-white rounded text-xs font-medium transition-colors"
        >
          <Plus className="w-3.5 h-3.5 text-[#4EC9B0]" />
          <span>New Command Snippet</span>
        </button>
      </div>

      {/* Parameterized Variable Prompt Dialog */}
      {paramSnippet && (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/70 backdrop-blur-xs animate-in fade-in duration-100">
          <div className="w-full max-w-sm bg-[#1E1E1E] border border-[#3C3C3C] rounded-xl shadow-2xl overflow-hidden p-4 space-y-3 font-sans">
            <div className="flex items-center justify-between border-b border-[#333333] pb-2">
              <div>
                <h4 className="text-xs font-semibold text-[#D4D4D4]">
                  Provide Parameters: {paramSnippet.title}
                </h4>
                <p className="text-[10px] text-[#858585]">
                  Fill dynamic variables before running in terminal
                </p>
              </div>
              <button
                onClick={() => setParamSnippet(null)}
                className="text-[#858585] hover:text-white p-1 rounded"
              >
                <X className="w-4 h-4" />
              </button>
            </div>

            <div className="space-y-2 text-xs">
              {Object.keys(paramValues).map((key) => (
                <div key={key} className="space-y-0.5">
                  <label className="block text-[10.5px] font-mono text-[#4EC9B0]">
                    &#123;&#123;{key}&#125;&#125;
                  </label>
                  <input
                    type="text"
                    value={paramValues[key]}
                    onChange={(e) =>
                      setParamValues({ ...paramValues, [key]: e.target.value })
                    }
                    placeholder={`Enter ${key}...`}
                    className="w-full px-2 py-1 bg-[#252526] border border-[#3C3C3C] rounded text-[#D4D4D4] font-mono text-xs focus:outline-none focus:border-[#007ACC]"
                  />
                </div>
              ))}
            </div>

            <div className="bg-[#121212] p-2 rounded border border-[#2D2D2D] text-[10.5px] font-mono text-[#DCDCAA] break-all">
              {(() => {
                let preview = paramSnippet.command;
                Object.entries(paramValues).forEach(([k, v]) => {
                  preview = preview.replace(
                    new RegExp(`\\{\\{${k}\\}\\}`, 'g'),
                    v || `{{${k}}}`
                  );
                });
                return preview;
              })()}
            </div>

            <div className="flex items-center justify-end gap-2 pt-1">
              <button
                onClick={() => setParamSnippet(null)}
                className="px-2.5 py-1 bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#CCCCCC] rounded text-xs transition-colors"
              >
                Cancel
              </button>
              <button
                onClick={handleExecuteWithParams}
                className="flex items-center gap-1 px-3 py-1 bg-[#007ACC] hover:bg-[#007ACC]/80 text-white rounded text-xs font-semibold transition-colors"
              >
                <Play className="w-3 h-3 fill-current" />
                <span>Run in Terminal</span>
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

/* =========================================================================
   3. SSH KEYS PANEL VIEW
   ========================================================================= */
const KeysPanelContent: React.FC<{
  keys: SSHKeyItem[];
  onUpdateKeys: (keys: SSHKeyItem[]) => void;
}> = ({ keys, onUpdateKeys }) => {
  const [copiedId, setCopiedId] = useState<string | null>(null);

  const handleCopyPub = (id: string, pub: string) => {
    navigator.clipboard.writeText(pub);
    setCopiedId(id);
    setTimeout(() => setCopiedId(null), 1500);
  };

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      <div className="p-2 border-b border-[#333333] bg-[#1E1E1E] flex items-center justify-between text-xs">
        <span className="text-[#858585] text-[11px]">SSH Keypairs Vault</span>
        <span className="text-[10px] px-1.5 py-0.5 rounded bg-[#2D2D2D] text-[#CCCCCC] font-mono border border-[#3C3C3C]">
          {keys.length} Keys
        </span>
      </div>

      <div className="flex-1 overflow-y-auto p-2 space-y-2">
        {keys.map((k) => (
          <div
            key={k.id}
            className="p-2.5 rounded bg-[#1E1E1E] hover:bg-[#2A2D2E] border border-[#333333] transition-all space-y-1.5 group"
          >
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-1.5">
                <KeyRound className="w-3.5 h-3.5 text-[#CE9178]" />
                <span className="font-medium text-xs text-[#D4D4D4] group-hover:text-white">
                  {k.name}
                </span>
              </div>
              <span className="text-[9px] px-1.5 py-0.2 rounded bg-[#2D2D2D] text-[#4EC9B0] border border-[#454545] font-mono uppercase">
                {k.type}
              </span>
            </div>

            <div className="text-[10px] font-mono text-[#858585] truncate bg-[#121212] px-2 py-1 rounded border border-[#2D2D2D]">
              {k.fingerprint || 'SHA256:4kL89/Nmd39Vxa1028XwQ'}
            </div>

            <div className="flex items-center justify-between text-[10px] text-[#6D6D6D]">
              <span>Bit length: {k.bits || 4096}</span>
              <button
                onClick={() => handleCopyPub(k.id, k.publicKey)}
                className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#CCCCCC] border border-[#3C3C3C] transition-colors"
              >
                {copiedId === k.id ? (
                  <Check className="w-3 h-3 text-[#27C93F]" />
                ) : (
                  <Copy className="w-3 h-3" />
                )}
                <span>Copy PubKey</span>
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
};

/* =========================================================================
   5. FLEET TELEMETRY & HEALTH PANEL VIEW
   ========================================================================= */
const TelemetryPanelContent: React.FC<{
  hosts: ServerHost[];
  onConnectHost: (host: ServerHost, tabType?: TabType) => void;
}> = ({ hosts, onConnectHost }) => {
  const onlineCount = hosts.filter((h) => h.status === 'online').length;

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* Fleet Stats Overview Card */}
      <div className="p-2.5 border-b border-[#333333] bg-[#1E1E1E] grid grid-cols-2 gap-2 text-xs">
        <div className="bg-[#252526] p-2 rounded border border-[#333333]">
          <div className="text-[#858585] text-[10px]">Fleet Availability</div>
          <div className="text-sm font-bold text-[#27C93F] font-mono mt-0.5">
            {Math.round((onlineCount / (hosts.length || 1)) * 100)}%
          </div>
          <div className="text-[9px] text-[#6D6D6D]">{onlineCount} / {hosts.length} reachable</div>
        </div>

        <div className="bg-[#252526] p-2 rounded border border-[#333333]">
          <div className="text-[#858585] text-[10px]">Average Latency</div>
          <div className="text-sm font-bold text-[#4EC9B0] font-mono mt-0.5">
            18.4 <span className="text-[10px] font-normal">ms</span>
          </div>
          <div className="text-[9px] text-[#6D6D6D]">Global Ping Jitter OK</div>
        </div>
      </div>

      {/* Fleet Node Health List */}
      <div className="flex-1 overflow-y-auto p-2 space-y-2">
        {hosts.map((host) => {
          const isOnline = host.status === 'online';
          return (
            <div
              key={host.id}
              onClick={() => onConnectHost(host, 'monitor')}
              className="p-2.5 rounded bg-[#1E1E1E] hover:bg-[#2A2D2E] border border-[#333333] transition-all cursor-pointer space-y-1.5 group"
            >
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-1.5 truncate">
                  <span
                    className={`w-2 h-2 rounded-full shrink-0 ${
                      isOnline ? 'bg-[#27C93F]' : 'bg-[#FF5F56]'
                    }`}
                  />
                  <span className="font-medium text-xs text-[#D4D4D4] truncate group-hover:text-[#4EC9B0]">
                    {host.name}
                  </span>
                </div>
                <span className="text-[9px] text-[#27C93F] font-mono">
                  {host.pingMs ? `${host.pingMs}ms` : '12ms'}
                </span>
              </div>

              {/* Mini Health Bars */}
              <div className="space-y-1 text-[10px]">
                <div className="flex items-center justify-between text-[#858585]">
                  <span>CPU Load</span>
                  <span className="font-mono text-[#D4D4D4]">24%</span>
                </div>
                <div className="w-full h-1 bg-[#2D2D2D] rounded-full overflow-hidden">
                  <div className="h-full bg-[#4EC9B0] rounded-full" style={{ width: '24%' }} />
                </div>
              </div>

              <div className="flex items-center justify-between text-[10px] text-[#6D6D6D] pt-0.5">
                <span>{host.osType}</span>
                <span className="text-[#007ACC] group-hover:underline">Open Top Monitor &rarr;</span>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
};
