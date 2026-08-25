import React, { useState } from 'react';
import {
  Code2,
  Plus,
  Play,
  Copy,
  Check,
  Search,
  Tag,
  Trash2,
  Edit2,
  Terminal,
  Layers,
  Sparkles,
} from 'lucide-react';
import { DevOpsSnippet, ServerHost } from '../types';

interface SnippetVaultViewProps {
  snippets: DevOpsSnippet[];
  onUpdateSnippets: (snippets: DevOpsSnippet[]) => void;
  onExecuteSnippet: (command: string) => void;
}

export const SnippetVaultView: React.FC<SnippetVaultViewProps> = ({
  snippets,
  onUpdateSnippets,
  onExecuteSnippet,
}) => {
  const [selectedCategory, setSelectedCategory] = useState<string>('ALL');
  const [searchQuery, setSearchQuery] = useState('');
  const [copiedId, setCopiedId] = useState<string | null>(null);

  // Variable parameter dialog
  const [paramModalSnippet, setParamModalSnippet] = useState<DevOpsSnippet | null>(null);
  const [paramValues, setParamValues] = useState<Record<string, string>>({});

  // Add Snippet Modal
  const [showAddModal, setShowAddModal] = useState(false);
  const [newSnippet, setNewSnippet] = useState<Partial<DevOpsSnippet>>({
    title: '',
    category: 'Docker & K8s',
    command: '',
    description: '',
    tags: [],
  });

  const categories = [
    'ALL',
    'Docker & K8s',
    'System & Maintenance',
    'Nginx & Web',
    'Database & Storage',
    'Security & Firewall',
    'Network & Diagnostics',
  ] as const;

  const filteredSnippets = snippets.filter((s) => {
    const matchesCategory = selectedCategory === 'ALL' || s.category === selectedCategory;
    const matchesSearch =
      s.title.toLowerCase().includes(searchQuery.toLowerCase()) ||
      s.command.toLowerCase().includes(searchQuery.toLowerCase()) ||
      s.description.toLowerCase().includes(searchQuery.toLowerCase()) ||
      s.tags.some((t) => t.toLowerCase().includes(searchQuery.toLowerCase()));
    return matchesCategory && matchesSearch;
  });

  const handleCopy = (snippet: DevOpsSnippet) => {
    navigator.clipboard.writeText(snippet.command);
    setCopiedId(snippet.id);
    setTimeout(() => setCopiedId(null), 2000);
  };

  const handleRunSnippetClick = (snippet: DevOpsSnippet) => {
    if (snippet.variables && snippet.variables.length > 0) {
      const initial: Record<string, string> = {};
      snippet.variables.forEach((v) => {
        initial[v.name] = v.default;
      });
      setParamValues(initial);
      setParamModalSnippet(snippet);
    } else {
      onExecuteSnippet(snippet.command);
    }
  };

  const handleExecuteWithParams = () => {
    if (!paramModalSnippet) return;
    let finalCommand = paramModalSnippet.command;
    Object.entries(paramValues).forEach(([key, val]) => {
      finalCommand = finalCommand.replace(new RegExp(`{{${key}}}`, 'g'), val);
    });
    onExecuteSnippet(finalCommand);
    setParamModalSnippet(null);
  };

  const handleAddSnippet = () => {
    if (!newSnippet.title || !newSnippet.command) return;
    const item: DevOpsSnippet = {
      id: `snip-${Date.now()}`,
      title: newSnippet.title,
      category: newSnippet.category || 'Docker & K8s',
      command: newSnippet.command,
      description: newSnippet.description || '',
      tags: newSnippet.tags || ['Custom'],
    };
    onUpdateSnippets([...snippets, item]);
    setShowAddModal(false);
  };

  return (
    <div className="flex-1 flex flex-col bg-zinc-950 overflow-y-auto p-4 space-y-4 font-sans select-none">
      {/* Top Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 bg-zinc-900 border border-zinc-800 p-3 rounded-xl">
        <div className="flex items-center gap-3">
          <div className="w-9 h-9 rounded-lg bg-amber-500/10 border border-amber-500/30 flex items-center justify-center text-amber-400">
            <Code2 className="w-5 h-5" />
          </div>
          <div>
            <h3 className="text-sm font-semibold text-zinc-100">
              DevOps Shell Snippets & Quick Scripts
            </h3>
            <p className="text-xs text-zinc-400">
              Pre-parameterized command vault for 1-click execution in active sessions
            </p>
          </div>
        </div>

        <button
          onClick={() => setShowAddModal(true)}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg text-xs font-medium transition-colors shadow-sm"
        >
          <Plus className="w-3.5 h-3.5" />
          <span>New Snippet</span>
        </button>
      </div>

      {/* Category Pills & Search Bar */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-2">
        <div className="flex items-center gap-1 overflow-x-auto pb-1 scrollbar-none text-xs">
          {categories.map((cat) => (
            <button
              key={cat}
              onClick={() => setSelectedCategory(cat)}
              className={`px-3 py-1 rounded-lg text-xs whitespace-nowrap transition-colors ${
                selectedCategory === cat
                  ? 'bg-cyan-500/20 text-cyan-300 font-medium border border-cyan-500/40'
                  : 'text-zinc-400 hover:text-zinc-200 bg-zinc-900 border border-zinc-800'
              }`}
            >
              {cat}
            </button>
          ))}
        </div>

        <div className="relative">
          <Search className="w-3.5 h-3.5 text-zinc-500 absolute left-2.5 top-2.5" />
          <input
            type="text"
            placeholder="Search commands, tags..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="pl-8 pr-3 py-1.5 bg-zinc-900 border border-zinc-800 rounded-lg text-xs text-zinc-200 w-full sm:w-56 focus:outline-none focus:border-cyan-500"
          />
        </div>
      </div>

      {/* Snippet Grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        {filteredSnippets.map((snippet) => (
          <div
            key={snippet.id}
            className="p-3.5 bg-zinc-900/80 border border-zinc-800 hover:border-zinc-700 rounded-xl space-y-2.5 transition-all group"
          >
            {/* Card Header */}
            <div className="flex items-start justify-between">
              <div>
                <h4 className="text-xs font-semibold text-zinc-100 font-sans">
                  {snippet.title}
                </h4>
                <p className="text-[11px] text-zinc-400 mt-0.5">{snippet.description}</p>
              </div>
              <span className="text-[10px] px-2 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono">
                {snippet.category}
              </span>
            </div>

            {/* Code Box */}
            <div className="p-2.5 bg-zinc-950 border border-zinc-850 rounded-lg font-mono text-xs text-cyan-300 whitespace-pre-wrap break-all select-text">
              {snippet.command}
            </div>

            {/* Tags & Actions */}
            <div className="flex items-center justify-between pt-1">
              <div className="flex items-center gap-1.5 flex-wrap">
                {snippet.tags.map((t) => (
                  <span
                    key={t}
                    className="text-[10px] px-1.5 py-0.2 rounded bg-zinc-800/60 text-zinc-400 border border-zinc-800 font-mono"
                  >
                    #{t}
                  </span>
                ))}
              </div>

              <div className="flex items-center gap-1.5">
                <button
                  onClick={() => handleCopy(snippet)}
                  title="Copy command"
                  className="flex items-center gap-1 px-2 py-1 bg-zinc-800 hover:bg-zinc-750 text-zinc-300 rounded text-xs transition-colors"
                >
                  {copiedId === snippet.id ? (
                    <Check className="w-3.5 h-3.5 text-emerald-400" />
                  ) : (
                    <Copy className="w-3.5 h-3.5" />
                  )}
                  <span>{copiedId === snippet.id ? 'Copied' : 'Copy'}</span>
                </button>

                <button
                  onClick={() => handleRunSnippetClick(snippet)}
                  className="flex items-center gap-1 px-3 py-1 bg-cyan-600/20 hover:bg-cyan-500/30 text-cyan-300 border border-cyan-500/40 rounded text-xs font-medium transition-colors"
                >
                  <Play className="w-3 h-3 text-cyan-400 fill-cyan-400/30" />
                  <span>Run in Terminal</span>
                </button>
              </div>
            </div>
          </div>
        ))}
      </div>

      {/* Parameter Dialog for Snippets with Variables */}
      {paramModalSnippet && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
          <div className="bg-zinc-900 border border-zinc-750 rounded-xl max-w-md w-full p-4 space-y-4 shadow-2xl">
            <h4 className="text-sm font-semibold text-zinc-100 flex items-center gap-2">
              <Terminal className="w-4 h-4 text-cyan-400" />
              <span>Configure Parameters for Execution</span>
            </h4>
            <p className="text-xs text-zinc-400">
              {paramModalSnippet.title}
            </p>

            <div className="space-y-3 font-mono text-xs">
              {paramModalSnippet.variables?.map((v) => (
                <div key={v.name} className="space-y-1">
                  <label className="text-zinc-300 font-sans block">{v.label} ({`{{${v.name}}}`})</label>
                  <input
                    type="text"
                    value={paramValues[v.name] || ''}
                    onChange={(e) =>
                      setParamValues({ ...paramValues, [v.name]: e.target.value })
                    }
                    className="w-full px-3 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500"
                  />
                </div>
              ))}
            </div>

            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={() => setParamModalSnippet(null)}
                className="px-3 py-1.5 text-xs text-zinc-400 hover:text-zinc-200 font-sans"
              >
                Cancel
              </button>
              <button
                onClick={handleExecuteWithParams}
                className="px-3.5 py-1.5 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg text-xs font-medium font-sans flex items-center gap-1.5"
              >
                <Play className="w-3.5 h-3.5" />
                <span>Execute Command</span>
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Add Custom Snippet Modal */}
      {showAddModal && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
          <div className="bg-zinc-900 border border-zinc-750 rounded-xl max-w-md w-full p-4 space-y-4 shadow-2xl">
            <h4 className="text-sm font-semibold text-zinc-100 flex items-center gap-2">
              <Plus className="w-4 h-4 text-cyan-400" />
              <span>Create DevOps Snippet</span>
            </h4>

            <div className="space-y-3 text-xs">
              <div>
                <label className="text-zinc-400 block mb-1">Snippet Title</label>
                <input
                  type="text"
                  placeholder="e.g. Restart Kubernetes Rollout"
                  value={newSnippet.title || ''}
                  onChange={(e) => setNewSnippet({ ...newSnippet, title: e.target.value })}
                  className="w-full px-3 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none"
                />
              </div>

              <div>
                <label className="text-zinc-400 block mb-1">Category</label>
                <select
                  value={newSnippet.category}
                  onChange={(e) => setNewSnippet({ ...newSnippet, category: e.target.value as any })}
                  className="w-full px-3 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none"
                >
                  {categories.filter((c) => c !== 'ALL').map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
              </div>

              <div>
                <label className="text-zinc-400 block mb-1 font-mono">Bash Command</label>
                <textarea
                  rows={3}
                  placeholder="e.g. kubectl rollout restart deployment/{{app}}"
                  value={newSnippet.command || ''}
                  onChange={(e) => setNewSnippet({ ...newSnippet, command: e.target.value })}
                  className="w-full px-3 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 font-mono text-xs focus:outline-none"
                />
              </div>
            </div>

            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={() => setShowAddModal(false)}
                className="px-3 py-1.5 text-xs text-zinc-400 hover:text-zinc-200"
              >
                Cancel
              </button>
              <button
                onClick={handleAddSnippet}
                className="px-3.5 py-1.5 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg text-xs font-medium"
              >
                Save Snippet
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
