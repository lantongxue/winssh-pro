import React, { useState, useMemo } from 'react';
import {
  Radio,
  Plus,
  RefreshCw,
  Search,
  MoreVertical,
  Check,
  Edit2,
  Trash2,
  Copy,
  ExternalLink,
  Clock,
  Layers,
  Sparkles,
  Zap,
  Globe,
  HardDrive,
  ShieldCheck,
  Activity,
  ArrowUpRight,
  Server,
  Filter,
} from 'lucide-react';
import { ClashSubscription, ClashCoreSettings } from '../../types';

interface ClashSidebarPanelProps {
  subscriptions: ClashSubscription[];
  activeSubscriptionId: string;
  onSelectSubscription: (id: string) => void;
  onAddNewSubscription: () => void;
  onEditSubscription: (sub: ClashSubscription) => void;
  onDeleteSubscription: (id: string) => void;
  onDuplicateSubscription: (sub: ClashSubscription) => void;
  onUpdateSubscription: (id: string) => void;
  onUpdateAllSubscriptions: () => void;
  clashSettings: ClashCoreSettings;
  width?: number;
}

export const ClashSidebarPanel: React.FC<ClashSidebarPanelProps> = ({
  subscriptions,
  activeSubscriptionId,
  onSelectSubscription,
  onAddNewSubscription,
  onEditSubscription,
  onDeleteSubscription,
  onDuplicateSubscription,
  onUpdateSubscription,
  onUpdateAllSubscriptions,
  clashSettings,
}) => {
  const [searchTerm, setSearchTerm] = useState('');
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  const [isUpdatingAll, setIsUpdatingAll] = useState(false);

  // Filter subscriptions by search
  const filteredSubscriptions = useMemo(() => {
    return subscriptions.filter(
      (s) =>
        s.name.toLowerCase().includes(searchTerm.toLowerCase()) ||
        (s.tag && s.tag.toLowerCase().includes(searchTerm.toLowerCase())) ||
        (s.notes && s.notes.toLowerCase().includes(searchTerm.toLowerCase()))
    );
  }, [subscriptions, searchTerm]);

  const activeSub = subscriptions.find((s) => s.id === activeSubscriptionId) || subscriptions[0];

  const handleUpdateAll = () => {
    setIsUpdatingAll(true);
    onUpdateAllSubscriptions();
    setTimeout(() => {
      setIsUpdatingAll(false);
    }, 1200);
  };

  const formatBytes = (bytes: number) => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
  };

  return (
    <div className="h-full flex flex-col bg-[#1E1E20] text-[#D4D4D8] select-none text-xs border-r border-[#2C2C30]">
      {/* Panel Header */}
      <div className="h-10 px-3 flex items-center justify-between border-b border-[#2C2C30] bg-[#18181A] shrink-0">
        <div className="flex items-center gap-2">
          <div className="w-5 h-5 rounded bg-cyan-500/15 border border-cyan-500/30 flex items-center justify-center text-cyan-400">
            <Radio className="w-3.5 h-3.5" />
          </div>
          <span className="font-semibold text-[11px] tracking-wide text-zinc-200 uppercase">
            订阅与节点管理
          </span>
          <span className="px-1.5 py-0.5 rounded text-[10px] bg-zinc-800 text-zinc-400 font-mono">
            {subscriptions.length}
          </span>
        </div>

        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={handleUpdateAll}
            title="一键更新全部订阅"
            className="p-1.5 hover:bg-[#2A2A2E] text-zinc-400 hover:text-cyan-400 rounded-md transition-colors"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${isUpdatingAll ? 'animate-spin text-cyan-400' : ''}`} />
          </button>
          <button
            type="button"
            onClick={onAddNewSubscription}
            title="新增 Clash.Meta 订阅"
            className="p-1.5 bg-cyan-600/90 hover:bg-cyan-500 text-white rounded-md transition-colors flex items-center gap-1 text-[11px] font-medium px-2 shadow-xs"
          >
            <Plus className="w-3.5 h-3.5" />
            <span>添加</span>
          </button>
        </div>
      </div>

      {/* Search Bar */}
      <div className="p-2 border-b border-[#2C2C30] bg-[#18181A]/60">
        <div className="relative">
          <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-500" />
          <input
            type="text"
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
            placeholder="搜索订阅名称、标签..."
            className="w-full bg-[#141416] border border-[#2E2E32] rounded-md pl-8 pr-3 py-1.5 text-xs text-zinc-200 placeholder-zinc-500 focus:outline-none focus:border-cyan-500 transition-colors"
          />
        </div>
      </div>

      {/* Subscription Cards List */}
      <div className="flex-1 overflow-y-auto p-2 space-y-2">
        {filteredSubscriptions.length === 0 ? (
          <div className="h-48 flex flex-col items-center justify-center text-center p-4 text-zinc-500 space-y-2">
            <Radio className="w-8 h-8 text-zinc-600 stroke-[1.5]" />
            <p className="text-xs">暂无匹配的 Clash 订阅</p>
            <button
              onClick={onAddNewSubscription}
              className="text-[11px] text-cyan-400 hover:underline flex items-center gap-1"
            >
              <Plus className="w-3 h-3" />
              <span>点击添加第一个订阅</span>
            </button>
          </div>
        ) : (
          filteredSubscriptions.map((sub) => {
            const isActive = sub.id === activeSubscriptionId;
            const usedBytes = sub.uploadBytes + sub.downloadBytes;
            const usagePercent =
              sub.totalBytes > 0 ? Math.min(100, Math.round((usedBytes / sub.totalBytes) * 100)) : 0;

            return (
              <div
                key={sub.id}
                onClick={() => onSelectSubscription(sub.id)}
                className={`group relative rounded-xl border p-2.5 transition-all cursor-pointer ${
                  isActive
                    ? 'bg-[#242428] border-cyan-500/50 shadow-md ring-1 ring-cyan-500/20'
                    : 'bg-[#18181A] border-[#2C2C30] hover:bg-[#202024] hover:border-[#3A3A40]'
                }`}
              >
                {/* Active indicator bar */}
                {isActive && (
                  <div className="absolute -left-[1px] top-3 bottom-3 w-[3px] bg-cyan-400 rounded-r shadow-[0_0_8px_rgba(6,182,212,0.8)]" />
                )}

                {/* Card Header */}
                <div className="flex items-start justify-between gap-1.5 mb-1.5">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-1.5">
                      <div
                        style={{ backgroundColor: sub.color || '#06b6d4' }}
                        className="w-2 h-2 rounded-full shrink-0 shadow-xs"
                      />
                      <h4
                        className={`text-xs font-semibold truncate ${
                          isActive ? 'text-white' : 'text-zinc-200 group-hover:text-white'
                        }`}
                        title={sub.name}
                      >
                        {sub.name}
                      </h4>
                    </div>

                    <div className="flex items-center gap-1.5 mt-1 text-[10px] text-zinc-400">
                      {sub.tag && (
                        <span className="px-1.5 py-0.5 rounded bg-[#2A2A2E] text-cyan-300 border border-[#3A3A40]">
                          {sub.tag}
                        </span>
                      )}
                      <span className="flex items-center gap-0.5 text-zinc-400 font-mono">
                        <Server className="w-2.5 h-2.5 text-zinc-500" />
                        {sub.nodes.length} 节点
                      </span>
                      <span className="text-zinc-500">•</span>
                      <span className="font-mono text-zinc-400">{sub.rules.length} 规则</span>
                    </div>
                  </div>

                  {/* Actions Dropdown Trigger */}
                  <div className="relative shrink-0" onClick={(e) => e.stopPropagation()}>
                    <button
                      type="button"
                      onClick={() => setOpenMenuId(openMenuId === sub.id ? null : sub.id)}
                      className="p-1 hover:bg-[#2C2C30] text-zinc-400 hover:text-white rounded transition-colors"
                    >
                      <MoreVertical className="w-3.5 h-3.5" />
                    </button>

                    {/* Popover Action Menu */}
                    {openMenuId === sub.id && (
                      <div className="absolute right-0 top-6 w-36 bg-[#252528] border border-[#3A3A40] rounded-lg shadow-xl py-1 z-30 animate-in fade-in select-none">
                        <button
                          type="button"
                          onClick={() => {
                            setOpenMenuId(null);
                            onSelectSubscription(sub.id);
                          }}
                          className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left text-[11px] text-zinc-300 hover:bg-cyan-600 hover:text-white transition-colors"
                        >
                          <Check className="w-3 h-3 text-cyan-400" />
                          <span>设为当前使用</span>
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setOpenMenuId(null);
                            onUpdateSubscription(sub.id);
                          }}
                          className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left text-[11px] text-zinc-300 hover:bg-cyan-600 hover:text-white transition-colors"
                        >
                          <RefreshCw className="w-3 h-3 text-emerald-400" />
                          <span>立即更新节点</span>
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setOpenMenuId(null);
                            onEditSubscription(sub);
                          }}
                          className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left text-[11px] text-zinc-300 hover:bg-cyan-600 hover:text-white transition-colors"
                        >
                          <Edit2 className="w-3 h-3 text-blue-400" />
                          <span>编辑配置</span>
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setOpenMenuId(null);
                            onDuplicateSubscription(sub);
                          }}
                          className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left text-[11px] text-zinc-300 hover:bg-cyan-600 hover:text-white transition-colors"
                        >
                          <Copy className="w-3 h-3 text-purple-400" />
                          <span>创建副本</span>
                        </button>
                        <div className="h-px bg-[#333338] my-1" />
                        <button
                          type="button"
                          onClick={() => {
                            setOpenMenuId(null);
                            onDeleteSubscription(sub.id);
                          }}
                          className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left text-[11px] text-red-400 hover:bg-red-500 hover:text-white transition-colors"
                        >
                          <Trash2 className="w-3 h-3" />
                          <span>删除订阅</span>
                        </button>
                      </div>
                    )}
                  </div>
                </div>

                {/* Traffic Usage Bar */}
                <div className="mt-2 space-y-1">
                  <div className="flex items-center justify-between text-[10px] text-zinc-400 font-mono">
                    <span>
                      已用: {formatBytes(usedBytes)} / {formatBytes(sub.totalBytes)}
                    </span>
                    <span className={usagePercent > 85 ? 'text-amber-400' : 'text-zinc-400'}>
                      {usagePercent}%
                    </span>
                  </div>
                  <div className="w-full h-1.5 bg-[#2A2A2E] rounded-full overflow-hidden">
                    <div
                      style={{
                        width: `${usagePercent}%`,
                        backgroundColor:
                          usagePercent > 90 ? '#ef4444' : usagePercent > 70 ? '#f59e0b' : sub.color || '#06b6d4',
                      }}
                      className="h-full rounded-full transition-all duration-300"
                    />
                  </div>
                </div>

                {/* Footer Meta */}
                <div className="mt-2 flex items-center justify-between text-[10px] text-zinc-400 pt-1.5 border-t border-[#26262A]">
                  <div className="flex items-center gap-1 truncate text-zinc-400">
                    <Clock className="w-2.5 h-2.5 shrink-0" />
                    <span className="truncate">{sub.updatedAt.split(' ')[0]}</span>
                  </div>
                  <span className="text-[10px] font-mono text-zinc-400">
                    到期: {sub.expireDate}
                  </span>
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* Bottom Core Info Widget */}
      <div className="p-2.5 border-t border-[#2C2C30] bg-[#18181A] shrink-0 space-y-1.5">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-1.5">
            <span
              className={`w-2 h-2 rounded-full ${
                clashSettings.masterProxyEnabled
                  ? 'bg-emerald-400 animate-pulse shadow-[0_0_6px_rgba(52,211,153,0.8)]'
                  : 'bg-zinc-500'
              }`}
            />
            <span className="text-[11px] font-semibold text-zinc-300">Clash.Meta (Mihomo)</span>
          </div>
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-zinc-800 text-cyan-400 font-mono">
            v1.18.9
          </span>
        </div>

        <div className="grid grid-cols-2 gap-1 text-[10px] text-zinc-400 font-mono">
          <div className="flex items-center justify-between bg-[#141416] px-1.5 py-1 rounded border border-[#26262A]">
            <span className="text-zinc-500">混合端口</span>
            <span className="text-zinc-200">{clashSettings.mixedPort}</span>
          </div>
          <div className="flex items-center justify-between bg-[#141416] px-1.5 py-1 rounded border border-[#26262A]">
            <span className="text-zinc-500">API控制</span>
            <span className="text-zinc-200">{clashSettings.apiPort}</span>
          </div>
        </div>
      </div>
    </div>
  );
};
