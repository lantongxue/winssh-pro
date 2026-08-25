import React, { useState, useEffect, useRef } from 'react';
import {
  Activity,
  Cpu,
  HardDrive,
  Network,
  Clock,
  RefreshCw,
  Search,
  Trash2,
  Play,
  RotateCw,
  Square,
  AlertCircle,
  CheckCircle2,
  Server,
  Layers,
  Terminal,
  X,
  ChevronRight,
  Maximize2,
  Minimize2,
  Zap,
  ArrowUpDown,
  TrendingUp,
} from 'lucide-react';
import { ServerHost, ServerTelemetry, ProcessItem, DockerContainerItem } from '../types';
import { MOCK_TELEMETRY_MAP } from '../data/defaultData';

interface SessionRightMonitorPanelProps {
  host: ServerHost;
  isOpen: boolean;
  onClose: () => void;
  width: number;
  onResizeWidth: (w: number) => void;
  onSendTerminalCommand?: (cmd: string) => void;
}

export const SessionRightMonitorPanel: React.FC<SessionRightMonitorPanelProps> = ({
  host,
  isOpen,
  onClose,
  width,
  onResizeWidth,
  onSendTerminalCommand,
}) => {
  const [telemetry, setTelemetry] = useState<ServerTelemetry>(
    MOCK_TELEMETRY_MAP[host.id] || MOCK_TELEMETRY_MAP['host-k8s-prod'] || {
      hostId: host.id,
      cpu: 25.0,
      cpuHistory: [20, 25, 22, 28, 30, 24, 26, 29, 31, 25],
      memoryUsedGB: 8.4,
      memoryTotalGB: 32.0,
      memHistory: [26, 26, 27, 27, 26, 27, 26, 26, 27, 26],
      diskUsedGB: 120,
      diskTotalGB: 512,
      netInKBps: 150,
      netOutKBps: 420,
      uptime: '32 days, 10 hours',
      loadAvg: [0.75, 0.88, 1.02],
      processes: [],
      containers: [],
    }
  );

  const [activeTab, setActiveTab] = useState<'overview' | 'processes' | 'containers' | 'diagnostics'>('overview');
  const [processSearch, setProcessSearch] = useState('');
  const [processSortBy, setProcessSortBy] = useState<'cpu' | 'mem'>('cpu');
  const [isResizing, setIsResizing] = useState(false);
  const resizeRef = useRef<{ startX: number; startWidth: number } | null>(null);

  // Sync telemetry when host changes
  useEffect(() => {
    const base = MOCK_TELEMETRY_MAP[host.id] || MOCK_TELEMETRY_MAP['host-k8s-prod'];
    if (base) {
      setTelemetry(JSON.parse(JSON.stringify(base)));
    }
  }, [host.id]);

  // Live real-time jitter simulation
  useEffect(() => {
    if (!isOpen) return;
    const interval = setInterval(() => {
      setTelemetry((prev) => {
        const jitter = (Math.random() - 0.5) * 4.5;
        const newCpu = Math.max(4, Math.min(96, parseFloat((prev.cpu + jitter).toFixed(1))));
        const newCpuHist = [...prev.cpuHistory.slice(1), newCpu];

        return {
          ...prev,
          cpu: newCpu,
          cpuHistory: newCpuHist,
          netInKBps: Math.max(10, Math.floor(prev.netInKBps + (Math.random() - 0.5) * 60)),
          netOutKBps: Math.max(20, Math.floor(prev.netOutKBps + (Math.random() - 0.5) * 120)),
        };
      });
    }, 1800);

    return () => clearInterval(interval);
  }, [isOpen, host.id]);

  // Mouse resize drag handler
  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!isResizing || !resizeRef.current) return;
      const delta = resizeRef.current.startX - e.clientX;
      const newWidth = Math.max(300, Math.min(700, resizeRef.current.startWidth + delta));
      onResizeWidth(newWidth);
    };

    const handleMouseUp = () => {
      setIsResizing(false);
      resizeRef.current = null;
    };

    if (isResizing) {
      window.addEventListener('mousemove', handleMouseMove);
      window.addEventListener('mouseup', handleMouseUp);
    }
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isResizing, onResizeWidth]);

  if (!isOpen) return null;

  const filteredProcesses = (telemetry.processes || [])
    .filter(
      (p) =>
        p.command.toLowerCase().includes(processSearch.toLowerCase()) ||
        p.user.toLowerCase().includes(processSearch.toLowerCase()) ||
        p.pid.toString().includes(processSearch)
    )
    .sort((a, b) => (processSortBy === 'cpu' ? b.cpu - a.cpu : b.mem - a.mem));

  const handleKillProcess = (pid: number) => {
    setTelemetry((prev) => ({
      ...prev,
      processes: prev.processes.filter((p) => p.pid !== pid),
    }));
    if (onSendTerminalCommand) {
      onSendTerminalCommand(`kill -9 ${pid}`);
    }
  };

  const handleContainerAction = (containerId: string, action: 'restart' | 'stop') => {
    setTelemetry((prev) => ({
      ...prev,
      containers: prev.containers.map((c) =>
        c.id === containerId
          ? {
              ...c,
              status: action === 'restart' ? 'running' : 'exited',
            }
          : c
      ),
    }));
  };

  const memPercent = Math.round((telemetry.memoryUsedGB / telemetry.memoryTotalGB) * 100);
  const diskPercent = Math.round((telemetry.diskUsedGB / telemetry.diskTotalGB) * 100);

  return (
    <div
      style={{ width: `${width}px` }}
      className="h-full bg-[#181818] border-l border-[#333333] flex flex-col z-30 select-none shadow-2xl relative shrink-0 font-sans"
    >
      {/* Resizing Left Handle */}
      <div
        onMouseDown={(e) => {
          setIsResizing(true);
          resizeRef.current = { startX: e.clientX, startWidth: width };
        }}
        className="absolute left-0 top-0 bottom-0 w-1.5 cursor-ew-resize hover:bg-[#007ACC] transition-colors z-40"
        title="Drag to resize telemetry panel"
      />

      {/* Top Header Bar */}
      <div className="h-8 bg-[#252526] border-b border-[#333333] px-2.5 flex items-center justify-between shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <span
            className="w-2 h-2 rounded-full shrink-0 animate-pulse"
            style={{ backgroundColor: host.color || '#27C93F' }}
          />
          <Activity className="w-3.5 h-3.5 text-[#27C93F] shrink-0" />
          <span className="text-xs font-semibold text-white truncate font-mono">
            {host.name} <span className="text-[#858585] font-normal">({host.host})</span>
          </span>
        </div>

        <div className="flex items-center gap-1">
          <button
            onClick={() => {
              const base = MOCK_TELEMETRY_MAP[host.id] || MOCK_TELEMETRY_MAP['host-k8s-prod'];
              if (base) setTelemetry(JSON.parse(JSON.stringify(base)));
            }}
            title="Refresh Metrics"
            className="p-1 text-[#858585] hover:text-[#4EC9B0] hover:bg-[#3C3C3C] rounded transition-colors"
          >
            <RefreshCw className="w-3 h-3" />
          </button>
          <button
            onClick={onClose}
            title="Close Top Monitor Panel"
            className="p-1 text-[#858585] hover:text-white hover:bg-[#3C3C3C] rounded transition-colors"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Host Status Mini Banner */}
      <div className="px-3 py-1.5 bg-[#1E1E1E] border-b border-[#333333] flex items-center justify-between text-[11px] font-mono text-[#858585]">
        <div className="flex items-center gap-2">
          <span className="text-[#D4D4D4] font-medium">Uptime: {telemetry.uptime}</span>
        </div>
        <div className="flex items-center gap-1 text-[10px]">
          <span className="text-[#6D6D6D]">Load:</span>
          <span className="text-[#4EC9B0] font-semibold">{telemetry.loadAvg.join(' ')}</span>
        </div>
      </div>

      {/* Navigation Sub-Tabs */}
      <div className="flex items-center bg-[#252526] border-b border-[#333333] px-2 text-[11px]">
        {[
          { id: 'overview', label: 'Metrics', icon: Activity },
          { id: 'processes', label: `Procs (${telemetry.processes?.length || 0})`, icon: Cpu },
          { id: 'containers', label: `Docker (${telemetry.containers?.length || 0})`, icon: Layers },
          { id: 'diagnostics', label: 'Diag', icon: Zap },
        ].map((tab) => {
          const Icon = tab.icon;
          const isActive = activeTab === tab.id;
          return (
            <button
              key={tab.id}
              onClick={() => setActiveTab(tab.id as any)}
              className={`flex items-center gap-1.5 px-2.5 py-1.5 border-b-2 font-medium transition-colors ${
                isActive
                  ? 'border-[#007ACC] text-white bg-[#1E1E1E]'
                  : 'border-transparent text-[#858585] hover:text-[#D4D4D4] hover:bg-[#2A2D2E]'
              }`}
            >
              <Icon className={`w-3 h-3 ${isActive ? 'text-[#4EC9B0]' : 'text-[#858585]'}`} />
              <span>{tab.label}</span>
            </button>
          );
        })}
      </div>

      {/* Tab Body Content */}
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        {/* OVERVIEW METRICS TAB */}
        {activeTab === 'overview' && (
          <div className="space-y-3">
            {/* CPU & Memory Dual Gauges */}
            <div className="grid grid-cols-2 gap-2">
              {/* CPU Widget */}
              <div className="bg-[#1E1E1E] border border-[#333333] p-2.5 rounded">
                <div className="flex items-center justify-between text-[11px] text-[#858585]">
                  <div className="flex items-center gap-1 text-[#D4D4D4]">
                    <Cpu className="w-3.5 h-3.5 text-[#569CD6]" />
                    <span className="font-semibold">CPU Usage</span>
                  </div>
                  <span className="font-mono font-bold text-[#569CD6] text-xs">
                    {telemetry.cpu}%
                  </span>
                </div>
                {/* Mini sparkline */}
                <div className="flex items-end gap-0.5 h-6 pt-1 mt-1">
                  {telemetry.cpuHistory.map((val, idx) => (
                    <div
                      key={idx}
                      style={{ height: `${Math.max(10, val)}%` }}
                      className={`flex-1 rounded-t-xs transition-all duration-300 ${
                        val > 80 ? 'bg-[#FF5F56]' : val > 50 ? 'bg-[#FFBD2E]' : 'bg-[#569CD6]'
                      }`}
                    />
                  ))}
                </div>
              </div>

              {/* Memory Widget */}
              <div className="bg-[#1E1E1E] border border-[#333333] p-2.5 rounded">
                <div className="flex items-center justify-between text-[11px] text-[#858585]">
                  <div className="flex items-center gap-1 text-[#D4D4D4]">
                    <HardDrive className="w-3.5 h-3.5 text-[#4EC9B0]" />
                    <span className="font-semibold">RAM Usage</span>
                  </div>
                  <span className="font-mono font-bold text-[#4EC9B0] text-xs">
                    {memPercent}%
                  </span>
                </div>
                <div className="mt-2 space-y-1">
                  <div className="w-full h-1.5 bg-[#2D2D2D] rounded-full overflow-hidden">
                    <div
                      style={{ width: `${memPercent}%` }}
                      className="h-full bg-[#4EC9B0] rounded-full"
                    />
                  </div>
                  <div className="flex justify-between text-[9px] font-mono text-[#6D6D6D]">
                    <span>{telemetry.memoryUsedGB} GB</span>
                    <span>{telemetry.memoryTotalGB} GB</span>
                  </div>
                </div>
              </div>
            </div>

            {/* Storage & Network Dual */}
            <div className="grid grid-cols-2 gap-2">
              {/* Storage */}
              <div className="bg-[#1E1E1E] border border-[#333333] p-2.5 rounded">
                <div className="flex items-center justify-between text-[11px]">
                  <span className="text-[#D4D4D4] font-semibold flex items-center gap-1">
                    <HardDrive className="w-3.5 h-3.5 text-[#CE9178]" />
                    Disk / Root
                  </span>
                  <span className="font-mono text-xs text-[#CE9178] font-bold">{diskPercent}%</span>
                </div>
                <div className="mt-2 space-y-1">
                  <div className="w-full h-1.5 bg-[#2D2D2D] rounded-full overflow-hidden">
                    <div
                      style={{ width: `${diskPercent}%` }}
                      className="h-full bg-[#CE9178] rounded-full"
                    />
                  </div>
                  <div className="flex justify-between text-[9px] font-mono text-[#6D6D6D]">
                    <span>{telemetry.diskUsedGB} GB</span>
                    <span>{telemetry.diskTotalGB} GB</span>
                  </div>
                </div>
              </div>

              {/* Network */}
              <div className="bg-[#1E1E1E] border border-[#333333] p-2.5 rounded">
                <div className="flex items-center justify-between text-[11px]">
                  <span className="text-[#D4D4D4] font-semibold flex items-center gap-1">
                    <Network className="w-3.5 h-3.5 text-[#C586C0]" />
                    Network I/O
                  </span>
                </div>
                <div className="mt-1.5 space-y-0.5 text-[10px] font-mono">
                  <div className="flex justify-between text-[#D4D4D4]">
                    <span className="text-[#858585]">▲ Out:</span>
                    <span className="text-[#C586C0] font-bold">{telemetry.netOutKBps} KB/s</span>
                  </div>
                  <div className="flex justify-between text-[#D4D4D4]">
                    <span className="text-[#858585]">▼ In:</span>
                    <span className="text-[#4EC9B0] font-bold">{telemetry.netInKBps} KB/s</span>
                  </div>
                </div>
              </div>
            </div>

            {/* Quick Process Snapshot */}
            <div className="bg-[#1E1E1E] border border-[#333333] rounded overflow-hidden">
              <div className="p-2 bg-[#252526] border-b border-[#333333] flex items-center justify-between text-[11px]">
                <span className="font-semibold text-[#D4D4D4] flex items-center gap-1.5">
                  <TrendingUp className="w-3.5 h-3.5 text-[#569CD6]" />
                  Top High-Load Processes
                </span>
                <button
                  onClick={() => setActiveTab('processes')}
                  className="text-[10px] text-[#4EC9B0] hover:underline flex items-center gap-0.5"
                >
                  View All ({telemetry.processes?.length || 0})
                  <ChevronRight className="w-3 h-3" />
                </button>
              </div>

              <div className="divide-y divide-[#2D2D2D] text-[11px] font-mono">
                {telemetry.processes.slice(0, 4).map((p) => (
                  <div
                    key={p.pid}
                    className="p-2 flex items-center justify-between hover:bg-[#2A2D2E] transition-colors"
                  >
                    <div className="min-w-0 pr-2">
                      <div className="flex items-center gap-1.5">
                        <span className="text-[#858585] text-[10px]">[{p.pid}]</span>
                        <span className="text-[#D4D4D4] font-medium truncate max-w-[150px]">
                          {p.command.split(' ')[0]}
                        </span>
                      </div>
                      <span className="text-[10px] text-[#6D6D6D] truncate block">
                        {p.user} • state {p.state}
                      </span>
                    </div>

                    <div className="flex items-center gap-2 shrink-0 text-right">
                      <div>
                        <div className="text-[#569CD6] font-bold text-[10px]">{p.cpu}% CPU</div>
                        <div className="text-[#858585] text-[9px]">{p.mem}% RAM</div>
                      </div>
                      <button
                        onClick={() => handleKillProcess(p.pid)}
                        title={`Kill PID ${p.pid}`}
                        className="p-1 hover:bg-[#FF5F56]/20 text-[#858585] hover:text-[#FF5F56] rounded transition-colors"
                      >
                        <Trash2 className="w-3 h-3" />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}

        {/* PROCESSES FULL LIST TAB */}
        {activeTab === 'processes' && (
          <div className="space-y-2">
            {/* Search & Sort Controls */}
            <div className="flex items-center gap-2">
              <div className="flex-1 relative">
                <Search className="w-3 h-3 text-[#858585] absolute left-2 top-2" />
                <input
                  type="text"
                  placeholder="Filter PID, user, cmd..."
                  value={processSearch}
                  onChange={(e) => setProcessSearch(e.target.value)}
                  className="w-full bg-[#1E1E1E] border border-[#333333] rounded pl-7 pr-2 py-1 text-xs text-[#D4D4D4] placeholder-[#6D6D6D] focus:outline-none focus:border-[#007ACC] font-mono"
                />
              </div>

              <div className="flex items-center bg-[#1E1E1E] border border-[#333333] rounded p-0.5 text-[10px]">
                <button
                  onClick={() => setProcessSortBy('cpu')}
                  className={`px-2 py-0.5 rounded font-mono ${
                    processSortBy === 'cpu'
                      ? 'bg-[#007ACC] text-white font-bold'
                      : 'text-[#858585] hover:text-[#D4D4D4]'
                  }`}
                >
                  CPU
                </button>
                <button
                  onClick={() => setProcessSortBy('mem')}
                  className={`px-2 py-0.5 rounded font-mono ${
                    processSortBy === 'mem'
                      ? 'bg-[#007ACC] text-white font-bold'
                      : 'text-[#858585] hover:text-[#D4D4D4]'
                  }`}
                >
                  MEM
                </button>
              </div>
            </div>

            {/* Process List Table */}
            <div className="bg-[#1E1E1E] border border-[#333333] rounded overflow-hidden divide-y divide-[#2D2D2D]">
              {filteredProcesses.map((p) => (
                <div
                  key={p.pid}
                  className="p-2 flex items-center justify-between hover:bg-[#2A2D2E] text-[11px] font-mono transition-colors"
                >
                  <div className="min-w-0 flex-1 pr-2">
                    <div className="flex items-center gap-1.5">
                      <span className="text-[#569CD6] font-bold text-[10px]">{p.pid}</span>
                      <span className="text-[#858585] text-[10px]">({p.user})</span>
                      <span className="text-white font-medium truncate">{p.command}</span>
                    </div>
                  </div>

                  <div className="flex items-center gap-2 shrink-0">
                    <span className="text-[#569CD6] font-bold text-[10px]">{p.cpu}%</span>
                    <span className="text-[#858585] text-[10px]">{p.mem}%</span>
                    <button
                      onClick={() => handleKillProcess(p.pid)}
                      title="Kill Process (SIGKILL)"
                      className="p-1 hover:bg-[#FF5F56]/20 text-[#858585] hover:text-[#FF5F56] rounded transition-colors"
                    >
                      <Trash2 className="w-3 h-3" />
                    </button>
                  </div>
                </div>
              ))}
              {filteredProcesses.length === 0 && (
                <div className="p-4 text-center text-xs text-[#858585]">
                  No matching processes found
                </div>
              )}
            </div>
          </div>
        )}

        {/* DOCKER CONTAINERS TAB */}
        {activeTab === 'containers' && (
          <div className="space-y-2">
            {telemetry.containers && telemetry.containers.length > 0 ? (
              telemetry.containers.map((c) => (
                <div
                  key={c.id}
                  className="bg-[#1E1E1E] border border-[#333333] p-2.5 rounded space-y-1.5 text-xs font-mono"
                >
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-1.5 min-w-0">
                      <span
                        className={`w-2 h-2 rounded-full shrink-0 ${
                          c.status === 'running' ? 'bg-[#27C93F]' : 'bg-[#FF5F56]'
                        }`}
                      />
                      <span className="font-semibold text-white truncate">{c.name}</span>
                    </div>
                    <span
                      className={`text-[9px] px-1.5 py-0.2 rounded uppercase font-mono ${
                        c.status === 'running'
                          ? 'bg-[#27C93F]/20 text-[#27C93F] border border-[#27C93F]/40'
                          : 'bg-[#FF5F56]/20 text-[#FF5F56] border border-[#FF5F56]/40'
                      }`}
                    >
                      {c.status}
                    </span>
                  </div>

                  <div className="text-[10px] text-[#858585] truncate">
                    Image: <span className="text-[#D4D4D4]">{c.image}</span>
                  </div>

                  <div className="flex items-center justify-between text-[10px] text-[#6D6D6D] pt-1 border-t border-[#2D2D2D]">
                    <div className="flex items-center gap-2">
                      <span>CPU: <strong className="text-[#569CD6]">{c.cpu}%</strong></span>
                      <span>RAM: <strong className="text-[#4EC9B0]">{c.mem}</strong></span>
                    </div>

                    <div className="flex items-center gap-1">
                      <button
                        onClick={() => handleContainerAction(c.id, 'restart')}
                        title="Restart Container"
                        className="p-1 hover:bg-[#3C3C3C] text-[#858585] hover:text-[#4EC9B0] rounded transition-colors"
                      >
                        <RotateCw className="w-3 h-3" />
                      </button>
                      <button
                        onClick={() =>
                          handleContainerAction(c.id, c.status === 'running' ? 'stop' : 'restart')
                        }
                        title={c.status === 'running' ? 'Stop Container' : 'Start Container'}
                        className="p-1 hover:bg-[#3C3C3C] text-[#858585] hover:text-[#FF5F56] rounded transition-colors"
                      >
                        {c.status === 'running' ? (
                          <Square className="w-3 h-3" />
                        ) : (
                          <Play className="w-3 h-3" />
                        )}
                      </button>
                    </div>
                  </div>
                </div>
              ))
            ) : (
              <div className="bg-[#1E1E1E] border border-[#333333] p-4 rounded text-center text-xs text-[#858585]">
                No active Docker daemon or containers on this host
              </div>
            )}
          </div>
        )}

        {/* QUICK TERMINAL DIAGNOSTICS TAB */}
        {activeTab === 'diagnostics' && (
          <div className="space-y-2">
            <div className="text-xs text-[#858585] mb-1">
              Execute live diagnostic commands in active terminal session:
            </div>
            {[
              { label: 'top -b -n 1', desc: 'Snapshot top process memory & cpu', cmd: 'top -b -n 1 | head -n 25' },
              { label: 'free -m', desc: 'Display memory and swap distribution', cmd: 'free -m' },
              { label: 'df -h', desc: 'Check mounted disk space filesystem', cmd: 'df -h' },
              { label: 'netstat -tulnp', desc: 'Active listening network ports', cmd: 'netstat -tulnp' },
              { label: 'dmesg -T | tail -20', desc: 'Kernel buffer & hardware logs', cmd: 'dmesg -T | tail -n 20' },
              { label: 'journalctl -xe', desc: 'Systemd service failure logs', cmd: 'journalctl -xe --no-pager -n 20' },
              { label: 'docker stats --no-stream', desc: 'Docker live resource telemetry', cmd: 'docker stats --no-stream' },
            ].map((diag) => (
              <div
                key={diag.label}
                onClick={() => onSendTerminalCommand && onSendTerminalCommand(diag.cmd)}
                className="p-2 bg-[#1E1E1E] hover:bg-[#2A2D2E] border border-[#333333] hover:border-[#007ACC] rounded cursor-pointer transition-all flex items-center justify-between group"
              >
                <div className="min-w-0 pr-2">
                  <div className="font-mono text-xs text-[#4EC9B0] font-semibold group-hover:text-white transition-colors">
                    {diag.label}
                  </div>
                  <div className="text-[10px] text-[#858585] truncate">{diag.desc}</div>
                </div>
                <Terminal className="w-3.5 h-3.5 text-[#6D6D6D] group-hover:text-[#4EC9B0] shrink-0" />
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
};
