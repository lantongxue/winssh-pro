import React, { useState, useEffect } from 'react';
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
  FileCode,
  Terminal,
} from 'lucide-react';
import { ServerHost, ServerTelemetry, ProcessItem, DockerContainerItem } from '../types';
import { MOCK_TELEMETRY_MAP } from '../data/defaultData';

interface HostMonitorDashboardProps {
  hosts: ServerHost[];
  selectedHostId: string;
  onSelectHost: (hostId: string) => void;
  onOpenTerminalTab: (hostId: string) => void;
}

export const HostMonitorDashboard: React.FC<HostMonitorDashboardProps> = ({
  hosts,
  selectedHostId,
  onSelectHost,
  onOpenTerminalTab,
}) => {
  const currentHost = hosts.find((h) => h.id === selectedHostId) || hosts[0];
  const [telemetry, setTelemetry] = useState<ServerTelemetry>(
    MOCK_TELEMETRY_MAP[currentHost?.id] || MOCK_TELEMETRY_MAP['host-k8s-prod']
  );

  const [processSearch, setProcessSearch] = useState('');
  const [processSortBy, setProcessSortBy] = useState<'cpu' | 'mem'>('cpu');
  const [selectedContainerLogs, setSelectedContainerLogs] = useState<string | null>(null);

  // Live simulation ticks
  useEffect(() => {
    const base = MOCK_TELEMETRY_MAP[currentHost?.id] || MOCK_TELEMETRY_MAP['host-k8s-prod'];
    setTelemetry(base);

    const interval = setInterval(() => {
      setTelemetry((prev) => {
        const jitter = (Math.random() - 0.5) * 4;
        const newCpu = Math.max(5, Math.min(95, parseFloat((prev.cpu + jitter).toFixed(1))));
        const newCpuHist = [...prev.cpuHistory.slice(1), newCpu];

        return {
          ...prev,
          cpu: newCpu,
          cpuHistory: newCpuHist,
          netInKBps: Math.floor(prev.netInKBps + (Math.random() - 0.5) * 50),
          netOutKBps: Math.floor(prev.netOutKBps + (Math.random() - 0.5) * 100),
        };
      });
    }, 2000);

    return () => clearInterval(interval);
  }, [currentHost?.id]);

  const filteredProcesses = telemetry.processes
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

  return (
    <div className="flex-1 flex flex-col bg-zinc-950 overflow-y-auto p-4 space-y-4 font-sans select-none">
      {/* Header with Server Switcher and Uptime */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 bg-zinc-900 border border-zinc-800 p-3 rounded-xl">
        <div className="flex items-center gap-3">
          <div className="w-9 h-9 rounded-lg bg-emerald-500/10 border border-emerald-500/30 flex items-center justify-center text-emerald-400">
            <Activity className="w-5 h-5" />
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h3 className="text-sm font-semibold text-zinc-100">
                Live Server Telemetry & Health
              </h3>
              <span className="w-2 h-2 rounded-full bg-emerald-400 animate-pulse" />
            </div>
            <p className="text-xs text-zinc-400 font-mono">
              Uptime: {telemetry.uptime} | Load Avg: {telemetry.loadAvg.join(', ')}
            </p>
          </div>
        </div>

        {/* Server Selector */}
        <div className="flex items-center gap-2">
          <select
            value={currentHost?.id}
            onChange={(e) => onSelectHost(e.target.value)}
            className="px-3 py-1.5 bg-zinc-950 border border-zinc-750 text-xs text-zinc-200 rounded-lg focus:outline-none focus:border-cyan-500 font-mono"
          >
            {hosts.map((h) => (
              <option key={h.id} value={h.id}>
                {h.name} ({h.host})
              </option>
            ))}
          </select>

          <button
            onClick={() => onOpenTerminalTab(currentHost.id)}
            className="flex items-center gap-1.5 px-3 py-1.5 bg-cyan-600/20 hover:bg-cyan-500/30 text-cyan-300 border border-cyan-500/40 rounded-lg text-xs font-medium transition-colors"
          >
            <Terminal className="w-3.5 h-3.5 text-cyan-400" />
            <span>Open Terminal</span>
          </button>
        </div>
      </div>

      {/* 4 Core Metrics Grid */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
        {/* 1. CPU Card */}
        <div className="bg-zinc-900/80 border border-zinc-800 p-3.5 rounded-xl space-y-2">
          <div className="flex items-center justify-between text-xs text-zinc-400">
            <div className="flex items-center gap-1.5">
              <Cpu className="w-4 h-4 text-cyan-400" />
              <span className="font-medium">CPU Utilization</span>
            </div>
            <span className="font-mono text-cyan-400 font-bold text-sm">
              {telemetry.cpu}%
            </span>
          </div>
          {/* Sparkline */}
          <div className="flex items-end gap-1 h-8 pt-1">
            {telemetry.cpuHistory.map((val, idx) => (
              <div
                key={idx}
                className="flex-1 bg-cyan-500/30 hover:bg-cyan-400 rounded-t transition-all"
                style={{ height: `${Math.max(10, val)}%` }}
                title={`${val}%`}
              />
            ))}
          </div>
          <div className="w-full bg-zinc-800 rounded-full h-1.5 overflow-hidden">
            <div
              className={`h-full transition-all duration-300 ${
                telemetry.cpu > 80 ? 'bg-red-500' : 'bg-cyan-500'
              }`}
              style={{ width: `${telemetry.cpu}%` }}
            />
          </div>
        </div>

        {/* 2. Memory RAM Card */}
        <div className="bg-zinc-900/80 border border-zinc-800 p-3.5 rounded-xl space-y-2">
          <div className="flex items-center justify-between text-xs text-zinc-400">
            <div className="flex items-center gap-1.5">
              <Layers className="w-4 h-4 text-emerald-400" />
              <span className="font-medium">RAM Memory</span>
            </div>
            <span className="font-mono text-emerald-400 font-bold text-xs">
              {telemetry.memoryUsedGB} / {telemetry.memoryTotalGB} GB
            </span>
          </div>
          <div className="flex items-end gap-1 h-8 pt-1">
            {telemetry.memHistory.map((val, idx) => (
              <div
                key={idx}
                className="flex-1 bg-emerald-500/30 hover:bg-emerald-400 rounded-t transition-all"
                style={{ height: `${val}%` }}
                title={`${val}%`}
              />
            ))}
          </div>
          <div className="w-full bg-zinc-800 rounded-full h-1.5 overflow-hidden">
            <div
              className="h-full bg-emerald-500 transition-all duration-300"
              style={{
                width: `${(telemetry.memoryUsedGB / telemetry.memoryTotalGB) * 100}%`,
              }}
            />
          </div>
        </div>

        {/* 3. Disk NVMe Storage */}
        <div className="bg-zinc-900/80 border border-zinc-800 p-3.5 rounded-xl space-y-2">
          <div className="flex items-center justify-between text-xs text-zinc-400">
            <div className="flex items-center gap-1.5">
              <HardDrive className="w-4 h-4 text-amber-400" />
              <span className="font-medium">NVMe Disk /</span>
            </div>
            <span className="font-mono text-amber-400 font-bold text-xs">
              {telemetry.diskUsedGB} / {telemetry.diskTotalGB} GB
            </span>
          </div>
          <div className="h-8 flex flex-col justify-center">
            <div className="flex justify-between text-[11px] font-mono text-zinc-500 mb-1">
              <span>Used: {Math.round((telemetry.diskUsedGB / telemetry.diskTotalGB) * 100)}%</span>
              <span>Free: {telemetry.diskTotalGB - telemetry.diskUsedGB} GB</span>
            </div>
            <div className="w-full bg-zinc-800 rounded-full h-2 overflow-hidden">
              <div
                className="h-full bg-amber-500"
                style={{
                  width: `${(telemetry.diskUsedGB / telemetry.diskTotalGB) * 100}%`,
                }}
              />
            </div>
          </div>
        </div>

        {/* 4. Network Traffic */}
        <div className="bg-zinc-900/80 border border-zinc-800 p-3.5 rounded-xl space-y-2">
          <div className="flex items-center justify-between text-xs text-zinc-400">
            <div className="flex items-center gap-1.5">
              <Network className="w-4 h-4 text-indigo-400" />
              <span className="font-medium">Network I/O</span>
            </div>
            <span className="font-mono text-indigo-400 font-bold text-xs">Active</span>
          </div>
          <div className="h-8 flex items-center justify-between font-mono text-xs text-zinc-300">
            <div>
              <span className="text-zinc-500 text-[10px] block">IN (RX):</span>
              <span className="text-cyan-300 font-semibold">{telemetry.netInKBps} KB/s</span>
            </div>
            <div className="text-right">
              <span className="text-zinc-500 text-[10px] block">OUT (TX):</span>
              <span className="text-indigo-300 font-semibold">{telemetry.netOutKBps} KB/s</span>
            </div>
          </div>
        </div>
      </div>

      {/* Two Column Layout: Processes & Docker Containers */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 flex-1">
        {/* Left: Process Task Manager */}
        <div className="bg-zinc-900 border border-zinc-800 rounded-xl flex flex-col overflow-hidden">
          <div className="p-3 border-b border-zinc-800 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Activity className="w-4 h-4 text-cyan-400" />
              <span className="text-xs font-semibold text-zinc-200">
                Process Task Manager ({telemetry.processes.length})
              </span>
            </div>

            <div className="flex items-center gap-2">
              <div className="relative">
                <Search className="w-3 h-3 text-zinc-500 absolute left-2 top-2" />
                <input
                  type="text"
                  placeholder="Filter PID, user..."
                  value={processSearch}
                  onChange={(e) => setProcessSearch(e.target.value)}
                  className="pl-6 pr-2 py-0.5 text-xs bg-zinc-950 border border-zinc-800 rounded text-zinc-200 w-32 focus:outline-none"
                />
              </div>

              <div className="flex items-center bg-zinc-950 border border-zinc-800 rounded p-0.5 text-[10px]">
                <button
                  onClick={() => setProcessSortBy('cpu')}
                  className={`px-1.5 py-0.5 rounded ${
                    processSortBy === 'cpu' ? 'bg-zinc-800 text-cyan-400' : 'text-zinc-400'
                  }`}
                >
                  CPU%
                </button>
                <button
                  onClick={() => setProcessSortBy('mem')}
                  className={`px-1.5 py-0.5 rounded ${
                    processSortBy === 'mem' ? 'bg-zinc-800 text-cyan-400' : 'text-zinc-400'
                  }`}
                >
                  MEM%
                </button>
              </div>
            </div>
          </div>

          <div className="flex-1 overflow-y-auto p-2 max-h-80">
            <table className="w-full text-xs text-left text-zinc-300 font-mono">
              <thead className="text-[10px] text-zinc-500 uppercase border-b border-zinc-850">
                <tr>
                  <th className="pb-1.5">PID</th>
                  <th className="pb-1.5">User</th>
                  <th className="pb-1.5">CPU%</th>
                  <th className="pb-1.5">MEM%</th>
                  <th className="pb-1.5">Command</th>
                  <th className="pb-1.5 text-right">Action</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-900 text-[11px]">
                {filteredProcesses.map((proc) => (
                  <tr key={proc.pid} className="hover:bg-zinc-850/60 transition-colors">
                    <td className="py-1.5 text-cyan-400 font-semibold">{proc.pid}</td>
                    <td className="py-1.5 text-zinc-400">{proc.user}</td>
                    <td className="py-1.5 font-bold text-zinc-200">{proc.cpu}%</td>
                    <td className="py-1.5 text-zinc-400">{proc.mem}%</td>
                    <td className="py-1.5 text-zinc-300 truncate max-w-[160px]" title={proc.command}>
                      {proc.command}
                    </td>
                    <td className="py-1.5 text-right">
                      <button
                        onClick={() => handleKillProcess(proc.pid)}
                        title="SIGTERM Kill Process"
                        className="p-1 text-zinc-500 hover:text-red-400 rounded hover:bg-zinc-800 transition-colors"
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

        {/* Right: Docker Containers Dashboard */}
        <div className="bg-zinc-900 border border-zinc-800 rounded-xl flex flex-col overflow-hidden">
          <div className="p-3 border-b border-zinc-800 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Layers className="w-4 h-4 text-blue-400" />
              <span className="text-xs font-semibold text-zinc-200">
                Docker Containers ({telemetry.containers.length})
              </span>
            </div>
            <span className="text-[10px] text-zinc-400 font-mono">Engine v26.1.4</span>
          </div>

          <div className="flex-1 overflow-y-auto p-3 space-y-2 max-h-80">
            {telemetry.containers.length === 0 ? (
              <div className="text-center py-8 text-zinc-500 text-xs">
                No active Docker containers running on this node.
              </div>
            ) : (
              telemetry.containers.map((container) => (
                <div
                  key={container.id}
                  className="p-2.5 bg-zinc-950/80 border border-zinc-800 rounded-lg space-y-1.5"
                >
                  <div className="flex items-center justify-between text-xs">
                    <div className="flex items-center gap-2">
                      <span
                        className={`w-2 h-2 rounded-full ${
                          container.status === 'running' ? 'bg-emerald-400 animate-pulse' : 'bg-zinc-600'
                        }`}
                      />
                      <span className="font-semibold text-zinc-100 font-mono">
                        {container.name}
                      </span>
                    </div>

                    <div className="flex items-center gap-1">
                      <button
                        onClick={() =>
                          setSelectedContainerLogs(
                            `[${container.name}] 2025-02-25 00:30:14 INFO Initializing worker pool...\n[${container.name}] 2025-02-25 00:30:15 INFO Listening on ${container.ports}\n[${container.name}] 2025-02-25 00:35:00 INFO Status 200 OK - Healthy`
                          )
                        }
                        className="px-2 py-0.5 bg-zinc-850 hover:bg-zinc-800 text-zinc-300 rounded text-[10px] font-mono"
                      >
                        Logs
                      </button>
                      <button
                        onClick={() => handleContainerAction(container.id, 'restart')}
                        title="Restart Container"
                        className="p-1 hover:bg-zinc-800 text-zinc-400 hover:text-cyan-300 rounded"
                      >
                        <RotateCw className="w-3 h-3" />
                      </button>
                      <button
                        onClick={() => handleContainerAction(container.id, 'stop')}
                        title="Stop Container"
                        className="p-1 hover:bg-zinc-800 text-zinc-400 hover:text-red-400 rounded"
                      >
                        <Square className="w-3 h-3" />
                      </button>
                    </div>
                  </div>

                  <div className="flex items-center justify-between text-[11px] text-zinc-400 font-mono">
                    <span className="truncate max-w-[180px] text-zinc-500">
                      {container.image}
                    </span>
                    <span>
                      CPU: <b className="text-zinc-300">{container.cpu}%</b> | Mem: <b className="text-zinc-300">{container.mem}</b>
                    </span>
                  </div>
                </div>
              ))
            )}
          </div>
        </div>
      </div>

      {/* Container Logs Viewer Modal */}
      {selectedContainerLogs && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
          <div className="bg-zinc-900 border border-zinc-750 rounded-xl max-w-xl w-full p-4 space-y-3 shadow-2xl">
            <div className="flex items-center justify-between border-b border-zinc-800 pb-2">
              <span className="text-xs font-semibold text-zinc-200 flex items-center gap-2">
                <FileCode className="w-4 h-4 text-cyan-400" />
                <span>Container Live Logs Stream</span>
              </span>
              <button
                onClick={() => setSelectedContainerLogs(null)}
                className="text-zinc-400 hover:text-zinc-200"
              >
                ✕
              </button>
            </div>
            <pre className="p-3 bg-zinc-950 rounded-lg text-xs font-mono text-cyan-300 whitespace-pre-wrap max-h-64 overflow-y-auto">
              {selectedContainerLogs}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
};
