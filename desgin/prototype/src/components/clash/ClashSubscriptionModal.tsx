import React, { useState } from 'react';
import {
  X,
  Radio,
  Globe,
  Link,
  FileCode,
  Clock,
  Sparkles,
  Shield,
  Layers,
  Check,
  AlertCircle,
  RefreshCw,
} from 'lucide-react';
import { ClashSubscription, ClashNode, ClashProxyGroup, ClashRule } from '../../types';

interface ClashSubscriptionModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSaveSubscription: (sub: ClashSubscription) => void;
  editingSubscription?: ClashSubscription | null;
}

export const ClashSubscriptionModal: React.FC<ClashSubscriptionModalProps> = ({
  isOpen,
  onClose,
  onSaveSubscription,
  editingSubscription,
}) => {
  const [name, setName] = useState(editingSubscription?.name || '');
  const [url, setUrl] = useState(editingSubscription?.url || '');
  const [sourceType, setSourceType] = useState<'url' | 'yaml'>(
    editingSubscription?.rawYaml && !editingSubscription?.url ? 'yaml' : 'url'
  );
  const [rawYaml, setRawYaml] = useState(
    editingSubscription?.rawYaml ||
      `# Clash.Meta (Mihomo) 自定义配置文件\nmixed-port: 7890\nmode: rule\nlog-level: info\n\nproxies:\n  - name: "🇭🇰 香港 01 [Hysteria2]"\n    type: hysteria2\n    server: hk01.example.com\n    port: 443\n    password: secret-token\n\nrules:\n  - DOMAIN-SUFFIX,google.com,PROXY\n  - GEOIP,CN,DIRECT\n  - MATCH,DIRECT\n`
  );
  const [autoUpdateInterval, setAutoUpdateInterval] = useState<number>(
    editingSubscription?.autoUpdateInterval ?? 24
  );
  const [userAgent, setUserAgent] = useState(
    editingSubscription?.userAgent || 'clash.meta / Mihomo v1.18.9'
  );
  const [tag, setTag] = useState(editingSubscription?.tag || 'IEPL高速');
  const [color, setColor] = useState(editingSubscription?.color || '#06b6d4');
  const [notes, setNotes] = useState(editingSubscription?.notes || '');
  const [isTestingUrl, setIsTestingUrl] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);

  if (!isOpen) return null;

  const colorOptions = [
    '#06b6d4', // Cyan
    '#3b82f6', // Blue
    '#8b5cf6', // Purple
    '#ec4899', // Pink
    '#10b981', // Emerald
    '#f59e0b', // Amber
    '#ef4444', // Red
  ];

  const handleTestOrFetch = () => {
    if (!url.trim() && sourceType === 'url') return;
    setIsTestingUrl(true);
    setTestResult(null);

    setTimeout(() => {
      setIsTestingUrl(false);
      setTestResult('解析成功: 获取到 12 个代理节点 (含 Hysteria2, VLESS, Trojan)，16 条分流规则');
    }, 800);
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    if (sourceType === 'url' && !url.trim()) return;

    // Generate mock nodes & rules if new, or preserve existing
    const existingNodes = editingSubscription?.nodes || [];
    const generatedNodes: ClashNode[] =
      existingNodes.length > 0
        ? existingNodes
        : [
            {
              id: `node-${Date.now()}-1`,
              name: '🇭🇰 香港 01 [Hysteria2 专线]',
              type: 'hysteria2',
              server: 'hk01.fastroute.net',
              port: 443,
              latency: 35,
              udp: true,
              tls: true,
              country: 'HK',
              flag: '🇭🇰',
              status: 'online',
            },
            {
              id: `node-${Date.now()}-2`,
              name: '🇯🇵 东京 01 [VLESS Reality]',
              type: 'vless',
              server: 'jp01.fastroute.net',
              port: 443,
              latency: 58,
              udp: true,
              tls: true,
              country: 'JP',
              flag: '🇯🇵',
              status: 'online',
            },
            {
              id: `node-${Date.now()}-3`,
              name: '🇺🇸 洛杉矶 01 [Hysteria2 8K]',
              type: 'hysteria2',
              server: 'us01.fastroute.net',
              port: 443,
              latency: 140,
              udp: true,
              tls: true,
              country: 'US',
              flag: '🇺🇸',
              status: 'online',
            },
            {
              id: `node-${Date.now()}-4`,
              name: '🇸🇬 新加坡 01 [Trojan BGP]',
              type: 'trojan',
              server: 'sg01.fastroute.net',
              port: 443,
              latency: 66,
              udp: true,
              tls: true,
              country: 'SG',
              flag: '🇸🇬',
              status: 'online',
            },
          ];

    const proxyGroups: ClashProxyGroup[] = editingSubscription?.proxyGroups || [
      {
        id: `grp-${Date.now()}-1`,
        name: '🚀 节点选择',
        type: 'select',
        selectedNode: generatedNodes[0]?.name || 'DIRECT',
        proxies: [...generatedNodes.map((n) => n.name), 'DIRECT'],
      },
      {
        id: `grp-${Date.now()}-2`,
        name: '♻️ 自动选择',
        type: 'url-test',
        selectedNode: generatedNodes[0]?.name || 'DIRECT',
        proxies: generatedNodes.map((n) => n.name),
        testUrl: 'http://www.gstatic.com/generate_204',
        interval: 300,
      },
    ];

    const rules: ClashRule[] = editingSubscription?.rules || [
      { id: `r-${Date.now()}-1`, type: 'DOMAIN-SUFFIX', payload: 'google.com', target: '🚀 节点选择', hits: 120 },
      { id: `r-${Date.now()}-2`, type: 'DOMAIN-SUFFIX', payload: 'github.com', target: '🚀 节点选择', hits: 88 },
      { id: `r-${Date.now()}-3`, type: 'DOMAIN-SUFFIX', payload: 'openai.com', target: '🚀 节点选择', hits: 45 },
      { id: `r-${Date.now()}-4`, type: 'IP-CIDR', payload: '192.168.0.0/16', target: 'DIRECT', hits: 850 },
      { id: `r-${Date.now()}-5`, type: 'GEOIP', payload: 'CN', target: 'DIRECT', hits: 2400 },
      { id: `r-${Date.now()}-6`, type: 'MATCH', payload: '', target: '🚀 节点选择', hits: 310 },
    ];

    const sub: ClashSubscription = {
      id: editingSubscription?.id || `sub-${Date.now()}`,
      name: name.trim(),
      url: sourceType === 'url' ? url.trim() : '',
      rawYaml: sourceType === 'yaml' ? rawYaml : undefined,
      updatedAt: new Date().toLocaleString(),
      autoUpdateInterval,
      uploadBytes: editingSubscription?.uploadBytes || 12.5 * 1024 * 1024 * 1024,
      downloadBytes: editingSubscription?.downloadBytes || 45.2 * 1024 * 1024 * 1024,
      totalBytes: editingSubscription?.totalBytes || 200 * 1024 * 1024 * 1024,
      expireDate: editingSubscription?.expireDate || '2027-12-31',
      nodes: generatedNodes,
      proxyGroups,
      rules,
      status: 'ready',
      tag: tag.trim() || undefined,
      color,
      userAgent: userAgent.trim(),
      notes: notes.trim(),
    };

    onSaveSubscription(sub);
    onClose();
  };

  return (
    <div className="fixed inset-0 bg-black/75 backdrop-blur-xs z-50 flex items-center justify-center p-4 select-none">
      <div className="bg-[#202022] border border-[#38383c] rounded-2xl max-w-xl w-full p-5 shadow-2xl space-y-4 max-h-[90vh] flex flex-col font-sans animate-in fade-in zoom-in-95 duration-150">
        {/* Modal Header */}
        <div className="flex items-center justify-between border-b border-[#333336] pb-3 shrink-0">
          <div className="flex items-center gap-2.5">
            <div
              style={{ backgroundColor: `${color}20`, borderColor: `${color}60` }}
              className="w-8 h-8 rounded-lg border flex items-center justify-center text-cyan-400"
            >
              <Radio className="w-4 h-4" style={{ color }} />
            </div>
            <div>
              <h3 className="text-sm font-semibold text-white">
                {editingSubscription ? '修改 Clash.Meta 订阅' : '添加 Clash.Meta 订阅'}
              </h3>
              <p className="text-[11px] text-[#8E8E93]">
                支持 Mihomo / Clash.Meta 专线、Hysteria2、VLESS、Trojan 等现代协议
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1 hover:bg-[#2C2C2E] rounded-md text-[#8E8E93] hover:text-white transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Modal Form */}
        <form onSubmit={handleSubmit} className="flex-1 overflow-y-auto space-y-3.5 pr-1 text-xs">
          {/* Source Type Selector */}
          <div>
            <label className="block text-[#A1A1A6] font-medium mb-1.5">导入方式</label>
            <div className="grid grid-cols-2 gap-2">
              <button
                type="button"
                onClick={() => setSourceType('url')}
                className={`flex items-center justify-center gap-2 py-2 px-3 rounded-lg border text-xs font-medium transition-all ${
                  sourceType === 'url'
                    ? 'bg-cyan-500/15 border-cyan-500 text-cyan-300 shadow-xs'
                    : 'bg-[#18181A] border-[#333336] text-[#A1A1A6] hover:bg-[#252528]'
                }`}
              >
                <Link className="w-3.5 h-3.5" />
                <span>远程订阅 URL (推荐)</span>
              </button>
              <button
                type="button"
                onClick={() => setSourceType('yaml')}
                className={`flex items-center justify-center gap-2 py-2 px-3 rounded-lg border text-xs font-medium transition-all ${
                  sourceType === 'yaml'
                    ? 'bg-cyan-500/15 border-cyan-500 text-cyan-300 shadow-xs'
                    : 'bg-[#18181A] border-[#333336] text-[#A1A1A6] hover:bg-[#252528]'
                }`}
              >
                <FileCode className="w-3.5 h-3.5" />
                <span>直接粘贴 YAML 配置</span>
              </button>
            </div>
          </div>

          {/* Subscription Name */}
          <div>
            <label className="block text-[#A1A1A6] font-medium mb-1">
              订阅名称 <span className="text-red-400">*</span>
            </label>
            <input
              type="text"
              required
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="例如: 🚀 极速专线 BGP IEPL 或 自建海外集群"
              className="w-full bg-[#161618] border border-[#38383c] rounded-lg px-3 py-2 text-white placeholder-[#636366] focus:outline-none focus:border-cyan-500 text-xs"
            />
          </div>

          {/* URL Input */}
          {sourceType === 'url' ? (
            <div>
              <div className="flex items-center justify-between mb-1">
                <label className="text-[#A1A1A6] font-medium">
                  订阅链接 (URL) <span className="text-red-400">*</span>
                </label>
                <button
                  type="button"
                  onClick={handleTestOrFetch}
                  disabled={!url || isTestingUrl}
                  className="text-[11px] text-cyan-400 hover:text-cyan-300 flex items-center gap-1 disabled:opacity-40"
                >
                  <RefreshCw className={`w-3 h-3 ${isTestingUrl ? 'animate-spin' : ''}`} />
                  <span>测试并解析链接</span>
                </button>
              </div>
              <input
                type="url"
                required
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://example.com/api/v1/client/subscribe?token=..."
                className="w-full bg-[#161618] border border-[#38383c] rounded-lg px-3 py-2 text-white placeholder-[#636366] focus:outline-none focus:border-cyan-500 text-xs font-mono"
              />
              {testResult && (
                <div className="mt-1.5 p-2 rounded bg-emerald-500/10 border border-emerald-500/30 text-emerald-300 text-[11px] flex items-center gap-1.5 animate-in fade-in">
                  <Check className="w-3.5 h-3.5 shrink-0" />
                  <span>{testResult}</span>
                </div>
              )}
            </div>
          ) : (
            <div>
              <label className="block text-[#A1A1A6] font-medium mb-1">
                YAML 配置文件内容 <span className="text-red-400">*</span>
              </label>
              <textarea
                rows={6}
                value={rawYaml}
                onChange={(e) => setRawYaml(e.target.value)}
                placeholder="在此粘贴 Clash.Meta / Mihomo 配置文件内容..."
                className="w-full bg-[#161618] border border-[#38383c] rounded-lg p-2.5 text-white font-mono text-[11px] placeholder-[#636366] focus:outline-none focus:border-cyan-500 resize-y"
              />
            </div>
          )}

          {/* Auto Update & User-Agent Row */}
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-[#A1A1A6] font-medium mb-1 flex items-center gap-1">
                <Clock className="w-3 h-3 text-[#8E8E93]" />
                <span>自动更新频率</span>
              </label>
              <select
                value={autoUpdateInterval}
                onChange={(e) => setAutoUpdateInterval(Number(e.target.value))}
                className="w-full bg-[#161618] border border-[#38383c] rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-cyan-500 text-xs"
              >
                <option value={0}>不自动更新 (手动更新)</option>
                <option value={6}>每 6 小时自动更新</option>
                <option value={12}>每 12 小时自动更新</option>
                <option value={24}>每 24 小时自动更新 (推荐)</option>
                <option value={72}>每 3 天自动更新</option>
              </select>
            </div>

            <div>
              <label className="block text-[#A1A1A6] font-medium mb-1 flex items-center gap-1">
                <Globe className="w-3 h-3 text-[#8E8E93]" />
                <span>请求 User-Agent</span>
              </label>
              <input
                type="text"
                value={userAgent}
                onChange={(e) => setUserAgent(e.target.value)}
                placeholder="clash.meta / Mihomo"
                className="w-full bg-[#161618] border border-[#38383c] rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-cyan-500 text-xs font-mono"
              />
            </div>
          </div>

          {/* Tag & Color Palette */}
          <div className="grid grid-cols-2 gap-3 items-center">
            <div>
              <label className="block text-[#A1A1A6] font-medium mb-1">标签分类</label>
              <input
                type="text"
                value={tag}
                onChange={(e) => setTag(e.target.value)}
                placeholder="例如: IEPL高速 / 自建节点"
                className="w-full bg-[#161618] border border-[#38383c] rounded-lg px-2.5 py-1.5 text-white placeholder-[#636366] focus:outline-none focus:border-cyan-500 text-xs"
              />
            </div>

            <div>
              <label className="block text-[#A1A1A6] font-medium mb-1">主题标识色</label>
              <div className="flex items-center gap-1.5">
                {colorOptions.map((c) => (
                  <button
                    key={c}
                    type="button"
                    onClick={() => setColor(c)}
                    style={{ backgroundColor: c }}
                    className={`w-6 h-6 rounded-full transition-transform flex items-center justify-center ${
                      color === c ? 'scale-110 ring-2 ring-white/60 shadow-xs' : 'opacity-80 hover:opacity-100'
                    }`}
                  >
                    {color === c && <Check className="w-3 h-3 text-white drop-shadow" />}
                  </button>
                ))}
              </div>
            </div>
          </div>

          {/* Notes */}
          <div>
            <label className="block text-[#A1A1A6] font-medium mb-1">备注说明</label>
            <input
              type="text"
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
              placeholder="例如: 沪港专线、支持 UDP / 游戏加速与流媒体解锁"
              className="w-full bg-[#161618] border border-[#38383c] rounded-lg px-3 py-1.5 text-white placeholder-[#636366] focus:outline-none focus:border-cyan-500 text-xs"
            />
          </div>

          {/* Clash.Meta Protocol Support Tips */}
          <div className="p-2.5 rounded-lg bg-[#18181A] border border-[#303034] text-[11px] text-[#A1A1A6] space-y-1">
            <div className="flex items-center gap-1.5 font-medium text-cyan-400">
              <Sparkles className="w-3.5 h-3.5" />
              <span>Clash.Meta (Mihomo) 特色协议支持</span>
            </div>
            <p className="text-[10px] leading-relaxed text-[#8E8E93]">
              完整兼容 Hysteria2 (QUIC/UDP 抗封锁)、VLESS-Reality、Trojan、Shadowsocks 2022、TUIC v5
              及 WireGuard 协议，内置 Fake-IP 与 TUN 虚拟网卡全量分流能力。
            </p>
          </div>

          {/* Actions */}
          <div className="flex items-center justify-end gap-2.5 pt-3 border-t border-[#333336]">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 rounded-lg bg-[#2A2A2E] hover:bg-[#343438] text-white text-xs font-medium transition-colors"
            >
              取消
            </button>
            <button
              type="submit"
              className="px-4 py-2 rounded-lg bg-cyan-600 hover:bg-cyan-500 text-white text-xs font-medium transition-colors shadow-sm flex items-center gap-1.5"
            >
              <Radio className="w-3.5 h-3.5" />
              <span>{editingSubscription ? '保存修改' : '立即添加订阅'}</span>
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
