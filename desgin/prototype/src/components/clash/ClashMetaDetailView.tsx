import React, { useState, useEffect, useMemo } from 'react';
import {
  Radio,
  Power,
  Zap,
  Shield,
  ShieldCheck,
  Globe,
  Sliders,
  Network,
  RefreshCw,
  Search,
  Filter,
  Check,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  Download,
  Copy,
  Terminal,
  Play,
  Pause,
  Trash2,
  Plus,
  Wifi,
  Cpu,
  ArrowUp,
  ArrowDown,
  Activity,
  Layers,
  Sparkles,
  Info,
  Server,
  AlertTriangle,
  ArrowRight,
} from 'lucide-react';
import {
  ClashSubscription,
  ClashCoreSettings,
  ClashProxyMode,
  ClashNode,
  ClashRule,
  ClashProxyGroup,
  ClashActiveConnection,
} from '../../types';
import { INITIAL_ACTIVE_CONNECTIONS } from '../../data/defaultClashData';

interface ClashMetaDetailViewProps {
  subscription: ClashSubscription;
  allSubscriptions: ClashSubscription[];
  clashSettings: ClashCoreSettings;
  onUpdateSettings: (settings: ClashCoreSettings) => void;
  onUpdateSubscription: (sub: ClashSubscription) => void;
}

