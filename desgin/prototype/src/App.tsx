import React, { useState, useEffect } from 'react';
import { HeaderNavbar } from './components/HeaderNavbar';
import { ActionBar, ActionPanelId } from './components/ActionBar';
import { SidePanel } from './components/SidePanel';
import { TerminalView } from './components/TerminalView';
import { AICopilotDrawer } from './components/AICopilotDrawer';
import { HostManagerModal } from './components/HostManagerModal';
import { QuickConnectModal } from './components/QuickConnectModal';
import { InteractiveVimModal } from './components/InteractiveVimModal';
import { SettingsModal } from './components/SettingsModal';
import { CredentialVaultModal } from './components/CredentialVaultModal';
import { ClashSubscriptionModal } from './components/clash/ClashSubscriptionModal';
import { ClashMetaDetailView } from './components/clash/ClashMetaDetailView';
import {
  DEFAULT_HOSTS,
  DEFAULT_SNIPPETS,
  DEFAULT_KEYS,
  DEFAULT_TUNNELS,
  DEFAULT_CREDENTIALS,
  INITIAL_SFTP_FILES,
} from './data/defaultData';
import {
  DEFAULT_CLASH_SUBSCRIPTIONS,
  DEFAULT_CLASH_SETTINGS,
} from './data/defaultClashData';
import {
  ServerHost,
  SessionTab,
  DevOpsSnippet,
  SSHKeyItem,
  PortTunnel,
  SFTPFile,
  AppSettings,
  TabType,
  TerminalPane,
  VaultCredential,
  ClashSubscription,
  ClashCoreSettings,
} from './types';
import {
  Search,
  Terminal,
  FolderSync,
  Activity,
  Code2,
  Plus,
  Sparkles,
  Network,
  KeyRound,
  ShieldCheck,
  Layers,
  Zap,
  Radio,
} from 'lucide-react';

