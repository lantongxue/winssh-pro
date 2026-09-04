import React, { useState } from 'react';
import {
  Settings,
  X,
  Palette,
  Terminal,
  FolderSync,
  Download,
  Upload,
  RotateCcw,
  Check,
  Shield,
  Bell,
  Cpu,
} from 'lucide-react';
import { AppSettings, ServerHost, DevOpsSnippet, SSHKeyItem, PortTunnel, VaultCredential } from '../types';

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
  settings: AppSettings;
  onUpdateSettings: (newSettings: AppSettings) => void;
  allData: {
    hosts: ServerHost[];
    snippets: DevOpsSnippet[];
    keys: SSHKeyItem[];
    tunnels: PortTunnel[];
    credentials?: VaultCredential[];
  };
  onImportData: (imported: any) => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  isOpen,
  onClose,
  settings,
  onUpdateSettings,
  allData,
  onImportData,
}) => {
  const [activeTab, setActiveTab] = useState<'terminal' | 'sftp' | 'backup'>('terminal');
  const [importStatus, setImportStatus] = useState<string | null>(null);

  if (!isOpen) return null;

  const handleExportJson = () => {
    const payload = {
      version: '2.4.0',
      exportedAt: new Date().toISOString(),
      settings,
      ...allData,
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], {
      type: 'application/json',
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `winssh_pro_backup_${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleImportFile = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (event) => {
      try {
        const parsed = JSON.parse(event.target?.result as string);
        onImportData(parsed);
        setImportStatus('Configuration restored successfully!');
        setTimeout(() => setImportStatus(null), 3000);
      } catch (err) {
        setImportStatus('Invalid JSON backup file.');
      }
    };
    reader.readAsText(file);
  };

  return (
    <div className="fixed inset-0 bg-black/75 backdrop-blur-sm z-50 flex items-center justify-center p-4 select-none">
      <div className="bg-zinc-900 border border-zinc-750 rounded-2xl max-w-xl w-full p-5 shadow-2xl space-y-4 font-sans max-h-[90vh] flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-zinc-800 pb-3">
          <div className="flex items-center gap-2.5">
            <div className="w-8 h-8 rounded-lg bg-cyan-500/10 border border-cyan-500/30 flex items-center justify-center text-cyan-400">
              <Settings className="w-4 h-4" />
            </div>
            <div>
              <h3 className="text-sm font-semibold text-zinc-100">
                WinSSH Pro Preferences & Engine Config
              </h3>
              <p className="text-[11px] text-zinc-400">
                Terminal rendering, theme aesthetics, SFTP engine, and system backup
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-zinc-400 hover:text-zinc-200 p-1 hover:bg-zinc-800 rounded-lg transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Tab Switcher */}
        <div className="flex items-center gap-1 border-b border-zinc-800 pb-2 text-xs">
          <button
            onClick={() => setActiveTab('terminal')}
            className={`px-3 py-1.5 rounded-lg transition-colors font-medium ${
              activeTab === 'terminal'
                ? 'bg-cyan-500/20 text-cyan-300 border border-cyan-500/40'
                : 'text-zinc-400 hover:text-zinc-200'
            }`}
          >
            Terminal & Emulation
          </button>
          <button
            onClick={() => setActiveTab('sftp')}
            className={`px-3 py-1.5 rounded-lg transition-colors font-medium ${
              activeTab === 'sftp'
                ? 'bg-cyan-500/20 text-cyan-300 border border-cyan-500/40'
                : 'text-zinc-400 hover:text-zinc-200'
            }`}
          >
            SFTP & Network
          </button>
          <button
            onClick={() => setActiveTab('backup')}
            className={`px-3 py-1.5 rounded-lg transition-colors font-medium ${
              activeTab === 'backup'
                ? 'bg-cyan-500/20 text-cyan-300 border border-cyan-500/40'
                : 'text-zinc-400 hover:text-zinc-200'
            }`}
          >
            Data Backup & Sync
          </button>
        </div>

        {/* Tab Contents */}
        <div className="flex-1 overflow-y-auto space-y-4 text-xs pr-1">
          {activeTab === 'terminal' && (
            <div className="space-y-4">
              {/* Terminal Theme */}
              <div className="space-y-1.5">
                <label className="text-zinc-300 font-medium block">Color Theme</label>
                <div className="grid grid-cols-3 gap-2">
                  {[
                    { id: 'dark', label: 'Midnight Cyan' },
                    { id: 'dracula', label: 'Dracula Purple' },
                    { id: 'matrix', label: 'Matrix Hacker' },
                    { id: 'solarized', label: 'Solarized Dark' },
                    { id: 'nord', label: 'Nordic Frost' },
                  ].map((thm) => (
                    <button
                      key={thm.id}
                      type="button"
                      onClick={() =>
                        onUpdateSettings({ ...settings, theme: thm.id as any })
                      }
                      className={`p-2 rounded-lg border text-left transition-colors ${
                        settings.theme === thm.id
                          ? 'bg-cyan-500/20 border-cyan-500 text-cyan-300'
                          : 'bg-zinc-950 border-zinc-800 text-zinc-400'
                      }`}
                    >
                      <span className="font-semibold block">{thm.label}</span>
                    </button>
                  ))}
                </div>
              </div>

              {/* Font Size & Family */}
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="text-zinc-300 font-medium">Font Size</label>
                  <select
                    value={settings.fontSize}
                    onChange={(e) =>
                      onUpdateSettings({
                        ...settings,
                        fontSize: Number(e.target.value),
                      })
                    }
                    className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none font-mono"
                  >
                    <option value={11}>11px (Ultra Compact)</option>
                    <option value={12}>12px (Compact)</option>
                    <option value={13}>13px (Standard)</option>
                    <option value={14}>14px (Comfortable)</option>
                    <option value={16}>16px (Large)</option>
                  </select>
                </div>

                <div className="space-y-1">
                  <label className="text-zinc-300 font-medium">Font Family</label>
                  <select
                    value={settings.fontFamily}
                    onChange={(e) =>
                      onUpdateSettings({
                        ...settings,
                        fontFamily: e.target.value,
                      })
                    }
                    className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none font-mono"
                  >
                    <option value="'JetBrains Mono', monospace">JetBrains Mono</option>
                    <option value="'Fira Code', monospace">Fira Code</option>
                    <option value="'SF Mono', Menlo, monospace">SF Mono / Menlo</option>
                    <option value="Consolas, monospace">Consolas</option>
                  </select>
                </div>
              </div>

              {/* Cursor Style & Bell */}
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="text-zinc-300 font-medium">Cursor Style</label>
                  <select
                    value={settings.cursorStyle}
                    onChange={(e) =>
                      onUpdateSettings({
                        ...settings,
                        cursorStyle: e.target.value as any,
                      })
                    }
                    className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none"
                  >
                    <option value="block">Solid Block █</option>
                    <option value="underline">Underline _</option>
                    <option value="bar">Vertical Bar |</option>
                  </select>
                </div>

                <div className="space-y-1">
                  <label className="text-zinc-300 font-medium">Scrollback Buffer</label>
                  <select
                    value={settings.scrollbackLines}
                    onChange={(e) =>
                      onUpdateSettings({
                        ...settings,
                        scrollbackLines: Number(e.target.value),
                      })
                    }
                    className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none font-mono"
                  >
                    <option value={1000}>1,000 lines</option>
                    <option value={5000}>5,000 lines</option>
                    <option value={10000}>10,000 lines</option>
                  </select>
                </div>
              </div>
            </div>
          )}

          {activeTab === 'sftp' && (
            <div className="space-y-4">
              <div className="p-3 bg-zinc-950 rounded-xl border border-zinc-800 space-y-3">
                <div className="flex items-center justify-between">
                  <div>
                    <span className="font-semibold text-zinc-200 block">Show Hidden Files</span>
                    <span className="text-[11px] text-zinc-500">Display dotfiles (.bashrc, .ssh, .env) in SFTP</span>
                  </div>
                  <input
                    type="checkbox"
                    checked={settings.sftpShowHidden}
                    onChange={(e) =>
                      onUpdateSettings({
                        ...settings,
                        sftpShowHidden: e.target.checked,
                      })
                    }
                    className="w-4 h-4 accent-cyan-500 rounded"
                  />
                </div>

                <div className="flex items-center justify-between border-t border-zinc-850 pt-3">
                  <div>
                    <span className="font-semibold text-zinc-200 block">Broadcast Input Mode</span>
                    <span className="text-[11px] text-zinc-500">Replicate typed commands across all open split panes</span>
                  </div>
                  <input
                    type="checkbox"
                    checked={settings.broadcastMode}
                    onChange={(e) =>
                      onUpdateSettings({
                        ...settings,
                        broadcastMode: e.target.checked,
                      })
                    }
                    className="w-4 h-4 accent-cyan-500 rounded"
                  />
                </div>
              </div>
            </div>
          )}

          {activeTab === 'backup' && (
            <div className="space-y-4">
              <div className="p-3 bg-zinc-950 rounded-xl border border-zinc-800 space-y-3">
                <div>
                  <h4 className="font-semibold text-zinc-200">Export WinSSH Pro State</h4>
                  <p className="text-[11px] text-zinc-400">
                    Export your server inventory, snippets, SSH keys, and tunnels into a secure JSON backup.
                  </p>
                </div>

                <button
                  onClick={handleExportJson}
                  className="flex items-center gap-1.5 px-3.5 py-1.5 bg-zinc-800 hover:bg-zinc-750 text-zinc-100 rounded-lg font-medium transition-colors border border-zinc-700"
                >
                  <Download className="w-3.5 h-3.5 text-cyan-400" />
                  <span>Download Backup (.json)</span>
                </button>
              </div>

              <div className="p-3 bg-zinc-950 rounded-xl border border-zinc-800 space-y-3">
                <div>
                  <h4 className="font-semibold text-zinc-200">Restore from Backup</h4>
                  <p className="text-[11px] text-zinc-400">
                    Import an existing WinSSH Pro JSON file to sync servers and preferences.
                  </p>
                </div>

                <label className="inline-flex items-center gap-1.5 px-3.5 py-1.5 bg-cyan-600/20 hover:bg-cyan-600/30 text-cyan-300 border border-cyan-500/40 rounded-lg font-medium transition-colors cursor-pointer">
                  <Upload className="w-3.5 h-3.5" />
                  <span>Choose Backup File</span>
                  <input
                    type="file"
                    accept=".json"
                    onChange={handleImportFile}
                    className="hidden"
                  />
                </label>

                {importStatus && (
                  <p className="text-xs font-mono text-emerald-400">{importStatus}</p>
                )}
              </div>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex justify-end pt-3 border-t border-zinc-800">
          <button
            onClick={onClose}
            className="px-5 py-2 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg font-medium transition-colors"
          >
            Done
          </button>
        </div>
      </div>
    </div>
  );
};
