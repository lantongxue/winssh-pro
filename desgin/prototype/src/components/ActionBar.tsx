import React from 'react';
import {
  Server,
  Code2,
  Network,
  KeyRound,
  Activity,
  Sparkles,
  Zap,
  Settings,
  Terminal,
} from 'lucide-react';

export type ActionPanelId = 'hosts' | 'snippets' | 'keys' | 'monitor';

interface ActionBarProps {
  activePanel: ActionPanelId;
  isPanelOpen: boolean;
  onSelectPanel: (panel: ActionPanelId) => void;
  onOpenQuickConnect: () => void;
  onOpenSettings: () => void;
  isCopilotOpen: boolean;
  onToggleCopilot: () => void;
}

export const ActionBar: React.FC<ActionBarProps> = ({
  activePanel,
  isPanelOpen,
  onSelectPanel,
  onOpenQuickConnect,
  onOpenSettings,
  isCopilotOpen,
  onToggleCopilot,
}) => {
  const topItems: {
    id: ActionPanelId;
    label: string;
    icon: React.ElementType;
    shortcut?: string;
  }[] = [
    {
      id: 'hosts',
      label: 'Hosts & Fleet Inventory',
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
      id: 'keys',
      label: 'SSH Key Vault',
      icon: KeyRound,
      shortcut: 'Alt+3',
    },
    {
      id: 'monitor',
      label: 'Fleet Telemetry & Status',
      icon: Activity,
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

        {/* Settings Trigger */}
        <div className="relative group w-full flex justify-center">
          <button
            onClick={onOpenSettings}
            aria-label="Settings"
            className="w-9 h-9 rounded-md flex items-center justify-center text-[#858585] hover:text-[#D4D4D4] hover:bg-[#202020] transition-colors"
          >
            <Settings className="w-5 h-5" />
          </button>
          <div className="absolute left-12 top-1/2 -translate-y-1/2 bg-[#252526] text-[#D4D4D4] text-xs px-2.5 py-1.5 rounded shadow-xl border border-[#454545] whitespace-nowrap hidden group-hover:flex items-center gap-2 pointer-events-none z-50">
            <span className="font-medium text-white">Terminal & App Settings</span>
          </div>
        </div>
      </div>
    </aside>
  );
};
