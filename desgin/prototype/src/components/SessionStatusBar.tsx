import React, { useState, useEffect } from 'react';
import {
  Activity,
  Cpu,
  HardDrive,
  Network,
  Wifi,
  Folder,
  Layers,
  ChevronUp,
  Terminal,
  Radio,
  Clock,
  Shield,
  Zap,
  ArrowDown,
  ArrowUp,
  RotateCw,
  Server,
  Code2,
} from 'lucide-react';
import { ServerHost, TerminalPane, SessionTab, ServerTelemetry } from '../types';
import { MOCK_TELEMETRY_MAP } from '../data/defaultData';

interface SessionStatusBarProps {
  host: ServerHost;
  activePane: TerminalPane;
  activeTab: SessionTab;
  isMonitorOpen: boolean;
  isSftpOpen: boolean;
  activeTunnelsCount: number;
  onToggleMonitor: () => void;
  onToggleSftp: () => void;
  onToggleTunnels: () => void;
  onSendQuickCommand?: (cmd: string) => void;
}

export const SessionStatusBar: React.FC<SessionStatusBarProps> = ({
  host,
  activePane,
  activeTab,
  isMonitorOpen,
  isSftpOpen,
  activeTunnelsCount,
  onToggleMonitor,
  onToggleSftp,
  onToggleTunnels,
  onSendQuickCommand,
}) => {
  // Base telemetry from mock map or fallback
  const baseTelemetry = MOCK_TELEMETRY_MAP[host.id] || MOCK_TELEMETRY_MAP['host-k8s-prod'] || {
    hostId: host.id,
    cpu: 24.5,
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
  };

  const [telemetry, setTelemetry] = useState<ServerTelemetry>(baseTelemetry);
  const [pingMs, setPingMs] = useState<number>(host.pingMs || 18);
  const [isPinging, setIsPinging] = useState(false);
  const [showQuickMenu, setShowQuickMenu] = useState(false);

  // Sync when host changes
  useEffect(() => {
    const next = MOCK_TELEMETRY_MAP[host.id] || MOCK_TELEMETRY_MAP['host-k8s-prod'];
    if (next) {
      setTelemetry(JSON.parse(JSON.stringify(next)));
    }
    setPingMs(host.pingMs || 18);
  }, [host.id, host.pingMs]);

  // Real-time live jitter simulation (CPU, Net, Ping)
  useEffect(() => {
    const interval = setInterval(() => {
      // Jitter CPU
      setTelemetry((prev) => {
        const cpuJitter = (Math.random() - 0.5) * 3.2;
        const nextCpu = Math.max(4.0, Math.min(95.0, parseFloat((prev.cpu + cpuJitter).toFixed(1))));
        const nextLoad0 = Math.max(0.1, parseFloat((prev.loadAvg[0] + (Math.random() - 0.5) * 0.08).toFixed(2)));

        return {
          ...prev,
          cpu: nextCpu,
          loadAvg: [nextLoad0, prev.loadAvg[1], prev.loadAvg[2]],
          netInKBps: Math.max(12, Math.floor(prev.netInKBps + (Math.random() - 0.5) * 40)),
          netOutKBps: Math.max(25, Math.floor(prev.netOutKBps + (Math.random() - 0.5) * 80)),
        };
      });

      // Jitter Ping slightly +/- 2ms
      setPingMs((prev) => {
        const base = host.pingMs || 18;
        const delta = Math.floor((Math.random() - 0.5) * 4);
        return Math.max(4, base + delta);
      });
    }, 2400);

    return () => clearInterval(interval);
  }, [host.id, host.pingMs]);

  const handleManualPing = (e: React.MouseEvent) => {
    e.stopPropagation();
    setIsPinging(true);
    setTimeout(() => {
      const freshPing = Math.max(6, Math.floor((host.pingMs || 18) + (Math.random() - 0.5) * 6));
      setPingMs(freshPing);
      setIsPinging(false);
    }, 450);
  };

  // Latency status color
  const getPingColor = (ms: number) => {
    if (ms < 30) return 'text-[#27C93F]';
    if (ms < 80) return 'text-[#4EC9B0]';
    if (ms < 150) return 'text-[#CE9178]';
    return 'text-[#FF5F56]';
  };

  // CPU status color
  const getCpuColor = (cpuVal: number) => {
    if (cpuVal < 50) return 'text-[#4EC9B0]';
    if (cpuVal < 80) return 'text-[#DCDCAA]';
    return 'text-[#FF5F56]';
  };

  const memPercent = Math.round((telemetry.memoryUsedGB / telemetry.memoryTotalGB) * 100);

  return (
    <div className="h-6 bg-[#181818] border-t border-[#2D2D2D] px-2 flex items-center justify-between text-[11px] font-mono select-none shrink-0 text-[#858585] z-10 relative">
      {/* LEFT SECTION: Host Connection Status, Protocol & Host Latency */}
      <div className="flex items-center gap-3 overflow-hidden">
        {/* Host Live Status & SSH Badge */}
        <div className="flex items-center gap-1.5 shrink-0">
          <span className="w-2 h-2 rounded-full bg-[#27C93F] animate-pulse" />
          <span className="text-[#D4D4D4] font-semibold flex items-center gap-1">
            <Server className="w-3 h-3 text-[#569CD6]" />
            <span className="truncate max-w-[130px]">{host.name}</span>
          </span>
          <span className="text-[9px] px-1 py-0.2 rounded bg-[#252526] text-[#858585] border border-[#333333]">
            SSH-2.0
          </span>
        </div>

        {/* Host Ping / Latency Metric */}
        <button
          onClick={handleManualPing}
          title={`Click to re-ping ${host.host}:${host.port} (RTT)`}
          className="flex items-center gap-1 px-1.5 py-0.5 rounded hover:bg-[#252526] transition-colors group"
        >
          <Activity
            className={`w-3 h-3 ${getPingColor(pingMs)} ${isPinging ? 'animate-spin' : ''}`}
          />
          <span className="text-[#6D6D6D]">RTT:</span>
          <span className={`font-bold ${getPingColor(pingMs)}`}>{pingMs}ms</span>
        </button>

        {/* Host User & Port */}
        <div className="hidden md:flex items-center gap-1 text-[#6D6D6D]">
          <span>{host.username}@{host.host}:{host.port}</span>
        </div>
      </div>

      {/* CENTER / MIDDLE SECTION: Real-time CPU Load, Load Average, Memory & Network */}
      <div className="flex items-center gap-3">
        {/* CPU Load Metric (Clickable to toggle Top Monitor) */}
        <button
          onClick={onToggleMonitor}
          title={`CPU Load: ${telemetry.cpu}% (Load Avg: ${telemetry.loadAvg.join(', ')}) - Click to toggle Top Monitor (Alt+M)`}
          className={`flex items-center gap-1.5 px-1.5 py-0.5 rounded transition-colors ${
            isMonitorOpen
              ? 'bg-[#27C93F]/20 text-[#27C93F] border border-[#27C93F]/40'
              : 'hover:bg-[#252526] text-[#CCCCCC]'
          }`}
        >
          <Cpu className={`w-3 h-3 ${getCpuColor(telemetry.cpu)}`} />
          <span className="text-[#6D6D6D]">CPU:</span>
          <span className={`font-bold ${getCpuColor(telemetry.cpu)}`}>{telemetry.cpu}%</span>
          {/* Mini CPU Bar */}
          <div className="w-8 h-1.5 bg-[#252526] rounded-full overflow-hidden hidden sm:block border border-[#333333]">
            <div
              className={`h-full transition-all duration-500 ${
                telemetry.cpu > 80 ? 'bg-[#FF5F56]' : telemetry.cpu > 50 ? 'bg-[#DCDCAA]' : 'bg-[#4EC9B0]'
              }`}
              style={{ width: `${Math.min(100, telemetry.cpu)}%` }}
            />
          </div>
        </button>

        {/* Load Average */}
        <div
          title="System Load Average (1m, 5m, 15m)"
          className="hidden lg:flex items-center gap-1 text-[#858585]"
        >
          <span className="text-[#6D6D6D]">Load:</span>
          <span className="text-[#D4D4D4] font-medium">
            {telemetry.loadAvg[0]}, {telemetry.loadAvg[1]}, {telemetry.loadAvg[2]}
          </span>
        </div>

        {/* Memory Metric */}
        <div
          title={`Memory: ${telemetry.memoryUsedGB} GB / ${telemetry.memoryTotalGB} GB (${memPercent}%)`}
          className="hidden sm:flex items-center gap-1.5 text-[#858585]"
        >
          <Layers className="w-3 h-3 text-[#C586C0]" />
          <span className="text-[#6D6D6D]">Mem:</span>
          <span className="text-[#D4D4D4]">{memPercent}%</span>
          <span className="text-[#6D6D6D] text-[10px] hidden xl:inline">
            ({telemetry.memoryUsedGB}G/{telemetry.memoryTotalGB}G)
          </span>
        </div>

        {/* Network I/O Throughput */}
        <div
          title="Network Inbound / Outbound Transfer Rate"
          className="hidden xl:flex items-center gap-2 text-[#6D6D6D] text-[10px]"
        >
          <span className="flex items-center gap-0.5 text-[#4EC9B0]">
            <ArrowDown className="w-2.5 h-2.5" />
            {telemetry.netInKBps} KB/s
          </span>
          <span className="flex items-center gap-0.5 text-[#569CD6]">
            <ArrowUp className="w-2.5 h-2.5" />
            {telemetry.netOutKBps} KB/s
          </span>
        </div>
      </div>

      {/* RIGHT SECTION: Working Directory, Tunnels, Encoding & Quick Menu */}
      <div className="flex items-center gap-2 shrink-0">
        {/* Active CWD badge (Clickable to toggle SFTP) */}
        <button
          onClick={onToggleSftp}
          title={`Current Directory: ${activePane.cwd} - Click to toggle SFTP panel (Alt+F)`}
          className={`flex items-center gap-1 px-1.5 py-0.5 rounded transition-colors truncate max-w-[140px] ${
            isSftpOpen
              ? 'bg-[#007ACC]/20 text-[#4EC9B0] border border-[#007ACC]/50'
              : 'hover:bg-[#252526] text-[#858585] hover:text-[#D4D4D4]'
          }`}
        >
          <Folder className="w-3 h-3 text-[#569CD6] shrink-0" />
          <span className="truncate">{activePane.cwd}</span>
        </button>

        {/* Active Tunnels Badge */}
        {activeTunnelsCount > 0 && (
          <button
            onClick={onToggleTunnels}
            title={`${activeTunnelsCount} Active Port Forwarding Tunnels (Alt+P)`}
            className="flex items-center gap-1 px-1.5 py-0.5 rounded bg-[#4EC9B0]/15 text-[#4EC9B0] border border-[#4EC9B0]/30 hover:bg-[#4EC9B0]/25 transition-colors"
          >
            <Radio className="w-2.5 h-2.5 animate-pulse" />
            <span>{activeTunnelsCount} Tunnels</span>
          </button>
        )}

        {/* Terminal Specs */}
        <div className="hidden md:flex items-center gap-1.5 text-[10px] text-[#6D6D6D]">
          <span>UTF-8</span>
          <span>•</span>
          <span>LF</span>
        </div>

        {/* Quick Commands Popover Button */}
        {onSendQuickCommand && (
          <div className="relative">
            <button
              onClick={() => setShowQuickMenu((prev) => !prev)}
              title="Quick DevOps command shortcuts"
              className={`flex items-center gap-0.5 px-1.5 py-0.5 rounded text-[10px] transition-colors border ${
                showQuickMenu
                  ? 'bg-[#007ACC] text-white border-[#007ACC]'
                  : 'bg-[#252526] hover:bg-[#333333] text-[#A0A0A0] hover:text-white border-[#333333]'
              }`}
            >
              <Code2 className="w-2.5 h-2.5" />
              <span>Cmds</span>
              <ChevronUp
                className={`w-2.5 h-2.5 transition-transform ${showQuickMenu ? 'rotate-180' : ''}`}
              />
            </button>

            {/* Quick Command Floating Menu */}
            {showQuickMenu && (
              <>
                <div
                  className="fixed inset-0 z-40"
                  onClick={() => setShowQuickMenu(false)}
                />
                <div className="absolute right-0 bottom-full mb-1 w-48 bg-[#1E1E1E] border border-[#3C3C3C] rounded-lg shadow-xl py-1 z-50 text-[11px] animate-in fade-in slide-in-from-bottom-1 duration-100">
                  <div className="px-2.5 py-1 text-[10px] text-[#6D6D6D] uppercase font-bold border-b border-[#2D2D2D]">
                    Quick Run in Terminal
                  </div>
                  {[
                    { label: 'ls -la', cmd: 'ls -la', desc: 'List files detailed' },
                    { label: 'top monitor', cmd: 'top', desc: 'Toggle Top Telemetry' },
                    { label: 'neofetch', cmd: 'neofetch', desc: 'System info summary' },
                    { label: 'docker ps', cmd: 'docker ps', desc: 'Running containers' },
                    { label: 'kubectl pods', cmd: 'kubectl get pods', desc: 'K8s active pods' },
                    { label: 'df -h', cmd: 'df -h', desc: 'Disk space usage' },
                    { label: 'free -h', cmd: 'free -h', desc: 'RAM usage overview' },
                    { label: 'clear', cmd: 'clear', desc: 'Clear screen' },
                  ].map((item) => (
                    <button
                      key={item.label}
                      onClick={() => {
                        if (item.cmd === 'top') {
                          onToggleMonitor();
                        } else {
                          onSendQuickCommand(item.cmd);
                        }
                        setShowQuickMenu(false);
                      }}
                      className="w-full px-2.5 py-1.5 hover:bg-[#2A2D2E] text-left flex items-center justify-between text-[#CCCCCC] hover:text-[#4EC9B0] transition-colors"
                    >
                      <span className="font-mono">{item.label}</span>
                      <span className="text-[9px] text-[#6D6D6D]">{item.desc}</span>
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
