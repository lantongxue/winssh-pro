import React, { useState, useRef, useEffect } from 'react';
import {
  Server,
  Code2,
  Network,
  KeyRound,
  ShieldCheck,
  Sparkles,
  Zap,
  Settings,
  Terminal,
  Radio,
} from 'lucide-react';

export type ActionPanelId = 'hosts' | 'snippets' | 'clash' | 'keys';

interface ActionBarProps {
  activePanel: ActionPanelId;
  isPanelOpen: boolean;
  onSelectPanel: (panel: ActionPanelId) => void;
  onOpenQuickConnect: () => void;
  onOpenSettings: () => void;
  onOpenVault?: () => void;
  isCopilotOpen: boolean;
  onToggleCopilot: () => void;
}

export const ActionBar: React.FC<ActionBarProps> = ({
  activePanel,
  isPanelOpen,
  onSelectPanel,
  onOpenQuickConnect,
  onOpenSettings,
  onOpenVault,
  isCopilotOpen,
  onToggleCopilot,
}) => {
  const [isSettingsMenuOpen, setIsSettingsMenuOpen] = useState(false);
  const settingsMenuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (settingsMenuRef.current && !settingsMenuRef.current.contains(e.target as Node)) {
        setIsSettingsMenuOpen(false);
      }
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && isSettingsMenuOpen) {
        setIsSettingsMenuOpen(false);
      }
    };

    if (isSettingsMenuOpen) {
      document.addEventListener('mousedown', handleClickOutside);
      document.addEventListener('keydown', handleKeyDown);
    }
    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [isSettingsMenuOpen]);
  const topItems: {
    id: ActionPanelId;
    label: string;
    icon: React.ElementType;
    shortcut?: string;
  }[] = [
    {
      id: 'hosts',
      label: 'Hosts & Servers',
      icon: Server,
      shortcut: 'Alt+1',
    },
    {
      id: 'snippets',
      label: 'DevOps Snippets Vault',
      icon: Code2,
      shortcut: 'Alt+2',
    },
    {
      id: 'clash',
      label: 'Clash.Meta 订阅与代理',
      icon: Radio,
      shortcut: 'Alt+4',
    },
  ];

  return (
    <aside className="w-12 h-full bg-[#181818] border-r border-[#2D2D2D] flex flex-col justify-between items-center py-2 z-30 select-none shrink-0">
      {/* Top Primary Actions */}
      <div className="flex flex-col items-center gap-1.5 w-full">
        {topItems.map((item) => {
          const Icon = item.icon;
          const isActive = activePanel === item.id && isPanelOpen;

          return (
            <div key={item.id} className="relative group w-full flex justify-center">
              {/* Active Left Indicator Bar */}
              {isActive && (
                <div className="absolute left-0 top-1.5 bottom-1.5 w-[3px] bg-[#007ACC] rounded-r" />
              )}

              <button
                onClick={() => onSelectPanel(item.id)}
                aria-label={item.label}
                className={`w-9 h-9 rounded-md flex items-center justify-center relative transition-all duration-150 ${
                  isActive
                    ? 'bg-[#252526] text-white shadow-sm'
                    : 'text-[#858585] hover:text-[#D4D4D4] hover:bg-[#202020]'
                }`}
              >
                <Icon
                  className={`w-5 h-5 transition-transform group-hover:scale-105 ${
                    isActive ? 'text-[#4EC9B0]' : ''
                  }`}
                />
              </button>

              {/* Tooltip Hover Overlay */}
              <div className="absolute left-12 top-1/2 -translate-y-1/2 bg-[#252526] text-[#D4D4D4] text-xs px-2.5 py-1.5 rounded shadow-xl border border-[#454545] whitespace-nowrap hidden group-hover:flex items-center gap-2 pointer-events-none z-50 animate-in fade-in zoom-in-95 duration-100">
                <span className="font-medium text-white">{item.label}</span>
                {item.shortcut && (
                  <span className="text-[10px] px-1.5 py-0.5 rounded bg-[#1E1E1E] text-[#858585] border border-[#3C3C3C] font-mono">
                    {item.shortcut}
                  </span>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {/* Bottom Utility Actions */}
      <div className="flex flex-col items-center gap-1.5 w-full pt-2 border-t border-[#252526]">
        {/* AI Copilot Toggle */}
        <div className="relative group w-full flex justify-center">
          {isCopilotOpen && (
            <div className="absolute left-0 top-1.5 bottom-1.5 w-[3px] bg-[#9CDCFE] rounded-r" />
          )}
          <button
            onClick={onToggleCopilot}
            aria-label="AI DevOps Copilot"
            className={`w-9 h-9 rounded-md flex items-center justify-center relative transition-all duration-150 ${
              isCopilotOpen
                ? 'bg-[#9CDCFE]/20 text-[#9CDCFE] border border-[#9CDCFE]/40'
                : 'text-[#858585] hover:text-[#9CDCFE] hover:bg-[#202020]'
            }`}
          >
            <Sparkles className="w-5 h-5" />
          </button>
          <div className="absolute left-12 top-1/2 -translate-y-1/2 bg-[#252526] text-[#D4D4D4] text-xs px-2.5 py-1.5 rounded shadow-xl border border-[#454545] whitespace-nowrap hidden group-hover:flex items-center gap-2 pointer-events-none z-50">
            <span className="font-medium text-white">AI DevOps Copilot</span>
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-[#1E1E1E] text-[#9CDCFE] border border-[#3C3C3C] font-mono">
              Ctrl+I
            </span>
          </div>
        </div>

        {/* Quick Connect Trigger */}
        <div className="relative group w-full flex justify-center">
          <button
            onClick={onOpenQuickConnect}
            aria-label="Quick Connect"
            className="w-9 h-9 rounded-md flex items-center justify-center text-[#858585] hover:text-[#D4D4D4] hover:bg-[#202020] transition-colors"
          >
            <Zap className="w-5 h-5 text-[#CE9178]" />
          </button>
          <div className="absolute left-12 top-1/2 -translate-y-1/2 bg-[#252526] text-[#D4D4D4] text-xs px-2.5 py-1.5 rounded shadow-xl border border-[#454545] whitespace-nowrap hidden group-hover:flex items-center gap-2 pointer-events-none z-50">
            <span className="font-medium text-white">Quick Connect</span>
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-[#1E1E1E] text-[#858585] border border-[#3C3C3C] font-mono">
              Ctrl+Q
            </span>
          </div>
        </div>

        {/* Settings Trigger with Popup Menu */}
        <div ref={settingsMenuRef} className="relative group w-full flex justify-center">
          <button
            onClick={() => setIsSettingsMenuOpen((prev) => !prev)}
            aria-label="Settings and Vault"
            className={`w-9 h-9 rounded-md flex items-center justify-center transition-colors ${
              isSettingsMenuOpen
                ? 'bg-[#007ACC] text-white shadow-sm'
                : 'text-[#858585] hover:text-[#D4D4D4] hover:bg-[#202020]'
            }`}
          >
            <Settings className="w-5 h-5" />
          </button>

          {/* Hover Tooltip when menu is not open */}
          {!isSettingsMenuOpen && (
            <div className="absolute left-12 top-1/2 -translate-y-1/2 bg-[#252526] text-[#D4D4D4] text-xs px-2.5 py-1.5 rounded shadow-xl border border-[#454545] whitespace-nowrap hidden group-hover:flex items-center gap-2 pointer-events-none z-50">
              <span className="font-medium text-white">设置</span>
            </div>
          )}

          {/* Settings & Vault Dropdown Menu */}
          {isSettingsMenuOpen && (
            <div className="absolute left-12 bottom-0 mb-0 ml-1 bg-[#252526] text-[#D4D4D4] rounded-lg shadow-2xl border border-[#454545] py-1.5 w-44 z-50 animate-in fade-in select-none">
              <button
                type="button"
                onClick={() => {
                  setIsSettingsMenuOpen(false);
                  onOpenSettings();
                }}
                className="w-full flex items-center gap-2.5 px-3 py-2 text-xs text-[#CCCCCC] hover:text-white hover:bg-[#007ACC] transition-colors text-left group/item"
              >
                <Settings className="w-4 h-4 text-[#9CDCFE] group-hover/item:text-white shrink-0" />
                <div className="flex-1 min-w-0 font-medium text-[13px]">
                  设置
                </div>
              </button>

              <div className="h-px bg-[#3C3C3C] my-1" />

              <button
                type="button"
                onClick={() => {
                  setIsSettingsMenuOpen(false);
                  onOpenVault?.();
                }}
                className="w-full flex items-center justify-between px-3 py-2 text-xs text-[#CCCCCC] hover:text-white hover:bg-[#007ACC] transition-colors text-left group/item"
              >
                <div className="flex items-center gap-2.5 min-w-0">
                  <ShieldCheck className="w-4 h-4 text-[#4EC9B0] group-hover/item:text-white shrink-0" />
                  <span className="font-medium text-[13px]">密码库</span>
                </div>
                <span className="text-[10px] px-1.5 py-0.5 rounded bg-[#1E1E1E] text-[#858585] border border-[#3C3C3C] font-mono group-hover/item:bg-[#005A9E] group-hover/item:border-transparent group-hover/item:text-white">
                  Alt+3
                </span>
              </button>
            </div>
          )}
        </div>
      </div>
    </aside>
  );
};