export const ClashMetaDetailView: React.FC<ClashMetaDetailViewProps> = ({
  subscription,
  allSubscriptions,
  clashSettings,
  onUpdateSettings,
  onUpdateSubscription,
}) => {
  // Tabs: 'proxies' (节点) | 'rules' (分流规则) | 'config' (配置源码) | 'connections' (连接与日志)
  const [activeTab, setActiveTab] = useState<'proxies' | 'rules' | 'config' | 'connections'>('proxies');

  // Proxies / Nodes State
  const [activeGroupId, setActiveGroupId] = useState<string>(
    subscription.proxyGroups[0]?.id || 'group-proxy'
  );
  const [nodeSearch, setNodeSearch] = useState('');
  const [selectedRegion, setSelectedRegion] = useState<string>('ALL');
  const [selectedProtocol, setSelectedProtocol] = useState<string>('ALL');
  const [testingAll, setTestingAll] = useState(false);
  const [testingNodeId, setTestingNodeId] = useState<string | null>(null);

  // Rules State
  const [ruleSearch, setRuleSearch] = useState('');
  const [ruleTypeFilter, setRuleTypeFilter] = useState<string>('ALL');
  const [testDomain, setTestDomain] = useState('github.com');
  const [testResult, setTestResult] = useState<{
    matchedRule: ClashRule;
    destPolicy: string;
  } | null>(null);

  // Connections State
  const [connections, setConnections] = useState<ClashActiveConnection[]>(INITIAL_ACTIVE_CONNECTIONS);
  const [logFilter, setLogFilter] = useState<'all' | 'info' | 'warn' | 'error'>('all');
  const [logs, setLogs] = useState<
    { id: string; time: string; level: 'info' | 'warn' | 'error'; msg: string }[]
  >([
    {
      id: 'l-1',
      time: '23:14:02',
      level: 'info',
      msg: '[Clash.Meta] Core initialized successfully (Mihomo v1.18.9-meta linux-amd64)',
    },
    {
      id: 'l-2',
      time: '23:14:03',
      level: 'info',
      msg: '[TUN] MetaTUN virtual interface created (gVisor stack, Fake-IP pool 198.18.0.1/16)',
    },
    {
      id: 'l-3',
      time: '23:14:05',
      level: 'info',
      msg: '[DNS] Nameserver ready: 223.5.5.5, 119.29.29.29, tls://1.1.1.1:853',
    },
    {
      id: 'l-4',
      time: '23:14:10',
      level: 'info',
      msg: '[TCP] api.github.com:443 --> matched [DOMAIN-SUFFIX github.com] --> [🇭🇰 香港 01 [Hysteria2 专线]]',
    },
    {
      id: 'l-5',
      time: '23:15:22',
      level: 'info',
      msg: '[TCP] registry-1.docker.io:443 --> matched [DOMAIN-SUFFIX docker.io] --> [🇭🇰 香港 01 [Hysteria2 专线]]',
    },
  ]);

  // Traffic dynamic simulation
  const [upSpeed, setUpSpeed] = useState<string>('24.8 KB/s');
  const [downSpeed, setDownSpeed] = useState<string>('1.62 MB/s');

  useEffect(() => {
    if (!clashSettings.masterProxyEnabled) {
      setUpSpeed('0 B/s');
      setDownSpeed('0 B/s');
      return;
    }

    const timer = setInterval(() => {
      const up = (Math.random() * 80 + 10).toFixed(1);
      const down = (Math.random() * 2.8 + 0.4).toFixed(2);
      setUpSpeed(`${up} KB/s`);
      setDownSpeed(`${down} MB/s`);
    }, 2500);

    return () => clearInterval(timer);
  }, [clashSettings.masterProxyEnabled]);

  // Active proxy group
  const activeGroup =
    subscription.proxyGroups.find((g) => g.id === activeGroupId) || subscription.proxyGroups[0];

  // Filter nodes for the current group
  const groupNodeNames = useMemo(() => {
    if (!activeGroup) return [];
    return activeGroup.proxies;
  }, [activeGroup]);

  const filteredNodes = useMemo(() => {
    return subscription.nodes.filter((node) => {
      // If group specifies certain proxies, filter accordingly if not general
      const inGroup =
        groupNodeNames.length === 0 ||
        groupNodeNames.includes(node.name) ||
        activeGroup?.type === 'select';

      const matchSearch =
        node.name.toLowerCase().includes(nodeSearch.toLowerCase()) ||
        node.server.toLowerCase().includes(nodeSearch.toLowerCase());

      const matchRegion =
        selectedRegion === 'ALL' || (node.country && node.country.toUpperCase() === selectedRegion);

      const matchProto =
        selectedProtocol === 'ALL' || node.type.toLowerCase() === selectedProtocol.toLowerCase();

      return inGroup && matchSearch && matchRegion && matchProto;
    });
  }, [subscription.nodes, groupNodeNames, activeGroup, nodeSearch, selectedRegion, selectedProtocol]);

  // Ping test individual node
  const handleTestNode = (nodeId: string) => {
    setTestingNodeId(nodeId);
    setTimeout(() => {
      const latency = Math.floor(Math.random() * 85 + 25);
      const updatedNodes = subscription.nodes.map((n) =>
        n.id === nodeId ? { ...n, latency, status: latency > 200 ? 'slow' as const : 'online' as const } : n
      );
      onUpdateSubscription({ ...subscription, nodes: updatedNodes });
      setTestingNodeId(null);
    }, 600);
  };

  // Ping test all nodes in current subscription
  const handleTestAllNodes = () => {
    setTestingAll(true);
    setTimeout(() => {
      const updatedNodes = subscription.nodes.map((n) => {
        const isTimeout = Math.random() < 0.05;
        const latency = isTimeout ? -1 : Math.floor(Math.random() * 120 + 28);
        return {
          ...n,
          latency,
          status: (latency === -1 ? 'timeout' : latency > 180 ? 'slow' : 'online') as
            | 'online'
            | 'slow'
            | 'timeout',
        };
      });
      onUpdateSubscription({ ...subscription, nodes: updatedNodes });
      setTestingAll(false);
    }, 1200);
  };

  // Select node for group
  const handleSelectNodeForGroup = (nodeName: string) => {
    if (!activeGroup) return;
    const updatedGroups = subscription.proxyGroups.map((g) =>
      g.id === activeGroup.id ? { ...g, selectedNode: nodeName } : g
    );
    onUpdateSubscription({ ...subscription, proxyGroups: updatedGroups });

    // Append log
    const newLog = {
      id: `l-${Date.now()}`,
      time: new Date().toLocaleTimeString(),
      level: 'info' as const,
      msg: `[ProxyGroup] ${activeGroup.name} switched active node to -> [${nodeName}]`,
    };
    setLogs((prev) => [newLog, ...prev]);
  };

  // Test domain routing
  const handleTestDomainRouting = (e: React.FormEvent) => {
    e.preventDefault();
    if (!testDomain.trim()) return;

    const domain = testDomain.trim().toLowerCase();
    // Match against rules in order
    let matched: ClashRule | null = null;
    for (const rule of subscription.rules) {
      if (rule.type === 'DOMAIN-SUFFIX' && (domain === rule.payload || domain.endsWith(`.${rule.payload}`))) {
        matched = rule;
        break;
      } else if (rule.type === 'DOMAIN' && domain === rule.payload) {
        matched = rule;
        break;
      } else if (rule.type === 'DOMAIN-KEYWORD' && domain.includes(rule.payload)) {
        matched = rule;
        break;
      } else if (rule.type === 'MATCH') {
        matched = rule;
        break;
      }
    }

    if (!matched) {
      matched = subscription.rules[subscription.rules.length - 1] || {
        id: 'r-fallback',
        type: 'MATCH',
        payload: '',
        target: 'DIRECT',
      };
    }

    setTestResult({
      matchedRule: matched,
      destPolicy: matched.target,
    });
  };

  // Protocols color mapping
  const getProtocolBadge = (type: string) => {
    switch (type.toLowerCase()) {
      case 'hysteria2':
        return 'bg-purple-500/15 text-purple-300 border-purple-500/30';
      case 'vless':
        return 'bg-cyan-500/15 text-cyan-300 border-cyan-500/30';
      case 'vmess':
        return 'bg-blue-500/15 text-blue-300 border-blue-500/30';
      case 'trojan':
        return 'bg-amber-500/15 text-amber-300 border-amber-500/30';
      case 'ss':
      case 'shadowsocks':
        return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30';
      case 'tuic':
        return 'bg-pink-500/15 text-pink-300 border-pink-500/30';
      case 'wireguard':
        return 'bg-indigo-500/15 text-indigo-300 border-indigo-500/30';
      default:
        return 'bg-zinc-800 text-zinc-300 border-zinc-700';
    }
  };

  return (
    <div className="h-full flex flex-col bg-[#141416] text-[#D4D4D8] overflow-hidden select-none font-sans min-w-0">
      {/* =========================================================================
          TOP CONTROL BAR: 代理总开关、代理模式、系统代理、虚拟网卡代理 (TUN)
         ========================================================================= */}
      <header className="border-b border-[#26262A] bg-[#1A1A1D] px-4 py-3 shrink-0 flex flex-wrap items-center justify-between gap-3 z-10 shadow-sm">
        {/* Left: Master Proxy Toggle & Active Subscription Name */}
        <div className="flex items-center gap-3">
          {/* Master Switch Button */}
          <button
            type="button"
            onClick={() =>
              onUpdateSettings({
                ...clashSettings,
                masterProxyEnabled: !clashSettings.masterProxyEnabled,
              })
            }
            className={`flex items-center gap-2 px-3 py-1.5 rounded-xl font-medium text-xs transition-all shadow-sm ${
              clashSettings.masterProxyEnabled
                ? 'bg-emerald-600 hover:bg-emerald-500 text-white shadow-emerald-950/40 ring-2 ring-emerald-500/30'
                : 'bg-zinc-800 hover:bg-zinc-700 text-zinc-400 border border-zinc-700'
            }`}
          >
            <Power className="w-3.5 h-3.5" />
            <span>{clashSettings.masterProxyEnabled ? '代理已运行' : '代理已停止'}</span>
            <div
              className={`w-2 h-2 rounded-full ${
                clashSettings.masterProxyEnabled
                  ? 'bg-white animate-pulse shadow-[0_0_8px_white]'
                  : 'bg-zinc-500'
              }`}
            />
          </button>

          {/* Active Subscription Breadcrumb */}
          <div className="hidden sm:flex flex-col min-w-0">
            <div className="flex items-center gap-1.5">
              <span className="text-[10px] text-zinc-400 uppercase tracking-wider font-semibold">
                当前生效订阅
              </span>
              <span className="w-1.5 h-1.5 rounded-full bg-cyan-400" />
            </div>
            <div className="text-xs font-semibold text-zinc-100 truncate max-w-xs" title={subscription.name}>
              {subscription.name}
            </div>
          </div>
        </div>

        {/* Center: Proxy Mode Selector (Rule, Global, Direct, Script) */}
        <div className="flex items-center bg-[#121214] p-1 rounded-xl border border-[#2E2E34] text-xs">
          {[
            { id: 'rule', label: '规则 (Rule)', icon: Sliders },
            { id: 'global', label: '全局 (Global)', icon: Globe },
            { id: 'direct', label: '直连 (Direct)', icon: Zap },
            { id: 'script', label: '脚本 (Script)', icon: Terminal },
          ].map((m) => {
            const isSelected = clashSettings.mode === m.id;
            const Icon = m.icon;
            return (
              <button
                key={m.id}
                type="button"
                onClick={() =>
                  onUpdateSettings({ ...clashSettings, mode: m.id as ClashProxyMode })
                }
                className={`flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-xs font-medium transition-all ${
                  isSelected
                    ? 'bg-cyan-500/20 text-cyan-300 border border-cyan-500/40 shadow-xs'
                    : 'text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800/60'
                }`}
              >
                <Icon className="w-3 h-3" />
                <span>{m.label}</span>
              </button>
            );
          })}
        </div>

        {/* Right: 系统代理开关 & 虚拟网卡代理 (TUN) 开关 & 实时速率 */}
        <div className="flex items-center gap-2.5">
          {/* 系统代理开关 (System Proxy) */}
          <div
            onClick={() =>
              onUpdateSettings({
                ...clashSettings,
                systemProxyEnabled: !clashSettings.systemProxyEnabled,
              })
            }
            className={`flex items-center gap-2 px-2.5 py-1.5 rounded-lg border cursor-pointer transition-all select-none ${
              clashSettings.systemProxyEnabled
                ? 'bg-blue-500/15 border-blue-500/40 text-blue-300'
                : 'bg-[#18181A] border-[#2C2C30] text-zinc-400 hover:bg-[#202024]'
            }`}
            title="设置操作系统系统代理 (127.0.0.1:7890 HTTP/SOCKS5)"
          >
            <div className="flex flex-col text-left">
              <span className="text-[11px] font-semibold leading-tight">系统代理</span>
              <span className="text-[9px] font-mono text-zinc-400 leading-tight">
                {clashSettings.systemProxyEnabled ? ':7890 运行中' : '未设置'}
              </span>
            </div>
            {/* Custom Toggle Switch */}
            <div
              className={`w-7 h-4 rounded-full transition-colors relative flex items-center p-0.5 ${
                clashSettings.systemProxyEnabled ? 'bg-blue-500' : 'bg-zinc-700'
              }`}
            >
              <div
                className={`w-3 h-3 rounded-full bg-white shadow-xs transition-transform ${
                  clashSettings.systemProxyEnabled ? 'translate-x-3' : 'translate-x-0'
                }`}
              />
            </div>
          </div>

          {/* 虚拟网卡代理 (TUN 模式) */}
          <div
            onClick={() =>
              onUpdateSettings({
                ...clashSettings,
                tunEnabled: !clashSettings.tunEnabled,
              })
            }
            className={`flex items-center gap-2 px-2.5 py-1.5 rounded-lg border cursor-pointer transition-all select-none ${
              clashSettings.tunEnabled
                ? 'bg-emerald-500/15 border-emerald-500/40 text-emerald-300'
                : 'bg-[#18181A] border-[#2C2C30] text-zinc-400 hover:bg-[#202024]'
            }`}
            title="启用 MetaTUN 虚拟网卡代理 (接管全系统所有 TCP/UDP/ICMP 流量，含终端、Docker 及 Git)"
          >
            <div className="flex flex-col text-left">
              <span className="text-[11px] font-semibold leading-tight flex items-center gap-1">
                <Network className="w-3 h-3 text-emerald-400" />
                <span>虚拟网卡 (TUN)</span>
              </span>
              <span className="text-[9px] font-mono text-zinc-400 leading-tight">
                {clashSettings.tunEnabled ? 'Fake-IP / gVisor' : '未启用'}
              </span>
            </div>
            {/* Custom Toggle Switch */}
            <div
              className={`w-7 h-4 rounded-full transition-colors relative flex items-center p-0.5 ${
                clashSettings.tunEnabled ? 'bg-emerald-500' : 'bg-zinc-700'
              }`}
            >
              <div
                className={`w-3 h-3 rounded-full bg-white shadow-xs transition-transform ${
                  clashSettings.tunEnabled ? 'translate-x-3' : 'translate-x-0'
                }`}
              />
            </div>
          </div>

          {/* Live Speed Indicators */}
          <div className="hidden lg:flex items-center gap-2 bg-[#121214] px-2.5 py-1 rounded-lg border border-[#2E2E34] text-[11px] font-mono">
            <div className="flex items-center gap-1 text-cyan-400" title="实时上行速度">
              <ArrowUp className="w-3 h-3" />
              <span>{upSpeed}</span>
            </div>
            <span className="text-zinc-600">|</span>
            <div className="flex items-center gap-1 text-emerald-400" title="实时下行速度">
              <ArrowDown className="w-3 h-3" />
              <span>{downSpeed}</span>
            </div>
          </div>

          {/* One-click Ping test all button */}
          <button
            type="button"
            onClick={handleTestAllNodes}
            disabled={testingAll}
            className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-zinc-800 hover:bg-zinc-700 border border-zinc-700 text-zinc-200 text-xs font-medium transition-colors disabled:opacity-50"
            title="一键测速全部节点延迟"
          >
            <Zap className={`w-3.5 h-3.5 text-amber-400 ${testingAll ? 'animate-bounce' : ''}`} />
            <span className="hidden sm:inline">{testingAll ? '测速中...' : '一键测速'}</span>
          </button>
        </div>
      </header>

      {/* =========================================================================
          NAVIGATION TABS
         ========================================================================= */}
      <nav className="h-10 px-4 flex items-center justify-between border-b border-[#26262A] bg-[#161619] shrink-0">
        <div className="flex items-center gap-1">
          {[
            {
              id: 'proxies',
              label: '节点与代理组',
              icon: Radio,
              badge: `${subscription.nodes.length} 节点`,
            },
            {
              id: 'rules',
              label: '分流规则',
              icon: Sliders,
              badge: `${subscription.rules.length} 规则`,
            },
            {
              id: 'config',
              label: '配置源码 (YAML)',
              icon: Terminal,
            },
            {
              id: 'connections',
              label: '实时连接与日志',
              icon: Activity,
              badge: `${connections.length} 活跃`,
            },
          ].map((tab) => {
            const isActive = activeTab === tab.id;
            const Icon = tab.icon;
            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => setActiveTab(tab.id as any)}
                className={`flex items-center gap-2 px-3 py-1.5 rounded-lg text-xs font-medium transition-all ${
                  isActive
                    ? 'bg-[#242428] text-cyan-300 border border-[#3A3A40] shadow-xs'
                    : 'text-zinc-400 hover:text-zinc-200 hover:bg-[#1E1E22]'
                }`}
              >
                <Icon className="w-3.5 h-3.5" />
                <span>{tab.label}</span>
                {tab.badge && (
                  <span
                    className={`text-[10px] px-1.5 py-0.2 rounded-full font-mono ${
                      isActive ? 'bg-cyan-500/20 text-cyan-300' : 'bg-zinc-800 text-zinc-400'
                    }`}
                  >
                    {tab.badge}
                  </span>
                )}
              </button>
            );
          })}
        </div>

        {/* Status notification banner */}
        <div className="hidden md:flex items-center gap-2 text-[11px] text-zinc-400 font-mono">
          <span>Clash.Meta 端口: HTTP 7890</span>
          <span>•</span>
          <span>TUN: {clashSettings.tunEnabled ? 'ON (utun/MetaTUN)' : 'OFF'}</span>
        </div>
      </nav>

      {/* =========================================================================
          TAB 1: 节点选择与代理组 (PROXIES & GROUPS)
         ========================================================================= */}
      {activeTab === 'proxies' && (
        <div className="flex-1 flex flex-col min-h-0 bg-[#141416]">
          {/* Proxy Groups Filter Bar */}
          <div className="p-3 border-b border-[#26262A] bg-[#18181B] flex flex-wrap items-center justify-between gap-2 shrink-0">
            {/* Groups list chips */}
            <div className="flex items-center gap-1.5 overflow-x-auto py-0.5 max-w-full">
              {subscription.proxyGroups.map((group) => {
                const isSelected = group.id === activeGroupId;
                return (
                  <button
                    key={group.id}
                    type="button"
                    onClick={() => setActiveGroupId(group.id)}
                    className={`px-3 py-1 rounded-lg text-xs font-medium whitespace-nowrap transition-all flex items-center gap-1.5 border ${
                      isSelected
                        ? 'bg-cyan-500/20 text-cyan-300 border-cyan-500/40 shadow-xs'
                        : 'bg-[#1E1E22] text-zinc-400 border-[#2E2E34] hover:text-white hover:bg-[#25252A]'
                    }`}
                  >
                    <span>{group.name}</span>
                    <span className="text-[10px] px-1 rounded bg-black/40 font-mono text-zinc-400">
                      {group.proxies.length}
                    </span>
                  </button>
                );
              })}
            </div>

            {/* Current Active Node in Group */}
            {activeGroup && (
              <div className="flex items-center gap-1.5 text-xs text-zinc-400">
                <span>当前选定:</span>
                <span className="text-cyan-300 font-semibold bg-cyan-500/10 px-2 py-0.5 rounded border border-cyan-500/30">
                  {activeGroup.selectedNode}
                </span>
              </div>
            )}
          </div>

          {/* Search & Region / Protocol Filters */}
          <div className="px-3 py-2 border-b border-[#26262A] bg-[#161619] flex flex-wrap items-center justify-between gap-2 shrink-0 text-xs">
            {/* Search */}
            <div className="relative flex-1 min-w-[200px] max-w-md">
              <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-500" />
              <input
                type="text"
                value={nodeSearch}
                onChange={(e) => setNodeSearch(e.target.value)}
                placeholder="搜索节点名称、服务器地址 (如 香港、Hysteria2)..."
                className="w-full bg-[#121214] border border-[#2E2E34] rounded-lg pl-8 pr-3 py-1.5 text-xs text-zinc-200 placeholder-zinc-500 focus:outline-none focus:border-cyan-500"
              />
            </div>

            {/* Region & Protocol Pills */}
            <div className="flex items-center gap-2">
              {/* Region */}
              <div className="flex items-center gap-1 bg-[#121214] p-0.5 rounded-lg border border-[#2E2E34]">
                {['ALL', 'HK', 'JP', 'US', 'SG'].map((reg) => (
                  <button
                    key={reg}
                    type="button"
                    onClick={() => setSelectedRegion(reg)}
                    className={`px-2 py-0.5 rounded text-[11px] font-medium transition-colors ${
                      selectedRegion === reg
                        ? 'bg-zinc-700 text-white shadow-xs'
                        : 'text-zinc-400 hover:text-zinc-200'
                    }`}
                  >
                    {reg === 'ALL' ? '全部' : reg}
                  </button>
                ))}
              </div>

              {/* Protocol Filter */}
              <select
                value={selectedProtocol}
                onChange={(e) => setSelectedProtocol(e.target.value)}
                className="bg-[#121214] border border-[#2E2E34] rounded-lg px-2.5 py-1 text-xs text-zinc-300 focus:outline-none focus:border-cyan-500"
              >
                <option value="ALL">全部协议</option>
                <option value="hysteria2">Hysteria2 (QUIC)</option>
                <option value="vless">VLESS (Reality)</option>
                <option value="trojan">Trojan</option>
                <option value="vmess">VMess</option>
                <option value="ss">Shadowsocks</option>
                <option value="tuic">TUIC v5</option>
              </select>
            </div>
          </div>

          {/* Nodes Grid */}
          <div className="flex-1 overflow-y-auto p-3 min-h-0">
            {filteredNodes.length === 0 ? (
              <div className="h-60 flex flex-col items-center justify-center text-zinc-500 space-y-2">
                <Radio className="w-8 h-8 text-zinc-600" />
                <p className="text-xs">未找到符合条件的节点</p>
              </div>
            ) : (
              <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-2.5">
                {filteredNodes.map((node) => {
                  const isSelectedInGroup = activeGroup?.selectedNode === node.name;
                  const isTesting = testingNodeId === node.id || testingAll;

                  // Latency color
                  let latencyColor = 'text-zinc-400 bg-zinc-800/80 border-zinc-700';
                  if (node.latency !== undefined && node.latency !== null) {
                    if (node.latency === -1) {
                      latencyColor = 'text-red-400 bg-red-500/10 border-red-500/30';
                    } else if (node.latency < 60) {
                      latencyColor = 'text-emerald-400 bg-emerald-500/10 border-emerald-500/30';
                    } else if (node.latency < 140) {
                      latencyColor = 'text-cyan-400 bg-cyan-500/10 border-cyan-500/30';
                    } else {
                      latencyColor = 'text-amber-400 bg-amber-500/10 border-amber-500/30';
                    }
                  }

                  return (
                    <div
                      key={node.id}
                      onClick={() => handleSelectNodeForGroup(node.name)}
                      className={`group relative rounded-xl border p-3 cursor-pointer transition-all duration-150 flex flex-col justify-between ${
                        isSelectedInGroup
                          ? 'bg-[#22242B] border-cyan-500/60 shadow-md ring-1 ring-cyan-500/30'
                          : 'bg-[#18181B] border-[#2C2C32] hover:bg-[#1E1E22] hover:border-[#383840]'
                      }`}
                    >
                      {/* Top: Name, country flag, protocol */}
                      <div>
                        <div className="flex items-start justify-between gap-2 mb-1.5">
                          <div className="flex items-center gap-2 min-w-0">
                            {node.flag && <span className="text-base shrink-0">{node.flag}</span>}
                            <h4
                              className={`text-xs font-semibold truncate ${
                                isSelectedInGroup ? 'text-white' : 'text-zinc-200 group-hover:text-white'
                              }`}
                              title={node.name}
                            >
                              {node.name}
                            </h4>
                          </div>

                          {/* Selected Radio Badge */}
                          <div
                            className={`w-4 h-4 rounded-full border flex items-center justify-center shrink-0 transition-colors ${
                              isSelectedInGroup
                                ? 'bg-cyan-500 border-cyan-400 text-black'
                                : 'border-zinc-600 group-hover:border-zinc-400'
                            }`}
                          >
                            {isSelectedInGroup && <Check className="w-2.5 h-2.5 stroke-[3]" />}
                          </div>
                        </div>

                        {/* Node Specs & Protocol Badge */}
                        <div className="flex items-center gap-1.5 flex-wrap text-[10px] text-zinc-400 mb-2">
                          <span
                            className={`px-1.5 py-0.5 rounded border uppercase font-mono font-semibold ${getProtocolBadge(
                              node.type
                            )}`}
                          >
                            {node.type}
                          </span>
                          <span className="font-mono text-zinc-400">
                            {node.server}:{node.port}
                          </span>
                          {node.network && (
                            <span className="px-1 rounded bg-zinc-800 text-zinc-400 font-mono">
                              {node.network}
                            </span>
                          )}
                          {node.udp && (
                            <span className="px-1 rounded bg-purple-500/10 text-purple-300 font-mono">
                              UDP
                            </span>
                          )}
                        </div>
                      </div>

                      {/* Bottom: Ping latency & individual test button */}
                      <div className="flex items-center justify-between pt-2 border-t border-[#26262B] text-[11px]">
                        <div
                          className={`flex items-center gap-1 px-2 py-0.5 rounded-full border text-[10px] font-mono ${latencyColor}`}
                        >
                          <span
                            className={`w-1.5 h-1.5 rounded-full ${
                              node.latency === -1
                                ? 'bg-red-400'
                                : node.latency && node.latency < 60
                                ? 'bg-emerald-400 animate-pulse'
                                : 'bg-cyan-400'
                            }`}
                          />
                          <span>
                            {isTesting
                              ? '测试中...'
                              : node.latency === -1
                              ? '超时'
                              : node.latency
                              ? `${node.latency} ms`
                              : '未测速'}
                          </span>
                        </div>

                        {/* Test button */}
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleTestNode(node.id);
                          }}
                          disabled={isTesting}
                          className="p-1 hover:bg-[#2A2A30] text-zinc-400 hover:text-cyan-400 rounded transition-colors"
                          title="测试此节点延迟"
                        >
                          <RefreshCw
                            className={`w-3.5 h-3.5 ${isTesting ? 'animate-spin text-cyan-400' : ''}`}
                          />
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      )}

      {/* =========================================================================
          TAB 2: 分流规则 (ROUTING RULES)
         ========================================================================= */}
      {activeTab === 'rules' && (
        <div className="flex-1 flex flex-col min-h-0 bg-[#141416]">
          {/* Rules Summary & Interactive Domain Tester */}
          <div className="p-3 border-b border-[#26262A] bg-[#18181B] shrink-0 space-y-2.5">
            {/* Top Interactive Router Lookup */}
            <form
              onSubmit={handleTestDomainRouting}
              className="flex items-center gap-2 bg-[#121214] p-2 rounded-xl border border-[#2E2E34]"
            >
              <div className="flex items-center gap-2 pl-1 text-xs text-cyan-400 font-medium shrink-0">
                <Sparkles className="w-3.5 h-3.5" />
                <span>分流探测器:</span>
              </div>
              <input
                type="text"
                value={testDomain}
                onChange={(e) => setTestDomain(e.target.value)}
                placeholder="输入目标域名测试分流走向，如 github.com 或 baidu.com"
                className="flex-1 bg-transparent text-xs text-white placeholder-zinc-500 focus:outline-none font-mono"
              />
              <button
                type="submit"
                className="px-3 py-1 rounded-lg bg-cyan-600 hover:bg-cyan-500 text-white text-xs font-medium transition-colors shrink-0"
              >
                测试分流
              </button>
            </form>

            {/* Test Result Display */}
            {testResult && (
              <div className="p-2.5 rounded-lg bg-cyan-500/10 border border-cyan-500/30 text-xs flex items-center justify-between animate-in fade-in">
                <div className="flex items-center gap-2">
                  <Check className="w-4 h-4 text-cyan-400" />
                  <span>
                    命中规则: <strong className="text-white font-mono">{testResult.matchedRule.type}</strong>{' '}
                    <span className="text-cyan-300 font-mono">{testResult.matchedRule.payload || 'DEFAULT'}</span>
                  </span>
                </div>
                <div className="flex items-center gap-1.5">
                  <span className="text-zinc-400">走向目标:</span>
                  <span className="px-2 py-0.5 rounded bg-cyan-500/20 text-cyan-300 font-semibold border border-cyan-500/40">
                    {testResult.destPolicy}
                  </span>
                </div>
              </div>
            )}
          </div>

          {/* Search & Type Filters */}
          <div className="px-3 py-2 border-b border-[#26262A] bg-[#161619] flex items-center justify-between gap-2 shrink-0 text-xs">
            <div className="relative flex-1 max-w-sm">
              <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-500" />
              <input
                type="text"
                value={ruleSearch}
                onChange={(e) => setRuleSearch(e.target.value)}
                placeholder="搜索规则匹配内容或目标..."
                className="w-full bg-[#121214] border border-[#2E2E34] rounded-lg pl-8 pr-3 py-1 text-xs text-zinc-200 placeholder-zinc-500 focus:outline-none focus:border-cyan-500"
              />
            </div>

            <div className="flex items-center gap-1.5 text-zinc-400 text-xs">
              <span>规则总计:</span>
              <span className="font-mono text-zinc-200 font-semibold">{subscription.rules.length} 条</span>
            </div>
          </div>

          {/* Rules Table */}
          <div className="flex-1 overflow-y-auto p-3 min-h-0">
            <table className="w-full text-left text-xs border-collapse">
              <thead>
                <tr className="border-b border-[#2C2C32] text-zinc-400 text-[11px] uppercase font-semibold">
                  <th className="pb-2 w-12 text-center">#</th>
                  <th className="pb-2 w-36">规则类型 (Type)</th>
                  <th className="pb-2">匹配特征 (Payload)</th>
                  <th className="pb-2 w-48">目标走向 (Target Policy)</th>
                  <th className="pb-2 w-24 text-right">命中次数</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-[#202024]">
                {subscription.rules
                  .filter(
                    (r) =>
                      r.payload.toLowerCase().includes(ruleSearch.toLowerCase()) ||
                      r.target.toLowerCase().includes(ruleSearch.toLowerCase()) ||
                      r.type.toLowerCase().includes(ruleSearch.toLowerCase())
                  )
                  .map((rule, idx) => {
                    let targetBadge = 'bg-zinc-800 text-zinc-300 border-zinc-700';
                    if (rule.target === 'DIRECT') {
                      targetBadge = 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30';
                    } else if (rule.target === 'REJECT') {
                      targetBadge = 'bg-red-500/15 text-red-300 border-red-500/30';
                    } else if (rule.target.includes('节点') || rule.target.includes('PROXY')) {
                      targetBadge = 'bg-cyan-500/15 text-cyan-300 border-cyan-500/30';
                    }

                    return (
                      <tr key={rule.id || idx} className="hover:bg-[#1A1A1E] transition-colors">
                        <td className="py-2 text-center text-zinc-500 font-mono text-[10px]">{idx + 1}</td>
                        <td className="py-2">
                          <span className="px-1.5 py-0.5 rounded bg-zinc-800 text-zinc-300 font-mono text-[10px] border border-zinc-700">
                            {rule.type}
                          </span>
                        </td>
                        <td className="py-2 font-mono text-zinc-200">
                          {rule.payload ? (
                            rule.payload
                          ) : (
                            <span className="text-zinc-500 italic">全量匹配 (Fallback)</span>
                          )}
                        </td>
                        <td className="py-2">
                          <span
                            className={`px-2 py-0.5 rounded border text-[11px] font-semibold ${targetBadge}`}
                          >
                            {rule.target}
                          </span>
                        </td>
                        <td className="py-2 text-right font-mono text-zinc-400 text-[11px]">
                          {rule.hits ? rule.hits.toLocaleString() : 0} 次
                        </td>
                      </tr>
                    );
                  })}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* =========================================================================
          TAB 3: 配置源码 (YAML / RAW CONFIG)
         ========================================================================= */}
      {activeTab === 'config' && (
        <div className="flex-1 flex flex-col min-h-0 bg-[#141416]">
          {/* Header Actions */}
          <div className="p-3 border-b border-[#26262A] bg-[#18181B] flex items-center justify-between shrink-0">
            <div className="flex items-center gap-2">
              <Terminal className="w-4 h-4 text-cyan-400" />
              <span className="text-xs font-semibold text-zinc-200">
                Clash.Meta (Mihomo) YAML 核心配置文件
              </span>
            </div>

            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => {
                  if (subscription.rawYaml) {
                    navigator.clipboard.writeText(subscription.rawYaml);
                  }
                }}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[#242428] hover:bg-[#2C2C32] text-zinc-200 text-xs font-medium border border-[#3A3A40] transition-colors"
              >
                <Copy className="w-3.5 h-3.5 text-cyan-400" />
                <span>复制 YAML</span>
              </button>
              <button
                type="button"
                onClick={() => {
                  const blob = new Blob([subscription.rawYaml || ''], { type: 'text/yaml' });
                  const url = URL.createObjectURL(blob);
                  const a = document.createElement('a');
                  a.href = url;
                  a.download = `clash-meta-${subscription.id}.yaml`;
                  a.click();
                  URL.revokeObjectURL(url);
                }}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-cyan-600 hover:bg-cyan-500 text-white text-xs font-medium shadow-xs transition-colors"
              >
                <Download className="w-3.5 h-3.5" />
                <span>下载配置</span>
              </button>
            </div>
          </div>

          {/* YAML Code Area */}
          <div className="flex-1 overflow-y-auto p-4 bg-[#101012] font-mono text-xs leading-relaxed text-zinc-300">
            <pre className="whitespace-pre font-mono">
              {subscription.rawYaml ||
                `# Clash.Meta Config: ${subscription.name}\nmixed-port: 7890\nmode: rule\nlog-level: info\n`}
            </pre>
          </div>
        </div>
      )}

      {/* =========================================================================
          TAB 4: 实时连接与核心日志 (CONNECTIONS & LOGS)
         ========================================================================= */}
      {activeTab === 'connections' && (
        <div className="flex-1 flex flex-col min-h-0 bg-[#141416]">
          {/* Active Connections Panel */}
          <div className="flex-1 flex flex-col min-h-0 border-b border-[#26262A]">
            <div className="p-2.5 bg-[#18181B] border-b border-[#26262A] flex items-center justify-between text-xs font-semibold text-zinc-200">
              <div className="flex items-center gap-2">
                <Activity className="w-4 h-4 text-emerald-400" />
                <span>活跃网络连接监控 (Active Connections)</span>
                <span className="px-1.5 py-0.2 rounded bg-zinc-800 text-emerald-400 font-mono text-[10px]">
                  {connections.length} 活跃
                </span>
              </div>
              <button
                type="button"
                onClick={() => setConnections([])}
                className="text-[11px] text-zinc-400 hover:text-red-400 flex items-center gap-1"
              >
                <Trash2 className="w-3 h-3" />
                <span>关闭全部连接</span>
              </button>
            </div>

            <div className="flex-1 overflow-y-auto p-2">
              <table className="w-full text-left text-xs border-collapse font-mono">
                <thead>
                  <tr className="border-b border-[#28282C] text-zinc-500 text-[10px] uppercase">
                    <th className="pb-1.5">目标主机 (Host)</th>
                    <th className="pb-1.5">进程</th>
                    <th className="pb-1.5">命中规则</th>
                    <th className="pb-1.5">代理链条走向</th>
                    <th className="pb-1.5 text-right">实时速率</th>
                    <th className="pb-1.5 w-10"></th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-[#1E1E22]">
                  {connections.map((c) => (
                    <tr key={c.id} className="hover:bg-[#1A1A1E]">
                      <td className="py-2 text-zinc-200 font-semibold">{c.host}</td>
                      <td className="py-2 text-cyan-300 text-[11px]">{c.process}</td>
                      <td className="py-2 text-zinc-400 text-[11px]">{c.rule}</td>
                      <td className="py-2 text-purple-300 text-[11px]">{c.proxyChain.join(' ➔ ')}</td>
                      <td className="py-2 text-right text-emerald-400">{c.speed}</td>
                      <td className="py-2 text-right">
                        <button
                          type="button"
                          onClick={() => setConnections((prev) => prev.filter((item) => item.id !== c.id))}
                          className="text-zinc-500 hover:text-red-400 p-1"
                          title="中断连接"
                        >
                          <Trash2 className="w-3 h-3" />
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>

          {/* Clash.Meta Core Live Logs */}
          <div className="h-44 flex flex-col bg-[#101012] shrink-0 font-mono text-xs">
            <div className="px-3 py-1.5 bg-[#18181A] border-b border-[#26262A] flex items-center justify-between text-[11px] text-zinc-400">
              <div className="flex items-center gap-1.5">
                <Terminal className="w-3.5 h-3.5 text-zinc-500" />
                <span>Mihomo Core 日志输出</span>
              </div>
              <button
                type="button"
                onClick={() => setLogs([])}
                className="text-[10px] text-zinc-400 hover:text-zinc-200"
              >
                清空日志
              </button>
            </div>
            <div className="flex-1 overflow-y-auto p-2.5 space-y-1 text-[11px]">
              {logs.map((log) => (
                <div key={log.id} className="flex items-start gap-2 leading-relaxed">
                  <span className="text-zinc-500 shrink-0">{log.time}</span>
                  <span
                    className={`px-1 py-0.2 rounded text-[9px] uppercase font-bold shrink-0 ${
                      log.level === 'info'
                        ? 'bg-blue-500/20 text-blue-400'
                        : log.level === 'warn'
                        ? 'bg-amber-500/20 text-amber-400'
                        : 'bg-red-500/20 text-red-400'
                    }`}
                  >
                    {log.level}
                  </span>
                  <span className="text-zinc-300 font-mono">{log.msg}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
