import React, { useState } from 'react';
import { Zap, X, Terminal, ArrowRight, History } from 'lucide-react';
import { ServerHost } from '../types';

interface QuickConnectModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConnect: (connectionString: string) => void;
  recentHosts: ServerHost[];
  onSelectRecent: (host: ServerHost) => void;
}

export const QuickConnectModal: React.FC<QuickConnectModalProps> = ({
  isOpen,
  onClose,
  onConnect,
  recentHosts,
  onSelectRecent,
}) => {
  const [connectStr, setConnectStr] = useState('');

  if (!isOpen) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!connectStr.trim()) return;
    onConnect(connectStr.trim());
    onClose();
  };

  return (
    <div className="fixed inset-0 bg-black/75 backdrop-blur-sm z-50 flex items-center justify-center p-4 select-none">
      <div className="bg-zinc-900 border border-zinc-750 rounded-2xl max-w-lg w-full p-5 shadow-2xl space-y-4 font-sans">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-zinc-800 pb-3">
          <div className="flex items-center gap-2.5">
            <div className="w-8 h-8 rounded-lg bg-cyan-500/10 border border-cyan-500/30 flex items-center justify-center text-cyan-400">
              <Zap className="w-4 h-4" />
            </div>
            <div>
              <h3 className="text-sm font-semibold text-zinc-100">
                SSH Quick Connect
              </h3>
              <p className="text-[11px] text-zinc-400">
                Connect instantly using standard OpenSSH URI syntax
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-zinc-400 hover:text-zinc-200 p-1 hover:bg-zinc-800 rounded-lg transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Input Form */}
        <form onSubmit={handleSubmit} className="space-y-3">
          <div className="space-y-1">
            <label className="text-xs text-zinc-300 font-medium">
              SSH Connection URI
            </label>
            <div className="relative font-mono">
              <input
                type="text"
                placeholder="ssh user@192.168.1.100 -p 22"
                value={connectStr}
                onChange={(e) => setConnectStr(e.target.value)}
                autoFocus
                className="w-full pl-3 pr-24 py-2.5 bg-zinc-950 border border-zinc-800 rounded-xl text-sm text-cyan-300 placeholder-zinc-600 focus:outline-none focus:border-cyan-500"
              />
              <button
                type="submit"
                className="absolute right-1.5 top-1.5 px-3 py-1.5 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg text-xs font-sans font-medium transition-colors flex items-center gap-1 shadow-sm"
              >
                <span>Connect</span>
                <ArrowRight className="w-3.5 h-3.5" />
              </button>
            </div>
            <p className="text-[11px] text-zinc-500">
              Syntax examples: <code className="text-zinc-400 font-mono">root@10.0.0.1</code> or <code className="text-zinc-400 font-mono">admin@server.com:2222</code>
            </p>
          </div>
        </form>

        {/* Recent Server Nodes */}
        {recentHosts.length > 0 && (
          <div className="space-y-2 pt-2 border-t border-zinc-800">
            <div className="flex items-center gap-1.5 text-xs text-zinc-400 font-medium">
              <History className="w-3.5 h-3.5 text-zinc-500" />
              <span>Recent Server Fleet</span>
            </div>

            <div className="space-y-1 max-h-48 overflow-y-auto pr-1">
              {recentHosts.map((h) => (
                <div
                  key={h.id}
                  onClick={() => {
                    onSelectRecent(h);
                    onClose();
                  }}
                  className="flex items-center justify-between p-2 rounded-lg bg-zinc-950/60 hover:bg-zinc-850 border border-zinc-850 hover:border-zinc-700 cursor-pointer transition-all group text-xs"
                >
                  <div className="flex items-center gap-2">
                    <span
                      className="w-2 h-2 rounded-full"
                      style={{ backgroundColor: h.color || '#38bdf8' }}
                    />
                    <span className="text-zinc-200 font-medium group-hover:text-cyan-300">
                      {h.name}
                    </span>
                    <span className="text-zinc-500 font-mono text-[11px]">
                      ({h.username}@{h.host}:{h.port})
                    </span>
                  </div>

                  <span className="text-[10px] text-cyan-400 group-hover:translate-x-0.5 transition-transform">
                    Connect →
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
