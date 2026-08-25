import React, { useState, useRef, useEffect } from 'react';
import {
  Terminal as TerminalIcon,
  X,
  Plus,
  Maximize2,
  Minimize2,
  SplitSquareVertical,
  SplitSquareHorizontal,
  Trash2,
  Copy,
  Sparkles,
  FolderSync,
  Activity,
  RotateCw,
  CornerDownLeft,
  Search,
  Download,
  Check,
  AlertTriangle,
  Play,
  ChevronUp,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Folder,
  Network,
} from 'lucide-react';
import {
  SessionTab,
  TerminalPane,
  ServerHost,
  AppSettings,
  SFTPFile,
  TerminalLine,
  PortTunnel,
} from '../types';
import { executeTerminalCommand } from '../services/terminalEngine';
import { SessionBottomSFTPPanel } from './SessionBottomSFTPPanel';
import { SessionRightMonitorPanel } from './SessionRightMonitorPanel';
import { SessionPortForwardModal } from './SessionPortForwardModal';
import { SessionStatusBar } from './SessionStatusBar';

interface TerminalViewProps {
  tabs: SessionTab[];
  activeTabId: string;
  onSelectTab: (tabId: string) => void;
  onCloseTab: (tabId: string) => void;
  onNewTab: (hostId: string) => void;
  hosts: ServerHost[];
  settings: AppSettings;
  sftpFiles: Record<string, SFTPFile[]>;
  onUpdateSftpFiles?: (updated: Record<string, SFTPFile[]>) => void;
  onOpenFileEditor: (file: { path: string; name: string; content: string }) => void;
  onSwitchToSftp: (hostId: string) => void;
  onSwitchToMonitor: (hostId: string) => void;
  onAskCopilot: (contextText: string, taskType: string) => void;
  onUpdateTabs: (updatedTabs: SessionTab[]) => void;
  tunnels?: PortTunnel[];
  onUpdateTunnels?: (tunnels: PortTunnel[]) => void;
}