export function App() {
  // App State: Hosts & Vaults
  const [hosts, setHosts] = useState<ServerHost[]>(() => {
    const saved = localStorage.getItem('winssh_hosts');
    return saved ? JSON.parse(saved) : DEFAULT_HOSTS;
  });

  const [snippets, setSnippets] = useState<DevOpsSnippet[]>(() => {
    const saved = localStorage.getItem('winssh_snippets');
    return saved ? JSON.parse(saved) : DEFAULT_SNIPPETS;
  });

  const [keys, setKeys] = useState<SSHKeyItem[]>(() => {
    const saved = localStorage.getItem('winssh_keys');
    return saved ? JSON.parse(saved) : DEFAULT_KEYS;
  });

  const [credentials, setCredentials] = useState<VaultCredential[]>(() => {
    const saved = localStorage.getItem('winssh_vault_credentials');
    return saved ? JSON.parse(saved) : DEFAULT_CREDENTIALS;
  });

  const [tunnels, setTunnels] = useState<PortTunnel[]>(() => {
    const saved = localStorage.getItem('winssh_tunnels');
    return saved ? JSON.parse(saved) : DEFAULT_TUNNELS;
  });

  const [sftpFiles, setSftpFiles] = useState<Record<string, SFTPFile[]>>(INITIAL_SFTP_FILES);

  const [settings, setSettings] = useState<AppSettings>(() => {
    const saved = localStorage.getItem('winssh_settings');
    return saved
      ? JSON.parse(saved)
      : {
          theme: 'dark',
          fontSize: 13,
          fontFamily: "'JetBrains Mono', monospace",
          cursorStyle: 'block',
          cursorBlink: true,
          scrollbackLines: 5000,
          bellSound: true,
          sftpShowHidden: true,
          keepaliveInterval: 30,
          broadcastMode: false,
          activeOS: 'macos',
        };
  });

  // Action Bar & Side Panel State
  const [activePanel, setActivePanel] = useState<ActionPanelId>('hosts');
  const [isPanelOpen, setIsPanelOpen] = useState(true);
  const [panelWidth, setPanelWidth] = useState<number>(() => {
    const saved = localStorage.getItem('winssh_panel_width');
    return saved ? parseInt(saved, 10) : 280;
  });

  const [selectedHostId, setSelectedHostId] = useState<string>(hosts[0]?.id || 'host-k8s-prod');

  // Terminal Tabs
  const [tabs, setTabs] = useState<SessionTab[]>([
    {
      id: 'tab-1',
      title: 'US-East K8s Master',
      hostId: 'host-k8s-prod',
      type: 'terminal',
      panes: [
        {
          id: 'pane-1',
          hostId: 'host-k8s-prod',
          cwd: '/root',
          history: [],
          commandHistory: ['neofetch', 'docker ps'],
          historyIdx: 2,
        },
      ],
      activePaneId: 'pane-1',
      split: 'single',
      connectedAt: new Date().toLocaleTimeString(),
    },
  ]);
  const [activeTabId, setActiveTabId] = useState<string>('tab-1');

  // Modals & Drawers
  const [isCopilotOpen, setIsCopilotOpen] = useState(false);
  const [copilotExternalPrompt, setCopilotExternalPrompt] = useState<{
    text: string;
    taskType?: string;
  } | null>(null);

  const [showHostModal, setShowHostModal] = useState(false);
  const [editingHost, setEditingHost] = useState<ServerHost | null>(null);

  // Clash.Meta State
  const [clashSubscriptions, setClashSubscriptions] = useState<ClashSubscription[]>(() => {
    const saved = localStorage.getItem('winssh_clash_subscriptions');
    return saved ? JSON.parse(saved) : DEFAULT_CLASH_SUBSCRIPTIONS;
  });
  const [activeClashSubscriptionId, setActiveClashSubscriptionId] = useState<string>(
    clashSubscriptions[0]?.id || 'sub-bgp-fast'
  );
  const [clashSettings, setClashSettings] = useState<ClashCoreSettings>(() => {
    const saved = localStorage.getItem('winssh_clash_settings');
    return saved ? JSON.parse(saved) : DEFAULT_CLASH_SETTINGS;
  });
  const [showClashModal, setShowClashModal] = useState(false);
  const [editingClashSubscription, setEditingClashSubscription] = useState<ClashSubscription | null>(null);

  const [showVaultModal, setShowVaultModal] = useState(false);
  const [showQuickConnect, setShowQuickConnect] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showCommandPalette, setShowCommandPalette] = useState(false);

  // File Editor modal
  const [editorFile, setEditorFile] = useState<{
    path: string;
    name: string;
    content: string;
  } | null>(null);

  // Persist State to LocalStorage
  useEffect(() => {
    localStorage.setItem('winssh_hosts', JSON.stringify(hosts));
  }, [hosts]);

  useEffect(() => {
    localStorage.setItem('winssh_snippets', JSON.stringify(snippets));
  }, [snippets]);

  useEffect(() => {
    localStorage.setItem('winssh_keys', JSON.stringify(keys));
  }, [keys]);

  useEffect(() => {
    localStorage.setItem('winssh_vault_credentials', JSON.stringify(credentials));
  }, [credentials]);

  useEffect(() => {
    localStorage.setItem('winssh_tunnels', JSON.stringify(tunnels));
  }, [tunnels]);

  useEffect(() => {
    localStorage.setItem('winssh_settings', JSON.stringify(settings));
  }, [settings]);

  useEffect(() => {
    localStorage.setItem('winssh_panel_width', panelWidth.toString());
  }, [panelWidth]);

  useEffect(() => {
    localStorage.setItem('winssh_clash_subscriptions', JSON.stringify(clashSubscriptions));
  }, [clashSubscriptions]);

  useEffect(() => {
    localStorage.setItem('winssh_clash_settings', JSON.stringify(clashSettings));
  }, [clashSettings]);

  // Global Keyboard Shortcuts (Ctrl+K Command Palette, Alt+1-5, Ctrl+B, etc.)
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'k') {
        e.preventDefault();
        setShowCommandPalette((prev) => !prev);
      } else if ((e.ctrlKey || e.metaKey) && e.key === 'q') {
        e.preventDefault();
        setShowQuickConnect(true);
      } else if ((e.ctrlKey || e.metaKey) && e.key === 'i') {
        e.preventDefault();
        setIsCopilotOpen((prev) => !prev);
      } else if ((e.ctrlKey || e.metaKey || e.altKey) && (e.key === 'b' || e.key === 'B')) {
        e.preventDefault();
        setIsPanelOpen((prev) => !prev);
      } else if (e.altKey && e.key === '1') {
        e.preventDefault();
        setActivePanel('hosts');
        setIsPanelOpen(true);
      } else if (e.altKey && e.key === '2') {
        e.preventDefault();
        setActivePanel('snippets');
        setIsPanelOpen(true);
      } else if (e.altKey && e.key === '3') {
        e.preventDefault();
        setShowVaultModal(true);
      } else if (e.altKey && e.key === '4') {
        e.preventDefault();
        setActivePanel('clash');
        setIsPanelOpen(true);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  const handleSelectPanel = (panel: ActionPanelId) => {
    if (activePanel === panel && isPanelOpen) {
      setIsPanelOpen(false);
    } else {
      setActivePanel(panel);
      setIsPanelOpen(true);
    }
  };

  const activeHost = hosts.find((h) => h.id === selectedHostId) || hosts[0];

  // Tab Handlers
  const handleConnectHost = (host: ServerHost, tabType: TabType = 'terminal') => {
    setSelectedHostId(host.id);

    // Check if tab already exists for this host
    const existingTab = tabs.find((t) => t.hostId === host.id);
    if (existingTab) {
      setActiveTabId(existingTab.id);
      if (tabType === 'sftp') {
        setTabs((prev) =>
          prev.map((t) => (t.id === existingTab.id ? { ...t, sftpOpen: true } : t))
        );
      } else if (tabType === 'monitor') {
        setTabs((prev) =>
          prev.map((t) => (t.id === existingTab.id ? { ...t, monitorOpen: true } : t))
        );
      }
    } else {
      const newTabId = `tab-${Date.now()}`;
      const newTab: SessionTab = {
        id: newTabId,
        title: host.name,
        hostId: host.id,
        type: 'terminal',
        panes: [
          {
            id: `pane-${Date.now()}`,
            hostId: host.id,
            cwd: '/root',
            history: [],
            commandHistory: [],
            historyIdx: 0,
          },
        ],
        activePaneId: `pane-${Date.now()}`,
        split: 'single',
        connectedAt: new Date().toLocaleTimeString(),
        sftpOpen: tabType === 'sftp' || true,
        monitorOpen: tabType === 'monitor',
      };
      setTabs((prev) => [...prev, newTab]);
      setActiveTabId(newTabId);
    }
  };

  const handleNewTab = (hostId: string) => {
    const host = hosts.find((h) => h.id === hostId) || activeHost;
    const newTabId = `tab-${Date.now()}`;
    const newTab: SessionTab = {
      id: newTabId,
      title: `${host.name} #${tabs.length + 1}`,
      hostId: host.id,
      type: 'terminal',
      panes: [
        {
          id: `pane-${Date.now()}`,
          hostId: host.id,
          cwd: '/root',
          history: [],
          commandHistory: [],
          historyIdx: 0,
        },
      ],
      activePaneId: `pane-${Date.now()}`,
      split: 'single',
      connectedAt: new Date().toLocaleTimeString(),
    };
    setTabs((prev) => [...prev, newTab]);
    setActiveTabId(newTabId);
  };

  const handleCloseTab = (tabId: string) => {
    const remaining = tabs.filter((t) => t.id !== tabId);
    setTabs(remaining);
    if (activeTabId === tabId && remaining.length > 0) {
      setActiveTabId(remaining[remaining.length - 1].id);
    }
  };

  const handleSetSplit = (split: 'none' | 'vertical' | 'horizontal') => {
    setTabs((prevTabs) =>
      prevTabs.map((tab) => {
        if (tab.id !== activeTabId) return tab;
        let panes = [...tab.panes];
        if (split !== 'none' && panes.length === 1) {
          // Add a second pane for the split view
          const newPane: TerminalPane = {
            id: `pane-${Date.now()}`,
            hostId: tab.hostId,
            title: `${tab.title} (2)`,
            cwd: '/var/log',
            history: [
              {
                id: `init-${Date.now()}`,
                text: 'Secondary terminal session initialized in split pane mode.',
                type: 'system',
                timestamp: new Date().toLocaleTimeString(),
              },
            ],
            commandHistory: [],
            historyIdx: 0,
          };
          panes.push(newPane);
        } else if (split === 'none' && panes.length > 1) {
          panes = [panes[0]];
        }
        return {
          ...tab,
          split,
          panes,
        };
      })
    );
  };

  const handleExecuteSnippetInTerminal = (command: string) => {
    const activeTab = tabs.find((t) => t.id === activeTabId) || tabs[0];
    if (!activeTab) return;

    const activePane = activeTab.panes[0];
    if (!activePane) return;

    const host = hosts.find((h) => h.id === activePane.hostId) || activeHost;

    // Simulate sending snippet command
    const updatedTabs = tabs.map((t) => {
      if (t.id !== activeTab.id) return t;
      return {
        ...t,
        panes: t.panes.map((p) => {
          if (p.id !== activePane.id) return p;
          return {
            ...p,
            history: [
              ...p.history,
              {
                id: `cmd-${Date.now()}`,
                text: command,
                type: 'command' as const,
                timestamp: new Date().toLocaleTimeString(),
                promptInfo: {
                  user: host.username,
                  host: host.name.toLowerCase(),
                  cwd: p.cwd,
                },
              },
              {
                id: `res-${Date.now()}`,
                text: `[WinSSH Pro] Executed snippet: ${command}\nOperation completed successfully with return code 0.`,
                type: 'success' as const,
                timestamp: new Date().toLocaleTimeString(),
              },
            ],
            commandHistory: [...p.commandHistory, command],
          };
        }),
      };
    });
    setTabs(updatedTabs);
  };

  const handleSaveEditedFile = (path: string, newContent: string) => {
    // Find file in sftpFiles and update content
    const updated = { ...sftpFiles };
    Object.keys(updated).forEach((dir) => {
      updated[dir] = updated[dir].map((f) =>
        f.path === path ? { ...f, content: newContent, size: newContent.length } : f
      );
    });
    setSftpFiles(updated);
  };

  const handleQuickConnect = (connectionString: string) => {
    // Parse "user@host:port" or "ssh user@host"
    let clean = connectionString.replace(/^ssh\s+/, '').trim();
    let port = 22;
    if (clean.includes(' -p ')) {
      const parts = clean.split(' -p ');
      clean = parts[0];
      port = parseInt(parts[1], 10) || 22;
    } else if (clean.includes(':')) {
      const parts = clean.split(':');
      clean = parts[0];
      port = parseInt(parts[1], 10) || 22;
    }

    let username = 'root';
    let hostIp = clean;
    if (clean.includes('@')) {
      const parts = clean.split('@');
      username = parts[0];
      hostIp = parts[1];
    }

    const newHost: ServerHost = {
      id: `host-quick-${Date.now()}`,
      name: `Direct-${hostIp}`,
      host: hostIp,
      port,
      username,
      authType: 'password',
      group: 'Quick Connect',
      tags: ['Adhoc', 'Quick'],
      color: '#10b981',
      osType: 'ubuntu',
      status: 'online',
    };

    setHosts((prev) => [newHost, ...prev]);
    handleConnectHost(newHost, 'terminal');
  };

  return (
    <div className="h-screen w-screen flex flex-col bg-[#1E1E1E] text-[#D4D4D4] font-sans overflow-hidden select-none">
      {/* Top Application Navbar */}
      <HeaderNavbar
        activeHost={activeHost}
        onOpenQuickConnect={() => setShowQuickConnect(true)}
        onOpenSettings={() => setShowSettings(true)}
        onOpenVaultModal={() => setShowVaultModal(true)}
        onOpenCommandPalette={() => setShowCommandPalette(true)}
        isCopilotOpen={isCopilotOpen}
        onToggleCopilot={() => setIsCopilotOpen(!isCopilotOpen)}
        settings={settings}
        onUpdateSettings={setSettings}
        activeTab={tabs.find((t) => t.id === activeTabId)}
        onSetSplit={handleSetSplit}
        connectedCount={hosts.filter((h) => h.status === 'online').length}
        totalHostsCount={hosts.length}
        clashSettings={clashSettings}
        onOpenClash={() => {
          setActivePanel('clash');
          setIsPanelOpen(true);
        }}
      />

      {/* Main App Body */}
      <div className="flex-1 flex overflow-hidden relative">
        {/* Left Action Bar (Activity Rail) */}
        <ActionBar
          activePanel={activePanel}
          isPanelOpen={isPanelOpen}
          onSelectPanel={handleSelectPanel}
          onOpenQuickConnect={() => setShowQuickConnect(true)}
          onOpenSettings={() => setShowSettings(true)}
          onOpenVault={() => setShowVaultModal(true)}
          isCopilotOpen={isCopilotOpen}
          onToggleCopilot={() => setIsCopilotOpen(!isCopilotOpen)}
        />

        {/* Resizable / Collapsible Side Panel */}
        <SidePanel
          activePanel={activePanel}
          isOpen={isPanelOpen}
          onClose={() => setIsPanelOpen(false)}
          width={panelWidth}
          onResizeWidth={setPanelWidth}
          hosts={hosts}
          selectedHostId={selectedHostId}
          onConnectHost={handleConnectHost}
          onAddNewHost={() => {
            setEditingHost(null);
            setShowHostModal(true);
          }}
          onEditHost={(h) => {
            setEditingHost(h);
            setShowHostModal(true);
          }}
          onDeleteHost={(id) => setHosts(hosts.filter((h) => h.id !== id))}
          onDuplicateHost={(h) => {
            const dup: ServerHost = {
              ...h,
              id: `host-${Date.now()}`,
              name: `${h.name} (Copy)`,
            };
            setHosts([...hosts, dup]);
          }}
          snippets={snippets}
          onUpdateSnippets={setSnippets}
          onExecuteSnippet={handleExecuteSnippetInTerminal}
          tunnels={tunnels}
          onUpdateTunnels={setTunnels}
          keys={keys}
          onUpdateKeys={setKeys}
          credentials={credentials}
          onUpdateCredentials={setCredentials}
          onOpenVaultModal={() => setShowVaultModal(true)}
          clashSubscriptions={clashSubscriptions}
          activeClashSubscriptionId={activeClashSubscriptionId}
          onSelectClashSubscription={(id) => {
            setActiveClashSubscriptionId(id);
            setActivePanel('clash');
            setIsPanelOpen(true);
          }}
          onAddNewClashSubscription={() => {
            setEditingClashSubscription(null);
            setShowClashModal(true);
          }}
          onEditClashSubscription={(sub) => {
            setEditingClashSubscription(sub);
            setShowClashModal(true);
          }}
          onDeleteClashSubscription={(id) => {
            const nextList = clashSubscriptions.filter((s) => s.id !== id);
            setClashSubscriptions(nextList);
            if (activeClashSubscriptionId === id && nextList[0]) {
              setActiveClashSubscriptionId(nextList[0].id);
            }
          }}
          onDuplicateClashSubscription={(sub) => {
            const dup: ClashSubscription = {
              ...sub,
              id: `sub-${Date.now()}`,
              name: `${sub.name} (副本)`,
              nodes: sub.nodes.map((n) => ({
                ...n,
                id: `node-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
              })),
            };
            setClashSubscriptions([...clashSubscriptions, dup]);
          }}
          onUpdateClashSubscription={(id) => {
            setClashSubscriptions((prev) =>
              prev.map((s) =>
                s.id === id ? { ...s, updatedAt: new Date().toLocaleString() } : s
              )
            );
          }}
          onUpdateAllClashSubscriptions={() => {
            setClashSubscriptions((prev) =>
              prev.map((s) => ({ ...s, updatedAt: new Date().toLocaleString() }))
            );
          }}
          clashSettings={clashSettings}
        />

        {/* Center Workspace View: Primary Terminal Workspace OR Clash.Meta Dashboard */}
        <main className="flex-1 flex flex-col overflow-hidden bg-[#0F0F0F] min-w-0">
          {activePanel === 'clash' ? (
            <ClashMetaDetailView
              subscription={
                clashSubscriptions.find((s) => s.id === activeClashSubscriptionId) ||
                clashSubscriptions[0] ||
                DEFAULT_CLASH_SUBSCRIPTIONS[0]
              }
              allSubscriptions={clashSubscriptions}
              clashSettings={clashSettings}
              onUpdateSettings={setClashSettings}
              onUpdateSubscription={(updated) => {
                setClashSubscriptions((prev) =>
                  prev.map((s) => (s.id === updated.id ? updated : s))
                );
              }}
            />
          ) : (
            <TerminalView
              tabs={tabs}
              activeTabId={activeTabId}
              onSelectTab={setActiveTabId}
              onCloseTab={handleCloseTab}
              onNewTab={handleNewTab}
              hosts={hosts}
              settings={settings}
              sftpFiles={sftpFiles}
              onUpdateSftpFiles={setSftpFiles}
              onOpenFileEditor={setEditorFile}
              onSwitchToSftp={(hostId) => {
                setSelectedHostId(hostId);
                // Open bottom SFTP panel in the active session tab
                setTabs((prev) =>
                  prev.map((t) =>
                    t.id === activeTabId ? { ...t, sftpOpen: true } : t
                  )
                );
              }}
              onSwitchToMonitor={(hostId) => {
                setSelectedHostId(hostId);
                // Open right monitor panel in the active session tab
                setTabs((prev) =>
                  prev.map((t) =>
                    t.id === activeTabId ? { ...t, monitorOpen: !t.monitorOpen } : t
                  )
                );
              }}
              onAskCopilot={(text, taskType) => {
                setCopilotExternalPrompt({ text, taskType });
                setIsCopilotOpen(true);
              }}
              onUpdateTabs={setTabs}
              tunnels={tunnels}
              onUpdateTunnels={setTunnels}
            />
          )}
        </main>

        {/* Right AI Copilot Drawer */}
        <AICopilotDrawer
          isOpen={isCopilotOpen}
          onClose={() => setIsCopilotOpen(false)}
          currentHost={activeHost}
          onExecuteInTerminal={handleExecuteSnippetInTerminal}
          externalPrompt={copilotExternalPrompt || undefined}
        />
      </div>

      {/* Interactive Vim File Editor Modal */}
      {editorFile && (
        <InteractiveVimModal
          isOpen={true}
          file={editorFile}
          onClose={() => setEditorFile(null)}
          onSave={handleSaveEditedFile}
          onAskAI={(code, taskType) => {
            setCopilotExternalPrompt({ text: code, taskType });
            setIsCopilotOpen(true);
          }}
        />
      )}

      {/* Host Manager (Add/Edit) Modal */}
      <HostManagerModal
        isOpen={showHostModal}
        onClose={() => {
          setShowHostModal(false);
          setEditingHost(null);
        }}
        onSaveHost={(host) => {
          if (editingHost) {
            setHosts(hosts.map((h) => (h.id === host.id ? host : h)));
          } else {
            setHosts([...hosts, host]);
          }
        }}
        editingHost={editingHost}
        allHosts={hosts}
        keys={keys}
        credentials={credentials}
        onOpenVaultModal={() => setShowVaultModal(true)}
      />

      {/* Quick Connect Modal */}
      <QuickConnectModal
        isOpen={showQuickConnect}
        onClose={() => setShowQuickConnect(false)}
        onConnect={handleQuickConnect}
        recentHosts={hosts}
        onSelectRecent={(h) => handleConnectHost(h, 'terminal')}
      />

      {/* Account Password & SSH Key Vault Management Modal */}
      <CredentialVaultModal
        isOpen={showVaultModal}
        onClose={() => setShowVaultModal(false)}
        credentials={credentials}
        onUpdateCredentials={setCredentials}
      />

      {/* Clash.Meta Subscription Add/Edit Modal */}
      {showClashModal && (
        <ClashSubscriptionModal
          isOpen={showClashModal}
          onClose={() => {
            setShowClashModal(false);
            setEditingClashSubscription(null);
          }}
          onSaveSubscription={(savedSub) => {
            if (editingClashSubscription) {
              setClashSubscriptions((prev) =>
                prev.map((s) => (s.id === savedSub.id ? savedSub : s))
              );
            } else {
              setClashSubscriptions((prev) => [savedSub, ...prev]);
              setActiveClashSubscriptionId(savedSub.id);
            }
          }}
          editingSubscription={editingClashSubscription}
        />
      )}

      {/* Preferences & Backup Settings Modal */}
      <SettingsModal
        isOpen={showSettings}
        onClose={() => setShowSettings(false)}
        settings={settings}
        onUpdateSettings={setSettings}
        allData={{ hosts, snippets, keys, tunnels, credentials }}
        onImportData={(data) => {
          if (data.hosts) setHosts(data.hosts);
          if (data.snippets) setSnippets(data.snippets);
          if (data.keys) setKeys(data.keys);
          if (data.tunnels) setTunnels(data.tunnels);
          if (data.credentials) setCredentials(data.credentials);
          if (data.settings) setSettings(data.settings);
        }}
      />

      {/* Command Palette Popup (Ctrl+K) */}
      {showCommandPalette && (
        <div
          onClick={() => setShowCommandPalette(false)}
          className="fixed inset-0 bg-black/80 backdrop-blur-xs z-50 flex items-start justify-center pt-20 p-4"
        >
          <div
            onClick={(e) => e.stopPropagation()}
            className="bg-[#252526] border border-[#454545] rounded-xl max-w-xl w-full p-3 shadow-2xl space-y-3 font-sans"
          >
            <div className="flex items-center gap-2 px-2 border-b border-[#333333] pb-2">
              <Search className="w-4 h-4 text-[#007ACC]" />
              <input
                type="text"
                placeholder="Type command, server name, or action (e.g. SFTP, Tunnel, Top)..."
                autoFocus
                className="w-full bg-transparent text-sm text-[#D4D4D4] placeholder-[#858585] focus:outline-none"
              />
              <span className="text-[10px] text-[#858585] font-mono px-1.5 py-0.5 rounded bg-[#1E1E1E] border border-[#333333]">
                ESC
              </span>
            </div>

            <div className="space-y-1 text-xs max-h-72 overflow-y-auto">
              <div className="text-[10px] uppercase font-bold text-[#6D6D6D] px-2 pt-1">
                Panels & Views
              </div>
              <button
                onClick={() => {
                  setActivePanel('hosts');
                  setIsPanelOpen(true);
                  setShowCommandPalette(false);
                }}
                className="w-full flex items-center justify-between px-3 py-2 hover:bg-[#37373D] rounded text-left text-[#CCCCCC] hover:text-[#4EC9B0]"
              >
                <div className="flex items-center gap-2.5">
                  <Terminal className="w-4 h-4 text-[#007ACC]" />
                  <span>Hosts & Servers</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono">Alt+1</span>
              </button>
              <button
                onClick={() => {
                  setActivePanel('snippets');
                  setIsPanelOpen(true);
                  setShowCommandPalette(false);
                }}
                className="w-full flex items-center justify-between px-3 py-2 hover:bg-[#37373D] rounded text-left text-[#CCCCCC] hover:text-[#4EC9B0]"
              >
                <div className="flex items-center gap-2.5">
                  <Code2 className="w-4 h-4 text-[#CE9178]" />
                  <span>DevOps Snippets & Playbooks</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono">Alt+2</span>
              </button>
              <button
                onClick={() => {
                  setActivePanel('snippets');
                  setIsPanelOpen(true);
                  setShowCommandPalette(false);
                }}
                className="w-full flex items-center justify-between px-3 py-2 hover:bg-[#37373D] rounded text-left text-[#CCCCCC] hover:text-[#4EC9B0]"
              >
                <div className="flex items-center gap-2.5">
                  <Plus className="w-4 h-4 text-[#4EC9B0]" />
                  <span>Create New Command Snippet</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono">Snippets</span>
              </button>
              <button
                onClick={() => {
                  setShowVaultModal(true);
                  setShowCommandPalette(false);
                }}
                className="w-full flex items-center justify-between px-3 py-2 hover:bg-[#37373D] rounded text-left text-[#CCCCCC] hover:text-[#4EC9B0]"
              >
                <div className="flex items-center gap-2.5">
                  <ShieldCheck className="w-4 h-4 text-[#4EC9B0]" />
                  <span>密码库 (Vault &amp; Credentials)</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono">Alt+3</span>
              </button>
              <button
                onClick={() => {
                  setActivePanel('clash');
                  setIsPanelOpen(true);
                  setShowCommandPalette(false);
                }}
                className="w-full flex items-center justify-between px-3 py-2 hover:bg-[#37373D] rounded text-left text-[#CCCCCC] hover:text-cyan-400"
              >
                <div className="flex items-center gap-2.5">
                  <Radio className="w-4 h-4 text-cyan-400" />
                  <span>Clash.Meta 订阅与代理管理</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono">Alt+4</span>
              </button>

              <div className="text-[10px] uppercase font-bold text-[#6D6D6D] px-2 pt-2 border-t border-[#333333]">
                Quick Actions
              </div>
              <button
                onClick={() => {
                  setIsPanelOpen((prev) => !prev);
                  setShowCommandPalette(false);
                }}
                className="w-full flex items-center justify-between px-3 py-2 hover:bg-[#37373D] rounded text-left text-[#CCCCCC] hover:text-white"
              >
                <div className="flex items-center gap-2.5">
                  <Layers className="w-4 h-4 text-[#569CD6]" />
                  <span>Toggle Left Side Panel</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono">Ctrl+B</span>
              </button>
              <button
                onClick={() => {
                  setIsCopilotOpen(true);
                  setShowCommandPalette(false);
                }}
                className="w-full flex items-center justify-between px-3 py-2 hover:bg-[#37373D] rounded text-left text-[#C586C0]"
              >
                <div className="flex items-center gap-2.5">
                  <Sparkles className="w-4 h-4 text-[#C586C0]" />
                  <span>Ask AI DevOps Copilot</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono">Ctrl+I</span>
              </button>
              <button
                onClick={() => {
                  setShowQuickConnect(true);
                  setShowCommandPalette(false);
                }}
                className="w-full flex items-center justify-between px-3 py-2 hover:bg-[#37373D] rounded text-left text-[#CE9178]"
              >
                <div className="flex items-center gap-2.5">
                  <Zap className="w-4 h-4 text-[#CE9178]" />
                  <span>Quick Connect via SSH URI</span>
                </div>
                <span className="text-[10px] text-[#6D6D6D] font-mono">Ctrl+Q</span>
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
