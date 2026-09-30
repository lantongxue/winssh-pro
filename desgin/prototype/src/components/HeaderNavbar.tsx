import React from 'react';
import {
  Terminal,
  Server,
  Zap,
  Sparkles,
  SplitSquareVertical,
  SplitSquareHorizontal,
  Square,
  Volume2,
  VolumeX,
  Sliders,
  Radio,
  Monitor,
  Apple,
  AppWindow,
  Search,
  ShieldCheck,
} from 'lucide-react';
import { AppSettings, SessionTab, ClashCoreSettings } from '../types';

interface HeaderNavbarProps {
  settings: AppSettings;
  onUpdateSettings: (newSettings: Partial<AppSettings>) => void;
  onOpenQuickConnect: () => void;
  onOpenCommandPalette: () => void;
  onToggleCopilot: () => void;
  isCopilotOpen: boolean;
  onOpenSettings: () => void;
  onOpenVaultModal?: () => void;
  activeTab?: SessionTab;
  onSetSplit: (split: 'none' | 'vertical' | 'horizontal') => void;
  connectedCount: number;
  totalHostsCount: number;
  clashSettings?: ClashCoreSettings;
  onOpenClash?: () => void;
}

export const HeaderNavbar: React.FC<HeaderNavbarProps> = ({
  settings,
  onUpdateSettings,
  onOpenQuickConnect,
  onOpenCommandPalette,
  onToggleCopilot,
  isCopilotOpen,
  onOpenSettings,
  onOpenVaultModal,
  activeTab,
  onSetSplit,
  connectedCount,
  totalHostsCount,
  clashSettings,
  onOpenClash,
}) => {
  return (
    <header className="h-12 bg-[#2D2D2D] border-b border-[#333333] select-none flex items-center justify-between px-4 z-30">
      {/* Left: Window Controls / OS Style Indicator & Brand */}
      <div className="flex items-center space-x-4">
        {settings.osWindowFrame === 'macos' ? (
          <div className="flex space-x-2 mr-1">
            <button
              aria-label="Close"
              className="h-3 w-3 rounded-full bg-[#FF5F56] hover:opacity-80 transition-opacity flex items-center justify-center"
            />
            <button
              aria-label="Minimize"
              className="h-3 w-3 rounded-full bg-[#FFBD2E] hover:opacity-80 transition-opacity flex items-center justify-center"
            />
            <button
              aria-label="Maximize"
              className="h-3 w-3 rounded-full bg-[#27C93F] hover:opacity-80 transition-opacity flex items-center justify-center"
            />
          </div>
        ) : settings.osWindowFrame === 'linux' ? (
          <div className="flex items-center gap-1.5 mr-1">
            <span className="w-2.5 h-2.5 rounded-full bg-[#CE9178] inline-block" />
            <span className="text-[11px] font-mono text-[#858585] font-semibold">GNOME</span>
          </div>
        ) : (
          <div className="flex space-x-1.5 mr-1">
            <div className="h-2.5 w-2.5 rounded-full bg-[#3C3C3C]"></div>
            <div className="h-2.5 w-2.5 rounded-full bg-[#3C3C3C]"></div>
            <div className="h-2.5 w-2.5 rounded-full bg-[#3C3C3C]"></div>
          </div>
        )}

        {/* Brand Label */}
        <div className="flex items-center space-x-2">
          <div className="w-5 h-5 rounded bg-[#007ACC] flex items-center justify-center text-white font-bold text-xs">
            <Terminal className="w-3.5 h-3.5" />
          </div>
          <span className="text-xs font-bold tracking-widest text-[#858585] font-mono">
            WINSSH <span className="text-[#007ACC]">PRO</span>
          </span>
        </div>

        {/* Active Node Indicator Badge */}
        {activeTab ? (
          <div className="hidden sm:flex h-7 items-center space-x-1 rounded bg-[#3C3C3C] px-2.5 text-xs font-mono">
            <span className="text-[#007ACC] font-semibold">root</span>
            <span className="text-[#858585]">@</span>
            <span className="text-[#4EC9B0]">{activeTab.title || 'node-prod'}</span>
          </div>
        ) : (
          <div className="hidden md:flex h-7 items-center space-x-1.5 rounded bg-[#3C3C3C] px-2.5 text-xs font-mono text-[#D4D4D4]">
            <span className="h-2 w-2 rounded-full bg-[#27C93F] animate-pulse" />
            <span className="text-[#4EC9B0] font-semibold">{connectedCount}</span>
            <span className="text-[#858585]">/</span>
            <span className="text-[#858585]">{totalHostsCount} Online</span>
          </div>
        )}
      </div>

      {/* Center: Command Palette / Search Trigger */}
      <div className="flex items-center space-x-3">
        <button
          onClick={onOpenCommandPalette}
          className="flex h-7 w-48 sm:w-60 items-center justify-between rounded border border-[#454545] bg-[#1E1E1E] px-2.5 text-[11px] text-[#858585] hover:border-[#6D6D6D] hover:text-[#D4D4D4] transition-all group"
        >
          <div className="flex items-center space-x-1.5 truncate">
            <Search className="w-3.5 h-3.5 text-[#858585] group-hover:text-[#4EC9B0] transition-colors" />
            <span className="truncate">Search connections...</span>
          </div>
          <kbd className="hidden sm:inline-block text-[9px] font-mono bg-[#2D2D2D] text-[#858585] px-1 py-0.5 rounded border border-[#454545]">
            Ctrl+K
          </kbd>
        </button>

        {/* Quick Connect Trigger */}
        <button
          onClick={onOpenQuickConnect}
          className="flex h-7 items-center space-x-1 rounded bg-[#007ACC] hover:bg-[#0062a3] px-2.5 text-xs font-medium text-white transition-all shadow-sm"
        >
          <Zap className="w-3.5 h-3.5 fill-white/20" />
          <span className="hidden sm:inline">Connect</span>
        </button>

        {/* Clash.Meta Status Indicator & Quick Switch */}
        {clashSettings && onOpenClash && (
          <button
            onClick={onOpenClash}
            title="Clash.Meta 代理管理与订阅 (Alt+4)"
            className={`hidden md:flex h-7 items-center space-x-1.5 rounded px-2.5 text-xs font-mono transition-all border ${
              clashSettings.masterProxyEnabled
                ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40 hover:bg-emerald-500/25 shadow-xs'
                : 'bg-[#222224] text-zinc-400 border-zinc-700 hover:text-zinc-200'
            }`}
          >
            <Radio className="w-3.5 h-3.5 text-cyan-400" />
            <span className="font-sans font-medium text-[11px]">
              {clashSettings.masterProxyEnabled
                ? clashSettings.tunEnabled
                  ? 'TUN代理: 运行中'
                  : '系统代理: 运行中'
                : '代理: 已停止'}
            </span>
          </button>
        )}
      </div>

      {/* Right: Layout Split, Broadcast, AI Copilot, Settings */}
      <div className="flex items-center space-x-2">
        {/* Split Screen Options */}
        {activeTab && (
          <div className="hidden lg:flex items-center bg-[#1E1E1E] border border-[#454545] rounded p-0.5">
            <button
              onClick={() => onSetSplit('none')}
              title="Single Pane"
              className={`p-1 rounded text-xs transition-colors ${
                activeTab.split === 'none'
                  ? 'bg-[#37373D] text-white shadow-sm'
                  : 'text-[#858585] hover:text-[#D4D4D4]'
              }`}
            >
              <Square className="w-3.5 h-3.5" />
            </button>
            <button
              onClick={() => onSetSplit('vertical')}
              title="Split Vertically"
              className={`p-1 rounded text-xs transition-colors ${
                activeTab.split === 'vertical'
                  ? 'bg-[#37373D] text-[#4EC9B0] shadow-sm'
                  : 'text-[#858585] hover:text-[#D4D4D4]'
              }`}
            >
              <SplitSquareVertical className="w-3.5 h-3.5" />
            </button>
            <button
              onClick={() => onSetSplit('horizontal')}
              title="Split Horizontally"
              className={`p-1 rounded text-xs transition-colors ${
                activeTab.split === 'horizontal'
                  ? 'bg-[#37373D] text-[#4EC9B0] shadow-sm'
                  : 'text-[#858585] hover:text-[#D4D4D4]'
              }`}
            >
              <SplitSquareHorizontal className="w-3.5 h-3.5" />
            </button>
          </div>
        )}

        {/* Broadcast Input Toggle */}
        <button
          onClick={() =>
            onUpdateSettings({ broadcastMode: !settings.broadcastMode })
          }
          title={
            settings.broadcastMode
              ? 'Broadcast Input ENABLED (Sent to all panes)'
              : 'Broadcast Input Disabled'
          }
          className={`flex h-7 items-center space-x-1 px-2 rounded text-xs font-mono transition-all border ${
            settings.broadcastMode
              ? 'bg-[#FF5F56]/20 text-[#FF5F56] border-[#FF5F56]/50 animate-pulse'
              : 'bg-[#3C3C3C] text-[#858585] hover:text-[#D4D4D4] border-[#454545]'
          }`}
        >
          <Radio className="w-3 h-3" />
          <span className="hidden xl:inline">
            {settings.broadcastMode ? 'BROADCAST' : 'Sync'}
          </span>
        </button>

        {/* AI Copilot Toggle */}
        <button
          onClick={onToggleCopilot}
          className={`flex h-7 items-center space-x-1.5 px-2.5 rounded text-xs font-medium transition-all border ${
            isCopilotOpen
              ? 'bg-[#007ACC] text-white border-[#007ACC] shadow-sm'
              : 'bg-[#3C3C3C] hover:bg-[#454545] text-[#D4D4D4] border-[#454545]'
          }`}
        >
          <Sparkles className="w-3.5 h-3.5 text-[#C586C0]" />
          <span className="hidden sm:inline">AI Copilot</span>
        </button>

        {/* Audio Mute / Sound Toggle */}
        <button
          onClick={() =>
            onUpdateSettings({ soundEffects: !settings.soundEffects })
          }
          title={settings.soundEffects ? 'Sound Effects Enabled' : 'Sound Muted'}
          className="h-7 w-7 flex items-center justify-center text-[#858585] hover:text-[#D4D4D4] bg-[#3C3C3C] hover:bg-[#454545] border border-[#454545] rounded transition-colors"
        >
          {settings.soundEffects ? (
            <Volume2 className="w-3.5 h-3.5 text-[#D4D4D4]" />
          ) : (
            <VolumeX className="w-3.5 h-3.5 text-[#858585]" />
          )}
        </button>

        {/* Settings Dialog Trigger */}
        <button
          onClick={onOpenSettings}
          title="App Preferences & SSH Config"
          className="h-7 w-7 flex items-center justify-center text-[#858585] hover:text-[#007ACC] bg-[#3C3C3C] hover:bg-[#454545] border border-[#454545] rounded transition-colors"
        >
          <Sliders className="w-3.5 h-3.5" />
        </button>

        {/* Windows Frame Window Controls */}
        {settings.osWindowFrame === 'windows' && (
          <div className="hidden sm:flex items-center space-x-1 border-l border-[#454545] pl-2 text-[#858585]">
            <button
              aria-label="Minimize"
              className="w-6 h-6 hover:bg-[#3C3C3C] hover:text-white rounded flex items-center justify-center text-xs"
            >
              —
            </button>
            <button
              aria-label="Maximize"
              className="w-6 h-6 hover:bg-[#3C3C3C] hover:text-white rounded flex items-center justify-center text-xs"
            >
              ▢
            </button>
            <button
              aria-label="Close"
              className="w-6 h-6 hover:bg-[#FF5F56] hover:text-white rounded flex items-center justify-center text-xs"
            >
              ✕
            </button>
          </div>
        )}
      </div>
    </header>
  );
};