export const TerminalView: React.FC<TerminalViewProps> = ({
  tabs,
  activeTabId,
  onSelectTab,
  onCloseTab,
  onNewTab,
  hosts,
  settings,
  sftpFiles,
  onUpdateSftpFiles,
  onOpenFileEditor,
  onSwitchToSftp,
  onSwitchToMonitor,
  onAskCopilot,
  onUpdateTabs,
  tunnels = [],
  onUpdateTunnels,
}) => {
  const activeTab = tabs.find((t) => t.id === activeTabId) || tabs[0];
  const [isTunnelModalOpen, setIsTunnelModalOpen] = useState(false);
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    paneId: string;
    selectedText?: string;
  } | null>(null);

  // Focus and input state
  const [inputValues, setInputValues] = useState<Record<string, string>>({});
  const inputRefs = useRef<Record<string, HTMLInputElement | null>>({});
  const terminalEndRefs = useRef<Record<string, HTMLDivElement | null>>({});

  // Auto-scroll when new output lines arrive
  useEffect(() => {
    if (!activeTab) return;
    activeTab.panes.forEach((pane) => {
      const endEl = terminalEndRefs.current[pane.id];
      if (endEl) {
        endEl.scrollIntoView({ behavior: 'smooth' });
      }
    });
  }, [activeTab?.panes]);

  // Click outside closes context menu
  useEffect(() => {
    const handleGlobalClick = () => setContextMenu(null);
    window.addEventListener('click', handleGlobalClick);
    return () => window.removeEventListener('click', handleGlobalClick);
  }, []);

  // Keyboard shortcuts: Alt+F for SFTP panel, Alt+M for Top Monitor panel, Alt+P for Port Forwarding
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.altKey && (e.key === 'f' || e.key === 'F')) {
        e.preventDefault();
        handleToggleSftpPanel();
      } else if (e.altKey && (e.key === 'm' || e.key === 'M' || e.key === 't' || e.key === 'T')) {
        e.preventDefault();
        handleToggleMonitorPanel();
      } else if (e.altKey && (e.key === 'p' || e.key === 'P')) {
        e.preventDefault();
        setIsTunnelModalOpen((prev) => !prev);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [activeTab?.id, activeTab?.sftpOpen, activeTab?.monitorOpen]);

  if (!activeTab) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center bg-zinc-950 text-zinc-400 select-none">
        <div className="w-16 h-16 rounded-2xl bg-zinc-900 border border-zinc-800 flex items-center justify-center text-cyan-400 mb-4 shadow-xl">
          <TerminalIcon className="w-8 h-8" />
        </div>
        <h3 className="text-base font-medium text-zinc-200">No Active Sessions</h3>
        <p className="text-xs text-zinc-500 mt-1 max-w-sm text-center">
          Select a server node from the left inventory or click Quick Connect to start an SSH session.
        </p>
        <button
          onClick={() => hosts[0] && onNewTab(hosts[0].id)}
          className="mt-4 px-3.5 py-1.5 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg text-xs font-medium transition-all shadow-md shadow-cyan-950 flex items-center gap-1.5"
        >
          <Plus className="w-3.5 h-3.5" />
          <span>Connect to {hosts[0]?.name || 'Server'}</span>
        </button>
      </div>
    );
  }

  const activeHost = hosts.find((h) => h.id === activeTab.hostId) || hosts[0];
  const activePane = activeTab.panes.find((p) => p.id === activeTab.activePaneId) || activeTab.panes[0];
  const isSftpOpen = activeTab.sftpOpen !== false; // Default to open in session
  const isMonitorOpen = !!activeTab.monitorOpen;

  const activeHostTunnels = tunnels.filter((t) => t.hostId === activeHost.id);
  const activeHostTunnelsCount = activeHostTunnels.filter((t) => t.status === 'active').length;

  const handleUpdateTabState = (tabId: string, updates: Partial<SessionTab>) => {
    const updated = tabs.map((t) => (t.id === tabId ? { ...t, ...updates } : t));
    onUpdateTabs(updated);
  };

  const handleToggleSftpPanel = () => {
    handleUpdateTabState(activeTab.id, { sftpOpen: !isSftpOpen });
  };

  const handleToggleMonitorPanel = () => {
    handleUpdateTabState(activeTab.id, { monitorOpen: !isMonitorOpen });
  };

  const handleSendCommand = (paneId: string, customCmd?: string) => {
    const pane = activeTab.panes.find((p) => p.id === paneId);
    if (!pane) return;

    const commandText = customCmd !== undefined ? customCmd : inputValues[paneId] || '';
    if (!commandText.trim()) return;

    // Determine target panes: if broadcastMode is on, send to all panes in this tab
    const targetPanes = settings.broadcastMode ? activeTab.panes : [pane];

    const updatedTabs = tabs.map((t) => {
      if (t.id !== activeTab.id) return t;

      const newPanes = t.panes.map((p) => {
        if (!targetPanes.some((tp) => tp.id === p.id)) return p;

        const host = hosts.find((h) => h.id === p.hostId) || activeHost;
        const promptLine: TerminalLine = {
          id: `cmd-${Date.now()}-${Math.random()}`,
          text: commandText,
          type: 'command',
          timestamp: new Date().toLocaleTimeString(),
          promptInfo: {
            user: host.username,
            host: host.name.toLowerCase(),
            cwd: p.cwd,
          },
        };

        const result = executeTerminalCommand(commandText, host, p.cwd, sftpFiles);

        // Check for special action triggers
        if (result.action === 'clear') {
          return {
            ...p,
            history: [],
            commandHistory: [...p.commandHistory, commandText],
            historyIdx: p.commandHistory.length + 1,
          };
        }

        if (result.action === 'open_editor' && result.editorFile) {
          onOpenFileEditor(result.editorFile);
        } else if (result.action === 'open_sftp') {
          // Open bottom SFTP panel for this session
          t.sftpOpen = true;
        } else if (result.action === 'open_monitor') {
          // Open right monitor panel for this session
          t.monitorOpen = true;
        }

        const newOutputLines: TerminalLine[] = result.lines.map((l) => ({
          ...l,
          id: `out-${Date.now()}-${Math.random()}`,
          timestamp: new Date().toLocaleTimeString(),
        }));

        const newCwd = result.newCwd || p.cwd;

        // If follow CWD is active on the tab, update the tab's SFTP remote path
        if (t.sftpFollowTerminalCwd !== false && newCwd !== p.cwd) {
          t.sftpRemotePath = newCwd;
        }

        return {
          ...p,
          cwd: newCwd,
          history: [...p.history, promptLine, ...newOutputLines],
          commandHistory: [...p.commandHistory, commandText],
          historyIdx: p.commandHistory.length + 1,
        };
      });

      return { ...t, panes: newPanes };
    });

    onUpdateTabs(updatedTabs);
    setInputValues((prev) => ({ ...prev, [paneId]: '' }));
  };

  const handleKeyDown = (
    e: React.KeyboardEvent<HTMLInputElement>,
    pane: TerminalPane
  ) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSendCommand(pane.id);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (pane.commandHistory.length === 0) return;
      const nextIdx = Math.max(0, pane.historyIdx - 1);
      const prevCmd = pane.commandHistory[nextIdx] || '';
      setInputValues((prev) => ({ ...prev, [pane.id]: prevCmd }));
      // Update pane history idx
      const updated = tabs.map((t) =>
        t.id === activeTab.id
          ? {
              ...t,
              panes: t.panes.map((p) =>
                p.id === pane.id ? { ...p, historyIdx: nextIdx } : p
              ),
            }
          : t
      );
      onUpdateTabs(updated);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      const nextIdx = Math.min(pane.commandHistory.length, pane.historyIdx + 1);
      const nextCmd = pane.commandHistory[nextIdx] || '';
      setInputValues((prev) => ({ ...prev, [pane.id]: nextCmd }));
      const updated = tabs.map((t) =>
        t.id === activeTab.id
          ? {
              ...t,
              panes: t.panes.map((p) =>
                p.id === pane.id ? { ...p, historyIdx: nextIdx } : p
              ),
            }
          : t
      );
      onUpdateTabs(updated);
    } else if (e.key === 'Tab') {
      e.preventDefault();
      // Simple autocomplete
      const current = inputValues[pane.id] || '';
      const suggestions = [
        'neofetch',
        'docker ps',
        'docker logs',
        'kubectl get pods',
        'systemctl status nginx',
        'cat /etc/nginx/nginx.conf',
        'cat /etc/hosts',
        'nano /root/docker-compose.yml',
        'tail -f /var/log/auth.log',
        'htop',
        'df -h',
        'free -h',
        'clear',
        'sftp',
      ];
      const match = suggestions.find((s) => s.startsWith(current) && s !== current);
      if (match) {
        setInputValues((prev) => ({ ...prev, [pane.id]: match }));
      }
    } else if (e.ctrlKey && e.key === 'l') {
      e.preventDefault();
      handleSendCommand(pane.id, 'clear');
    } else if (e.ctrlKey && e.key === 'c') {
      e.preventDefault();
      const promptLine: TerminalLine = {
        id: `sigint-${Date.now()}`,
        text: (inputValues[pane.id] || '') + '^C',
        type: 'error',
        timestamp: new Date().toLocaleTimeString(),
        promptInfo: {
          user: activeHost.username,
          host: activeHost.name.toLowerCase(),
          cwd: pane.cwd,
        },
      };
      const updated = tabs.map((t) =>
        t.id === activeTab.id
          ? {
              ...t,
              panes: t.panes.map((p) =>
                p.id === pane.id ? { ...p, history: [...p.history, promptLine] } : p
              ),
            }
          : t
      );
      onUpdateTabs(updated);
      setInputValues((prev) => ({ ...prev, [pane.id]: '' }));
    }
  };

  const handleExportLog = (pane: TerminalPane) => {
    const text = pane.history.map((l) => `[${l.timestamp}] ${l.text}`).join('\n');
    const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `winssh_${activeHost.name}_${Date.now()}.log`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="flex-1 flex flex-col bg-[#0F0F0F] overflow-hidden relative select-text">
      {/* Tab Navigation Header Bar */}
      <div className="h-8 bg-[#252526] border-b border-[#333333] flex items-center px-1 justify-between select-none shrink-0">
        <div className="flex items-center gap-1 overflow-x-auto scrollbar-none flex-1">
          {tabs.map((tab) => {
            const isTabActive = tab.id === activeTabId;
            const host = hosts.find((h) => h.id === tab.hostId) || activeHost;
            return (
              <div
                key={tab.id}
                onClick={() => onSelectTab(tab.id)}
                className={`group flex items-center gap-2 px-3 py-1 text-xs rounded-t cursor-pointer transition-all border-t-2 ${
                  isTabActive
                    ? 'bg-[#1E1E1E] text-white font-medium border-t-[#007ACC] border-x border-[#333333] shadow-sm'
                    : 'bg-[#2D2D2D] hover:bg-[#2A2D2E] text-[#858585] hover:text-[#D4D4D4] border-t-transparent'
                }`}
              >
                <span
                  className="w-2 h-2 rounded-full shrink-0"
                  style={{ backgroundColor: host.color || '#27C93F' }}
                />
                <span className="truncate max-w-[140px] font-mono text-[11px]">
                  {tab.title || host.name}
                </span>

                {/* Tab Close Button */}
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    onCloseTab(tab.id);
                  }}
                  className="opacity-0 group-hover:opacity-100 p-0.5 hover:bg-[#3C3C3C] rounded text-[#858585] hover:text-white transition-opacity"
                >
                  <X className="w-3 h-3" />
                </button>
              </div>
            );
          })}

          {/* New Tab Button */}
          <button
            onClick={() => onNewTab(activeHost.id)}
            title="Open New Terminal Tab"
            className="p-1 text-[#858585] hover:text-[#D4D4D4] hover:bg-[#3C3C3C] rounded transition-colors ml-1"
          >
            <Plus className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* Tab Right Controls: Host Ping, Bottom SFTP Panel, Port Forwarding & Right Top Monitor Panel Switchers */}
        <div className="flex items-center gap-1.5 pr-2 text-xs font-mono text-[#858585]">
          {/* Session Port Forwarding Button */}
          <button
            onClick={() => setIsTunnelModalOpen(true)}
            title={`Session Port Forwarding for ${activeHost.name} (Alt+P)`}
            className={`flex items-center gap-1 px-2 py-0.5 rounded border transition-all text-[11px] ${
              activeHostTunnelsCount > 0
                ? 'bg-[#007ACC]/20 text-[#4EC9B0] border-[#007ACC]/70 font-semibold shadow-xs'
                : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] border-[#333333] text-[#A0A0A0] hover:text-white'
            }`}
          >
            <Network
              className={`w-3 h-3 ${
                activeHostTunnelsCount > 0 ? 'text-[#4EC9B0]' : 'text-[#858585]'
              }`}
            />
            <span>Tunnels</span>
            {activeHostTunnelsCount > 0 ? (
              <span className="px-1 py-[0.5px] rounded-full bg-[#007ACC] text-white text-[9px] font-mono leading-none">
                {activeHostTunnelsCount}
              </span>
            ) : null}
          </button>

          {/* Bottom SFTP Panel Toggle Button */}
          <button
            onClick={handleToggleSftpPanel}
            title={isSftpOpen ? 'Hide Bottom SFTP Panel (Alt+F)' : 'Show Bottom SFTP Panel (Alt+F)'}
            className={`flex items-center gap-1 px-2.5 py-0.5 rounded border transition-all text-[11px] ${
              isSftpOpen
                ? 'bg-[#007ACC]/20 text-[#4EC9B0] border-[#007ACC]/70 font-semibold shadow-xs'
                : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] border-[#333333] text-[#CE9178]'
            }`}
          >
            <FolderSync className={`w-3 h-3 ${isSftpOpen ? 'text-[#4EC9B0]' : 'text-[#CE9178]'}`} />
            <span>SFTP Panel</span>
            {isSftpOpen ? (
              <ChevronDown className="w-3 h-3 text-[#4EC9B0]" />
            ) : (
              <ChevronUp className="w-3 h-3 text-[#858585]" />
            )}
          </button>

          {/* Right Top / Telemetry Monitor button (Follows active session host) */}
          <button
            onClick={handleToggleMonitorPanel}
            title={isMonitorOpen ? 'Hide Right Top Monitor (Alt+M)' : 'Show Right Top Monitor (Alt+M)'}
            className={`flex items-center gap-1.5 px-2.5 py-0.5 rounded border transition-all text-[11px] ${
              isMonitorOpen
                ? 'bg-[#27C93F]/20 text-[#27C93F] border-[#27C93F]/70 font-semibold shadow-xs'
                : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] border-[#333333] text-[#27C93F]'
            }`}
          >
            <Activity className={`w-3 h-3 text-[#27C93F] ${isMonitorOpen ? 'animate-pulse' : ''}`} />
            <span>Top Monitor</span>
            {isMonitorOpen ? (
              <ChevronRight className="w-3 h-3 text-[#27C93F]" />
            ) : (
              <ChevronLeft className="w-3 h-3 text-[#858585]" />
            )}
          </button>
        </div>
      </div>

      {/* Main Terminal Workspace Layout: Center Panes + Bottom SFTP Panel + Right Top Monitor Panel */}
      <div className="flex-1 flex overflow-hidden">
        {/* Center Area: Terminal Panes & Bottom SFTP Panel */}
        <div className="flex-1 flex flex-col overflow-hidden min-w-0">
          {/* Main Terminal Panes Container (Single or Split) */}
          <div
            className={`flex-1 flex overflow-hidden ${
              activeTab.split === 'vertical'
                ? 'flex-row'
                : activeTab.split === 'horizontal'
                ? 'flex-col'
                : 'flex-row'
            }`}
          >
        {activeTab.panes.map((pane, idx) => {
          const host = hosts.find((h) => h.id === pane.hostId) || activeHost;
          return (
            <div
              key={pane.id}
              onClick={() => {
                inputRefs.current[pane.id]?.focus();
                handleUpdateTabState(activeTab.id, { activePaneId: pane.id });
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                const sel = window.getSelection()?.toString();
                setContextMenu({
                  x: e.clientX,
                  y: e.clientY,
                  paneId: pane.id,
                  selectedText: sel,
                });
              }}
              className={`flex-1 flex flex-col bg-[#0F0F0F] overflow-hidden relative font-mono ${
                idx > 0
                  ? activeTab.split === 'vertical'
                    ? 'border-l border-[#333333]'
                    : 'border-t border-[#333333]'
                  : ''
              }`}
              style={{
                fontSize: `${settings.fontSize}px`,
                fontFamily: settings.fontFamily,
              }}
            >
              {/* Pane Top Sub-Bar */}
              <div className="h-6 bg-[#1E1E1E] border-b border-[#333333] px-2.5 flex items-center justify-between text-[11px] text-[#858585] select-none shrink-0">
                <div className="flex items-center gap-2">
                  <span className="w-1.5 h-1.5 rounded-full bg-[#27C93F] animate-pulse" />
                  <span className="text-[#D4D4D4] font-semibold">{host.username}@{host.host}</span>
                  <span className="text-[#454545]">|</span>
                  <span className="text-[#569CD6] truncate max-w-[240px]">{pane.cwd}</span>
                </div>

                <div className="flex items-center gap-2 text-[10px]">
                  <button
                    onClick={() => handleExportLog(pane)}
                    title="Export Terminal Session Log"
                    className="p-1 hover:text-[#4EC9B0] transition-colors"
                  >
                    <Download className="w-3 h-3" />
                  </button>
                  <button
                    onClick={() => handleSendCommand(pane.id, 'clear')}
                    title="Clear Screen (Ctrl+L)"
                    className="p-1 hover:text-[#4EC9B0] transition-colors"
                  >
                    <Trash2 className="w-3 h-3" />
                  </button>
                </div>
              </div>

              {/* Terminal Buffer Scroll Area */}
              <div className="flex-1 overflow-y-auto p-3 space-y-1">
                {/* Initial Welcome / Host Header */}
                {pane.history.length === 0 && (
                  <div className="text-[#6D6D6D] mb-2 space-y-0.5 text-xs">
                    <p className="text-[#4EC9B0] font-bold">
                      Connected to {host.name} ({host.host}:{host.port}) via OpenSSH 9.6p1
                    </p>
                    <p>Last login: {new Date().toUTCString()} from 192.168.1.50</p>
                    <p className="text-[#858585]">
                      Type <span className="text-[#D4D4D4] underline">help</span>, <span className="text-[#D4D4D4] underline">sftp</span>, or <span className="text-[#D4D4D4] underline">neofetch</span> to explore commands.
                    </p>
                  </div>
                )}

                {/* Render Command Output History */}
                {pane.history.map((line) => {
                  if (line.type === 'command' && line.promptInfo) {
                    return (
                      <div key={line.id} className="flex items-center gap-1.5 text-[#D4D4D4] flex-wrap">
                        <span className="text-[#4EC9B0] font-semibold">
                          {line.promptInfo.user}@{line.promptInfo.host}
                        </span>
                        <span className="text-[#6D6D6D]">:</span>
                        <span className="text-[#569CD6] font-medium">{line.promptInfo.cwd}</span>
                        <span className="text-[#858585] font-bold">$</span>
                        <span className="text-white font-semibold">{line.text}</span>
                      </div>
                    );
                  }

                  if (line.type === 'error') {
                    return (
                      <div key={line.id} className="text-[#FF5F56] whitespace-pre-wrap flex items-start gap-1">
                        <AlertTriangle className="w-3.5 h-3.5 text-[#FF5F56] shrink-0 mt-0.5" />
                        <span>{line.text}</span>
                      </div>
                    );
                  }

                  if (line.type === 'success') {
                    return (
                      <div key={line.id} className="text-[#27C93F] whitespace-pre-wrap flex items-start gap-1">
                        <Check className="w-3.5 h-3.5 text-[#27C93F] shrink-0 mt-0.5" />
                        <span>{line.text}</span>
                      </div>
                    );
                  }

                  if (line.type === 'system') {
                    return (
                      <div key={line.id} className="text-[#CE9178] italic text-xs whitespace-pre-wrap">
                        {line.text}
                      </div>
                    );
                  }

                  if (line.type === 'ascii') {
                    return (
                      <pre key={line.id} className="text-[#4EC9B0] whitespace-pre leading-snug font-mono">
                        {line.text}
                      </pre>
                    );
                  }

                  return (
                    <div key={line.id} className="text-[#D4D4D4] whitespace-pre-wrap leading-relaxed">
                      {line.text}
                    </div>
                  );
                })}

                {/* Active Interactive Input Prompt */}
                <div className="flex items-center gap-1.5 pt-1 text-[#D4D4D4]">
                  <span className="text-[#4EC9B0] font-semibold">
                    {host.username}@{host.name.toLowerCase()}
                  </span>
                  <span className="text-[#6D6D6D]">:</span>
                  <span className="text-[#569CD6] font-medium">{pane.cwd}</span>
                  <span className="text-[#858585] font-bold">$</span>
                  <div className="flex-1 relative flex items-center">
                    <input
                      ref={(el) => (inputRefs.current[pane.id] = el)}
                      type="text"
                      value={inputValues[pane.id] || ''}
                      onChange={(e) =>
                        setInputValues((prev) => ({ ...prev, [pane.id]: e.target.value }))
                      }
                      onKeyDown={(e) => handleKeyDown(e, pane)}
                      autoFocus
                      spellCheck={false}
                      autoComplete="off"
                      className="w-full bg-transparent text-white focus:outline-none caret-[#007ACC] font-mono"
                    />
                  </div>
                </div>

                <div ref={(el) => (terminalEndRefs.current[pane.id] = el)} />
              </div>

              {/* Bottom Session Status Bar (Host Latency, CPU Load, Mem, CWD, Diagnostics) */}
              <SessionStatusBar
                host={host}
                activePane={pane}
                activeTab={activeTab}
                isMonitorOpen={isMonitorOpen}
                isSftpOpen={isSftpOpen}
                activeTunnelsCount={activeHostTunnelsCount}
                onToggleMonitor={handleToggleMonitorPanel}
                onToggleSftp={handleToggleSftpPanel}
                onToggleTunnels={() => setIsTunnelModalOpen(true)}
                onSendQuickCommand={(cmd) => handleSendCommand(pane.id, cmd)}
              />
            </div>
          );
        })}
      </div>

      {/* Bottom SFTP Panel (Follows Active Session & Host) */}
      {isSftpOpen && (
        <SessionBottomSFTPPanel
          activeTab={activeTab}
          activeHost={activeHost}
          terminalCwd={activePane?.cwd || '/root'}
          sftpFiles={sftpFiles}
          onUpdateSftpFiles={onUpdateSftpFiles || (() => {})}
          onOpenFileEditor={onOpenFileEditor}
          onExecuteCommandInTerminal={(cmd) => handleSendCommand(activePane.id, cmd)}
          onClose={() => handleUpdateTabState(activeTab.id, { sftpOpen: false })}
          onUpdateTabState={handleUpdateTabState}
          settings={settings}
        />
      )}
        </div>

        {/* Right Top Monitor Panel (Follows Active Session & Host) */}
        {isMonitorOpen && (
          <SessionRightMonitorPanel
            host={activeHost}
            isOpen={isMonitorOpen}
            onClose={handleToggleMonitorPanel}
            width={activeTab.monitorWidth || 360}
            onResizeWidth={(w) => handleUpdateTabState(activeTab.id, { monitorWidth: w })}
            onSendTerminalCommand={(cmd) => handleSendCommand(activePane.id, cmd)}
          />
        )}
      </div>

      {/* Right Click Terminal Context Menu */}
      {contextMenu && (
        <div
          style={{ top: `${contextMenu.y}px`, left: `${contextMenu.x}px` }}
          className="fixed bg-[#252526] border border-[#454545] rounded shadow-2xl py-1 z-50 text-xs font-sans text-[#D4D4D4] w-56"
          onClick={(e) => e.stopPropagation()}
        >
          <button
            onClick={() => {
              if (contextMenu.selectedText) {
                navigator.clipboard.writeText(contextMenu.selectedText);
              }
              setContextMenu(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] hover:text-[#4EC9B0] text-left"
          >
            <Copy className="w-3.5 h-3.5 text-[#858585]" />
            <span>Copy Selection</span>
          </button>
          <button
            onClick={() => {
              navigator.clipboard.readText().then((clipText) => {
                if (clipText) {
                  setInputValues((prev) => ({
                    ...prev,
                    [contextMenu.paneId]: (prev[contextMenu.paneId] || '') + clipText,
                  }));
                }
              });
              setContextMenu(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] hover:text-[#4EC9B0] text-left"
          >
            <CornerDownLeft className="w-3.5 h-3.5 text-[#4EC9B0]" />
            <span>Paste to Prompt</span>
          </button>
          <hr className="my-1 border-[#333333]" />
          <button
            onClick={() => {
              setIsTunnelModalOpen(true);
              setContextMenu(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-[#4EC9B0] text-left"
          >
            <Network className="w-3.5 h-3.5 text-[#4EC9B0]" />
            <span>Port Forwarding... (Alt+P)</span>
          </button>
          <button
            onClick={() => {
              handleToggleMonitorPanel();
              setContextMenu(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-[#27C93F] text-left"
          >
            <Activity className="w-3.5 h-3.5 text-[#27C93F]" />
            <span>{isMonitorOpen ? 'Close Top Monitor' : 'Open Top Monitor (Alt+M)'}</span>
          </button>
          <button
            onClick={() => {
              handleToggleSftpPanel();
              setContextMenu(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-[#CE9178] text-left"
          >
            <FolderSync className="w-3.5 h-3.5 text-[#CE9178]" />
            <span>{isSftpOpen ? 'Close SFTP Panel' : 'Open SFTP Panel (Alt+F)'}</span>
          </button>
          <hr className="my-1 border-[#333333]" />
          <button
            onClick={() => {
              onAskCopilot(
                contextMenu.selectedText || 'Explain current server status and logs',
                'explain_command'
              );
              setContextMenu(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-[#C586C0] text-left font-medium"
          >
            <Sparkles className="w-3.5 h-3.5 text-[#C586C0]" />
            <span>AI Explain with Copilot</span>
          </button>
          <button
            onClick={() => {
              handleSendCommand(contextMenu.paneId, 'clear');
              setContextMenu(null);
            }}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-[#37373D] text-left"
          >
            <Trash2 className="w-3.5 h-3.5 text-[#858585]" />
            <span>Clear Buffer</span>
          </button>
        </div>
      )}

      {/* Session Port Forwarding Modal */}
      {isTunnelModalOpen && onUpdateTunnels && (
        <SessionPortForwardModal
          isOpen={isTunnelModalOpen}
          onClose={() => setIsTunnelModalOpen(false)}
          host={activeHost}
          tunnels={tunnels}
          onUpdateTunnels={onUpdateTunnels}
          onSendTerminalCommand={(cmd) => {
            handleSendCommand(activePane.id, cmd);
            setIsTunnelModalOpen(false);
          }}
        />
      )}
    </div>
  );
};
