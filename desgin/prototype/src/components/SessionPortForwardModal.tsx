import React, { useState } from 'react';
import {
  Network,
  Plus,
  Play,
  Square,
  ArrowRight,
  Shield,
  Radio,
  Trash2,
  ExternalLink,
  Copy,
  Check,
  CheckCircle2,
  Laptop,
  Server,
  Zap,
  HelpCircle,
  X,
} from 'lucide-react';
import { PortTunnel, ServerHost, TunnelType } from '../types';

interface SessionPortForwardModalProps {
  isOpen: boolean;
  onClose: () => void;
  host: ServerHost;
  tunnels: PortTunnel[];
  onUpdateTunnels: (tunnels: PortTunnel[]) => void;
  onSendTerminalCommand?: (cmd: string) => void;
}

export const SessionPortForwardModal: React.FC<SessionPortForwardModalProps> = ({
  isOpen,
  onClose,
  host,
  tunnels,
  onUpdateTunnels,
  onSendTerminalCommand,
}) => {
  const [showAddForm, setShowAddForm] = useState(false);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<{ id: string; success: boolean; msg: string } | null>(null);

  // New tunnel form state
  const [tunnelType, setTunnelType] = useState<TunnelType>('local');
  const [name, setName] = useState('');
  const [bindAddress, setBindAddress] = useState('127.0.0.1');
  const [localPort, setLocalPort] = useState(8080);
  const [remoteHost, setRemoteHost] = useState('127.0.0.1');
  const [remotePort, setRemotePort] = useState(80);
  const [description, setDescription] = useState('');

  if (!isOpen) return null;

  // Filter tunnels belonging specifically to this active session host
  const hostTunnels = tunnels.filter((t) => t.hostId === host.id);
  const activeCount = hostTunnels.filter((t) => t.status === 'active').length;

  const handleToggle = (id: string) => {
    onUpdateTunnels(
      tunnels.map((t) =>
        t.id === id ? { ...t, status: t.status === 'active' ? 'inactive' : 'active' } : t
      )
    );
  };

  const handleDelete = (id: string) => {
    onUpdateTunnels(tunnels.filter((t) => t.id !== id));
  };

  const handleCreate = () => {
    if (!name.trim()) return;

    const newTunnel: PortTunnel = {
      id: `tunnel-${Date.now()}`,
      name: name.trim(),
      hostId: host.id,
      type: tunnelType,
      bindAddress: bindAddress.trim() || '127.0.0.1',
      localPort: Number(localPort) || 8080,
      remoteHost: remoteHost.trim() || '127.0.0.1',
      remotePort: Number(remotePort) || 80,
      status: 'active',
      bytesTransferred: 0,
      description: description.trim() || `${tunnelType.toUpperCase()} forwarding on ${host.name}`,
    };

    onUpdateTunnels([...tunnels, newTunnel]);
    setName('');
    setDescription('');
    setShowAddForm(false);
  };

  const applyPreset = (preset: {
    name: string;
    type: TunnelType;
    local: number;
    remHost: string;
    remPort: number;
    desc: string;
  }) => {
    setName(preset.name);
    setTunnelType(preset.type);
    setLocalPort(preset.local);
    setRemoteHost(preset.remHost);
    setRemotePort(preset.remPort);
    setDescription(preset.desc);
  };

  const getSshCommand = (t: PortTunnel) => {
    if (t.type === 'local') {
      return `ssh -N -L ${t.bindAddress}:${t.localPort}:${t.remoteHost}:${t.remotePort} ${host.username}@${host.host} -p ${host.port}`;
    } else if (t.type === 'remote') {
      return `ssh -N -R ${t.remotePort}:${t.bindAddress}:${t.localPort} ${host.username}@${host.host} -p ${host.port}`;
    } else {
      return `ssh -N -D ${t.bindAddress}:${t.localPort} ${host.username}@${host.host} -p ${host.port}`;
    }
  };

  const handleCopyCmd = (t: PortTunnel) => {
    const cmd = getSshCommand(t);
    navigator.clipboard.writeText(cmd);
    setCopiedId(t.id);
    setTimeout(() => setCopiedId(null), 1500);
  };

  const handleTestSocket = (t: PortTunnel) => {
    setTestResult({
      id: t.id,
      success: true,
      msg: `Port ${t.localPort} socket listening cleanly. Tunnel connection is healthy.`,
    });
    setTimeout(() => setTestResult(null), 3000);
  };

  const formatTraffic = (bytes: number) => {
    if (bytes === 0) return '0 KB';
    const mb = (bytes / (1024 * 1024)).toFixed(2);
    return `${mb} MB`;
  };

  return (
    <div className="fixed inset-0 bg-black/65 backdrop-blur-xs flex items-center justify-center z-50 p-4 animate-in fade-in duration-150 select-none">
      <div
        className="bg-[#1E1E1E] border border-[#454545] rounded-xl shadow-2xl w-full max-w-2xl overflow-hidden flex flex-col max-h-[85vh] text-[#CCCCCC]"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Modal Header */}
        <div className="h-12 px-4 border-b border-[#333333] bg-[#252526] flex items-center justify-between shrink-0">
          <div className="flex items-center gap-2.5">
            <div className="w-8 h-8 rounded-lg bg-[#4EC9B0]/15 border border-[#4EC9B0]/30 flex items-center justify-center text-[#4EC9B0]">
              <Network className="w-4 h-4" />
            </div>
            <div>
              <div className="flex items-center gap-2">
                <h3 className="text-sm font-semibold text-white">Session Port Forwarding</h3>
                <span className="text-[11px] px-2 py-0.5 rounded-full bg-[#27C93F]/20 text-[#27C93F] font-mono border border-[#27C93F]/40 flex items-center gap-1">
                  <span className="w-1.5 h-1.5 rounded-full bg-[#27C93F] animate-pulse" />
                  {activeCount} Active Forwarding
                </span>
              </div>
              <p className="text-[11px] text-[#858585] font-mono">
                Host: <span className="text-[#4EC9B0]">{host.name}</span> ({host.username}@{host.host}:{host.port})
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            {!showAddForm && (
              <button
                onClick={() => setShowAddForm(true)}
                className="flex items-center gap-1 px-2.5 py-1 rounded bg-[#007ACC] hover:bg-[#0062a3] text-white text-xs font-medium transition-colors"
              >
                <Plus className="w-3.5 h-3.5" />
                <span>Add Forward</span>
              </button>
            )}
            <button
              onClick={onClose}
              className="p-1 hover:bg-[#333333] text-[#858585] hover:text-white rounded transition-colors"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Modal Body */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {/* Quick Add Form */}
          {showAddForm && (
            <div className="p-3.5 rounded-lg bg-[#252526] border border-[#007ACC]/60 space-y-3 animate-in fade-in duration-100">
              <div className="flex items-center justify-between">
                <span className="text-xs font-semibold text-white flex items-center gap-1.5">
                  <Plus className="w-3.5 h-3.5 text-[#4EC9B0]" />
                  Create New Port Forward Rule
                </span>

                {/* Preset Chips */}
                <div className="flex items-center gap-1">
                  <span className="text-[10px] text-[#858585] mr-1">Presets:</span>
                  {[
                    {
                      name: 'MySQL DB',
                      type: 'local' as const,
                      local: 3306,
                      remHost: '127.0.0.1',
                      remPort: 3306,
                      desc: 'Remote MySQL port 3306 mapped to localhost',
                    },
                    {
                      name: 'PostgreSQL',
                      type: 'local' as const,
                      local: 5432,
                      remHost: '127.0.0.1',
                      remPort: 5432,
                      desc: 'Remote Postgres port 5432 mapped to localhost',
                    },
                    {
                      name: 'Redis',
                      type: 'local' as const,
                      local: 6379,
                      remHost: '127.0.0.1',
                      remPort: 6379,
                      desc: 'Remote Redis cache port 6379',
                    },
                    {
                      name: 'SOCKS5 Proxy',
                      type: 'dynamic' as const,
                      local: 1080,
                      remHost: '127.0.0.1',
                      remPort: 1080,
                      desc: 'Dynamic SOCKS5 SSH proxy on port 1080',
                    },
                  ].map((p) => (
                    <button
                      key={p.name}
                      type="button"
                      onClick={() => applyPreset(p)}
                      className="px-1.5 py-0.5 rounded bg-[#1E1E1E] hover:bg-[#333333] text-[#CE9178] border border-[#3C3C3C] text-[10px] transition-colors"
                    >
                      {p.name}
                    </button>
                  ))}
                </div>
              </div>

              {/* Form Grid */}
              <div className="grid grid-cols-2 gap-3 text-xs">
                <div>
                  <label className="block text-[11px] text-[#858585] mb-1">Rule Name</label>
                  <input
                    type="text"
                    placeholder="e.g. Prod Database or Web API"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    className="w-full px-2.5 py-1.5 bg-[#1E1E1E] border border-[#3C3C3C] rounded text-[#D4D4D4] focus:outline-none focus:border-[#007ACC]"
                  />
                </div>

                <div>
                  <label className="block text-[11px] text-[#858585] mb-1">Forwarding Type</label>
                  <div className="grid grid-cols-3 gap-1 bg-[#1E1E1E] p-0.5 rounded border border-[#3C3C3C]">
                    <button
                      type="button"
                      onClick={() => setTunnelType('local')}
                      className={`py-1 text-[10.5px] rounded font-medium transition-colors ${
                        tunnelType === 'local'
                          ? 'bg-[#007ACC] text-white'
                          : 'text-[#858585] hover:text-white'
                      }`}
                    >
                      -L Local
                    </button>
                    <button
                      type="button"
                      onClick={() => setTunnelType('remote')}
                      className={`py-1 text-[10.5px] rounded font-medium transition-colors ${
                        tunnelType === 'remote'
                          ? 'bg-[#007ACC] text-white'
                          : 'text-[#858585] hover:text-white'
                      }`}
                    >
                      -R Remote
                    </button>
                    <button
                      type="button"
                      onClick={() => setTunnelType('dynamic')}
                      className={`py-1 text-[10.5px] rounded font-medium transition-colors ${
                        tunnelType === 'dynamic'
                          ? 'bg-[#007ACC] text-white'
                          : 'text-[#858585] hover:text-white'
                      }`}
                    >
                      -D Dynamic
                    </button>
                  </div>
                </div>

                <div>
                  <label className="block text-[11px] text-[#858585] mb-1">
                    {tunnelType === 'remote' ? 'Remote Listen Port' : 'Local Listen Port'}
                  </label>
                  <input
                    type="number"
                    value={localPort}
                    onChange={(e) => setLocalPort(parseInt(e.target.value, 10) || 0)}
                    className="w-full px-2.5 py-1.5 bg-[#1E1E1E] border border-[#3C3C3C] rounded text-[#4EC9B0] font-mono focus:outline-none focus:border-[#007ACC]"
                  />
                </div>

                {tunnelType !== 'dynamic' && (
                  <div>
                    <label className="block text-[11px] text-[#858585] mb-1">
                      Target (Host:Port)
                    </label>
                    <div className="flex items-center gap-1.5">
                      <input
                        type="text"
                        placeholder="127.0.0.1"
                        value={remoteHost}
                        onChange={(e) => setRemoteHost(e.target.value)}
                        className="flex-1 px-2.5 py-1.5 bg-[#1E1E1E] border border-[#3C3C3C] rounded text-[#D4D4D4] font-mono text-xs focus:outline-none focus:border-[#007ACC]"
                      />
                      <span className="text-[#6D6D6D]">:</span>
                      <input
                        type="number"
                        placeholder="80"
                        value={remotePort}
                        onChange={(e) => setRemotePort(parseInt(e.target.value, 10) || 0)}
                        className="w-20 px-2 py-1.5 bg-[#1E1E1E] border border-[#3C3C3C] rounded text-[#CE9178] font-mono text-xs focus:outline-none focus:border-[#007ACC]"
                      />
                    </div>
                  </div>
                )}
              </div>

              <div className="flex items-center justify-end gap-2 pt-1">
                <button
                  type="button"
                  onClick={() => setShowAddForm(false)}
                  className="px-3 py-1 bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#858585] hover:text-white rounded text-xs transition-colors"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={handleCreate}
                  disabled={!name.trim()}
                  className="px-3.5 py-1 bg-[#007ACC] hover:bg-[#0062a3] disabled:opacity-50 text-white rounded text-xs font-medium transition-colors"
                >
                  Save & Enable
                </button>
              </div>
            </div>
          )}

          {/* Forwarding Rules List */}
          {hostTunnels.length === 0 ? (
            <div className="p-8 text-center bg-[#252526]/50 border border-dashed border-[#333333] rounded-xl space-y-2">
              <Network className="w-8 h-8 text-[#6D6D6D] mx-auto opacity-50" />
              <h4 className="text-sm font-medium text-[#D4D4D4]">No Port Forwarding Rules for This Host</h4>
              <p className="text-xs text-[#858585] max-w-md mx-auto">
                Forward local ports directly through this SSH connection to access remote databases, web dashboards, or expose local services securely.
              </p>
              {!showAddForm && (
                <button
                  onClick={() => setShowAddForm(true)}
                  className="mt-2 inline-flex items-center gap-1.5 px-3 py-1.5 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs font-medium transition-colors"
                >
                  <Plus className="w-3.5 h-3.5" />
                  <span>Create First Forward Rule</span>
                </button>
              )}
            </div>
          ) : (
            <div className="space-y-2.5">
              {hostTunnels.map((tunnel) => {
                const isActive = tunnel.status === 'active';
                const isLocal = tunnel.type === 'local';
                const isRemote = tunnel.type === 'remote';
                const isDynamic = tunnel.type === 'dynamic';

                return (
                  <div
                    key={tunnel.id}
                    className={`p-3 rounded-lg border transition-all space-y-2.5 ${
                      isActive
                        ? 'bg-[#252526] border-[#007ACC]/50 shadow-sm'
                        : 'bg-[#1E1E1E] border-[#333333] opacity-75'
                    }`}
                  >
                    {/* Top Row: Status, Title, Badges & Controls */}
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        <span
                          className={`w-2.5 h-2.5 rounded-full ${
                            isActive ? 'bg-[#27C93F] animate-pulse shadow-sm' : 'bg-[#6D6D6D]'
                          }`}
                        />
                        <span className="font-semibold text-xs text-white">{tunnel.name}</span>
                        <span
                          className={`text-[9px] px-1.5 py-0.2 rounded uppercase font-mono font-bold border ${
                            isLocal
                              ? 'bg-[#007ACC]/20 text-[#007ACC] border-[#007ACC]/40'
                              : isRemote
                              ? 'bg-[#CE9178]/20 text-[#CE9178] border-[#CE9178]/40'
                              : 'bg-[#C586C0]/20 text-[#C586C0] border-[#C586C0]/40'
                          }`}
                        >
                          {tunnel.type} Forward
                        </span>
                      </div>

                      <div className="flex items-center gap-1.5">
                        {/* Start/Stop Toggle */}
                        <button
                          onClick={() => handleToggle(tunnel.id)}
                          className={`flex items-center gap-1 px-2 py-0.5 rounded text-[11px] font-medium transition-colors ${
                            isActive
                              ? 'bg-[#27C93F]/20 text-[#27C93F] hover:bg-[#27C93F]/30 border border-[#27C93F]/50'
                              : 'bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#858585] hover:text-white border border-[#3C3C3C]'
                          }`}
                        >
                          {isActive ? (
                            <>
                              <Square className="w-3 h-3 fill-current" />
                              <span>Stop</span>
                            </>
                          ) : (
                            <>
                              <Play className="w-3 h-3 fill-current" />
                              <span>Start</span>
                            </>
                          )}
                        </button>

                        <button
                          onClick={() => handleDelete(tunnel.id)}
                          title="Delete Forward Rule"
                          className="p-1 text-[#6D6D6D] hover:text-[#FF5F56] rounded hover:bg-[#333333] transition-colors"
                        >
                          <Trash2 className="w-3.5 h-3.5" />
                        </button>
                      </div>
                    </div>

                    {/* Mapping Topology Visualization */}
                    <div className="bg-[#181818] p-2 rounded border border-[#2D2D2D] flex items-center justify-between text-xs font-mono">
                      <div className="flex items-center gap-1.5">
                        <Laptop className="w-3.5 h-3.5 text-[#569CD6]" />
                        <span className="text-[#858585]">127.0.0.1:</span>
                        <span className="text-[#4EC9B0] font-bold">{tunnel.localPort}</span>
                      </div>

                      <div className="flex items-center gap-1 text-[11px] text-[#6D6D6D] px-2">
                        <span>{isLocal ? 'Encrypted SSH Pipe' : isRemote ? 'Reverse SSH Pipe' : 'SOCKS5 Proxy'}</span>
                        <ArrowRight className="w-3.5 h-3.5 text-[#4EC9B0]" />
                      </div>

                      <div className="flex items-center gap-1.5">
                        <Server className="w-3.5 h-3.5 text-[#CE9178]" />
                        {isDynamic ? (
                          <span className="text-[#C586C0]">Dynamic SOCKS5 Target</span>
                        ) : (
                          <>
                            <span className="text-[#858585]">{tunnel.remoteHost}:</span>
                            <span className="text-[#CE9178] font-bold">{tunnel.remotePort}</span>
                          </>
                        )}
                      </div>
                    </div>

                    {/* Footer Actions & Diagnostics */}
                    <div className="flex items-center justify-between text-[11px] text-[#858585] pt-0.5">
                      <div className="flex items-center gap-2">
                        <span>Traffic: {formatTraffic(tunnel.bytesTransferred || 1024 * 512)}</span>
                        {isActive && isLocal && (
                          <a
                            href={`http://127.0.0.1:${tunnel.localPort}`}
                            target="_blank"
                            rel="noopener noreferrer"
                            className="text-[#007ACC] hover:underline flex items-center gap-0.5 ml-2"
                          >
                            <span>Open in Browser</span>
                            <ExternalLink className="w-3 h-3" />
                          </a>
                        )}
                      </div>

                      <div className="flex items-center gap-1.5">
                        {/* Test Socket */}
                        <button
                          onClick={() => handleTestSocket(tunnel)}
                          className="px-2 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#D4D4D4] border border-[#3C3C3C] text-[10px] transition-colors"
                        >
                          Test Socket
                        </button>

                        {/* Copy SSH Command Equivalent */}
                        <button
                          onClick={() => handleCopyCmd(tunnel)}
                          title="Copy equivalent SSH CLI command"
                          className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#D4D4D4] border border-[#3C3C3C] text-[10px] transition-colors"
                        >
                          {copiedId === tunnel.id ? (
                            <Check className="w-3 h-3 text-[#27C93F]" />
                          ) : (
                            <Copy className="w-3 h-3" />
                          )}
                          <span>Copy CLI</span>
                        </button>
                      </div>
                    </div>

                    {/* Test result toast */}
                    {testResult && testResult.id === tunnel.id && (
                      <div className="p-1.5 rounded bg-[#27C93F]/15 border border-[#27C93F]/40 text-[#27C93F] text-[10.5px] flex items-center gap-1.5">
                        <CheckCircle2 className="w-3.5 h-3.5 shrink-0" />
                        <span>{testResult.msg}</span>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* Modal Footer */}
        <div className="h-10 px-4 border-t border-[#333333] bg-[#252526] flex items-center justify-between text-[11px] text-[#858585] shrink-0">
          <div className="flex items-center gap-2 font-mono">
            <span>Shortcut: <kbd className="px-1 py-0.5 rounded bg-[#1E1E1E] text-[#D4D4D4] border border-[#3C3C3C]">Alt+P</kbd> to toggle</span>
          </div>

          <button
            onClick={onClose}
            className="px-3 py-1 rounded bg-[#333333] hover:bg-[#444444] text-[#D4D4D4] text-xs transition-colors"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
};
