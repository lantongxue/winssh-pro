import React, { useState, useEffect } from 'react';
import {
  Code2,
  X,
  Plus,
  Tag,
  FileText,
  Terminal,
  Layers,
  Sparkles,
  Info,
  Check,
  Trash2,
} from 'lucide-react';
import { DevOpsSnippet, SnippetVariable } from '../types';

interface SnippetModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSaveSnippet: (snippet: DevOpsSnippet) => void;
  onDeleteSnippet?: (snippetId: string) => void;
  initialSnippet?: DevOpsSnippet | null;
}

const CATEGORIES: DevOpsSnippet['category'][] = [
  'Docker & K8s',
  'System & Maintenance',
  'Nginx & Web',
  'Database & Storage',
  'Security & Firewall',
  'Network & Diagnostics',
  'Git & DevOps',
];

const PRESET_TEMPLATES: {
  name: string;
  category: DevOpsSnippet['category'];
  title: string;
  command: string;
  description: string;
  tags: string[];
}[] = [
  {
    name: 'Docker Prune All',
    category: 'Docker & K8s',
    title: 'Docker System Prune (Deep Clean)',
    command: 'docker system prune -a --volumes -f',
    description: 'Remove all unused containers, networks, images (both dangling and unreferenced), and volumes.',
    tags: ['docker', 'clean', 'disk'],
  },
  {
    name: 'Follow Container Logs',
    category: 'Docker & K8s',
    title: 'Docker Live Logs Stream',
    command: 'docker logs -f --tail={{lines}} {{container_name}}',
    description: 'Stream logs from a target container with custom tail length.',
    tags: ['docker', 'logs', 'stream'],
  },
  {
    name: 'Find Top Large Files',
    category: 'System & Maintenance',
    title: 'Find Top 10 Largest Files in Directory',
    command: 'find {{directory}} -type f -exec du -Sh {} + | sort -rh | head -n 10',
    description: 'Scan disk and list the top 10 largest files in specified path.',
    tags: ['system', 'disk', 'find'],
  },
  {
    name: 'Nginx Config Test & Reload',
    category: 'Nginx & Web',
    title: 'Nginx Test Config & Zero-Downtime Reload',
    command: 'nginx -t && systemctl reload nginx',
    description: 'Verify syntax of Nginx configuration files and perform graceful reload.',
    tags: ['nginx', 'web', 'reload'],
  },
  {
    name: 'MySQL Database Dump',
    category: 'Database & Storage',
    title: 'MySQL Compressed DB Backup',
    command: 'mysqldump -u {{user}} -p{{password}} {{dbname}} | gzip > /backups/{{dbname}}_$(date +%F).sql.gz',
    description: 'Export and compress a MySQL database to backup archive.',
    tags: ['mysql', 'backup', 'database'],
  },
  {
    name: 'Check Listening Ports',
    category: 'Network & Diagnostics',
    title: 'Check All Active Listening Ports (ss)',
    command: 'ss -tulpn | grep LISTEN',
    description: 'Display all listening TCP/UDP sockets with process names and PIDs.',
    tags: ['network', 'ports', 'ss'],
  },
  {
    name: 'Git Force Pull & Reset',
    category: 'Git & DevOps',
    title: 'Git Force Sync with Remote Branch',
    command: 'git fetch origin && git reset --hard origin/{{branch}}',
    description: 'Discard all local changes and strictly align with the upstream branch.',
    tags: ['git', 'deploy', 'sync'],
  },
];

export const SnippetModal: React.FC<SnippetModalProps> = ({
  isOpen,
  onClose,
  onSaveSnippet,
  onDeleteSnippet,
  initialSnippet,
}) => {
  const isEditing = !!initialSnippet;

  const [title, setTitle] = useState('');
  const [category, setCategory] = useState<DevOpsSnippet['category']>('Docker & K8s');
  const [command, setCommand] = useState('');
  const [description, setDescription] = useState('');
  const [tagsInput, setTagsInput] = useState('');
  const [detectedVars, setDetectedVars] = useState<string[]>([]);
  const [showConfirmDelete, setShowConfirmDelete] = useState(false);

  useEffect(() => {
    if (initialSnippet) {
      setTitle(initialSnippet.title);
      setCategory(initialSnippet.category);
      setCommand(initialSnippet.command);
      setDescription(initialSnippet.description || '');
      setTagsInput((initialSnippet.tags || []).join(', '));
    } else {
      setTitle('');
      setCategory('Docker & K8s');
      setCommand('');
      setDescription('');
      setTagsInput('devops, custom');
    }
    setShowConfirmDelete(false);
  }, [initialSnippet, isOpen]);

  // Detect {{variables}} in command automatically
  useEffect(() => {
    const matches = command.match(/\{\{([a-zA-Z0-9_]+)\}\}/g);
    if (matches) {
      const unique = Array.from(new Set(matches.map((m) => m.replace(/\{\{|\}\}/g, ''))));
      setDetectedVars(unique);
    } else {
      setDetectedVars([]);
    }
  }, [command]);

  if (!isOpen) return null;

  const handleApplyPreset = (preset: typeof PRESET_TEMPLATES[0]) => {
    setTitle(preset.title);
    setCategory(preset.category);
    setCommand(preset.command);
    setDescription(preset.description);
    setTagsInput(preset.tags.join(', '));
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!title.trim() || !command.trim()) return;

    const parsedTags = tagsInput
      .split(',')
      .map((t) => t.trim().replace(/^#/, ''))
      .filter(Boolean);

    // Build variables object if detected
    const variables: SnippetVariable[] = detectedVars.map((v) => {
      const existing = initialSnippet?.variables?.find((ev) => ev.name === v);
      return {
        name: v,
        default: existing?.default || (v.includes('port') ? '8080' : v.includes('lines') ? '100' : 'default'),
        label: existing?.label || v.replace(/_/g, ' ').toUpperCase(),
      };
    });

    const snippetData: DevOpsSnippet = {
      id: initialSnippet?.id || `snip-${Date.now()}`,
      title: title.trim(),
      category,
      command: command.trim(),
      description: description.trim(),
      tags: parsedTags.length > 0 ? parsedTags : ['custom'],
      variables: variables.length > 0 ? variables : undefined,
    };

    onSaveSnippet(snippetData);
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/75 backdrop-blur-xs animate-in fade-in duration-150">
      <div className="w-full max-w-lg bg-[#1E1E1E] border border-[#3C3C3C] rounded-xl shadow-2xl overflow-hidden flex flex-col font-sans max-h-[90vh]">
        {/* Header */}
        <div className="px-4 py-3 bg-[#252526] border-b border-[#333333] flex items-center justify-between">
          <div className="flex items-center gap-2.5">
            <div className="w-7 h-7 rounded-lg bg-[#007ACC]/20 border border-[#007ACC]/40 flex items-center justify-center text-[#4EC9B0]">
              <Code2 className="w-4 h-4" />
            </div>
            <div>
              <h3 className="text-sm font-semibold text-[#D4D4D4]">
                {isEditing ? 'Edit DevOps Snippet' : 'Create New Command Snippet'}
              </h3>
              <p className="text-[11px] text-[#858585]">
                Save reusable shell scripts & parameterized commands
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="p-1 text-[#858585] hover:text-white hover:bg-[#333333] rounded-md transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Form Body */}
        <form onSubmit={handleSubmit} className="flex-1 overflow-y-auto p-4 space-y-3.5 text-xs">
          {/* Quick Presets / Templates Dropdown */}
          {!isEditing && (
            <div className="bg-[#181818] p-2.5 rounded-lg border border-[#2D2D2D] space-y-1.5">
              <div className="flex items-center justify-between text-[11px] text-[#858585]">
                <span className="flex items-center gap-1 font-medium text-[#CCCCCC]">
                  <Sparkles className="w-3 h-3 text-[#DCDCAA]" />
                  Quick Snippet Templates
                </span>
                <span>Click to pre-fill</span>
              </div>
              <div className="flex flex-wrap gap-1.5 pt-0.5">
                {PRESET_TEMPLATES.map((preset) => (
                  <button
                    key={preset.name}
                    type="button"
                    onClick={() => handleApplyPreset(preset)}
                    className="px-2 py-0.5 bg-[#252526] hover:bg-[#333333] border border-[#3C3C3C] hover:border-[#007ACC] text-[#D4D4D4] rounded text-[10.5px] transition-colors"
                  >
                    {preset.name}
                  </button>
                ))}
              </div>
            </div>
          )}

          {/* Snippet Title */}
          <div className="space-y-1">
            <label className="block text-[11px] font-medium text-[#CCCCCC]">
              Snippet Title <span className="text-[#FF5F56]">*</span>
            </label>
            <input
              type="text"
              required
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. Nginx Graceful Reload & Test"
              className="w-full px-2.5 py-1.5 bg-[#252526] border border-[#3C3C3C] rounded-md text-[#D4D4D4] placeholder-[#6D6D6D] focus:outline-none focus:border-[#007ACC]"
            />
          </div>

          {/* Category & Tags */}
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="block text-[11px] font-medium text-[#CCCCCC]">
                Category <span className="text-[#FF5F56]">*</span>
              </label>
              <select
                value={category}
                onChange={(e) => setCategory(e.target.value as DevOpsSnippet['category'])}
                className="w-full px-2 py-1.5 bg-[#252526] border border-[#3C3C3C] rounded-md text-[#D4D4D4] focus:outline-none focus:border-[#007ACC]"
              >
                {CATEGORIES.map((cat) => (
                  <option key={cat} value={cat}>
                    {cat}
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-1">
              <label className="block text-[11px] font-medium text-[#CCCCCC]">
                Tags <span className="text-[10px] text-[#858585]">(comma-separated)</span>
              </label>
              <input
                type="text"
                value={tagsInput}
                onChange={(e) => setTagsInput(e.target.value)}
                placeholder="docker, maintenance, logs"
                className="w-full px-2.5 py-1.5 bg-[#252526] border border-[#3C3C3C] rounded-md text-[#D4D4D4] placeholder-[#6D6D6D] focus:outline-none focus:border-[#007ACC]"
              />
            </div>
          </div>

          {/* Command Script Textarea */}
          <div className="space-y-1">
            <div className="flex items-center justify-between">
              <label className="block text-[11px] font-medium text-[#CCCCCC]">
                Shell Command / Script <span className="text-[#FF5F56]">*</span>
              </label>
              <span className="text-[10px] text-[#858585] font-mono">
                Use <span className="text-[#4EC9B0]">&#123;&#123;variable&#125;&#125;</span> for params
              </span>
            </div>
            <textarea
              required
              rows={4}
              value={command}
              onChange={(e) => setCommand(e.target.value)}
              placeholder="e.g. docker logs -f --tail={{lines}} {{container_name}}"
              className="w-full px-2.5 py-2 bg-[#121212] border border-[#3C3C3C] rounded-md text-[#4EC9B0] font-mono text-[11px] placeholder-[#6D6D6D] focus:outline-none focus:border-[#007ACC] leading-relaxed"
            />
          </div>

          {/* Dynamic Detected Variables Indicator */}
          {detectedVars.length > 0 && (
            <div className="p-2 bg-[#007ACC]/10 border border-[#007ACC]/30 rounded-md text-[11px] space-y-1">
              <div className="flex items-center gap-1.5 text-[#4EC9B0] font-medium">
                <Info className="w-3.5 h-3.5" />
                <span>Detected Parameters ({detectedVars.length}):</span>
              </div>
              <div className="flex flex-wrap gap-1 pt-0.5">
                {detectedVars.map((v) => (
                  <span
                    key={v}
                    className="px-1.5 py-0.5 rounded bg-[#007ACC]/25 text-[#9CDCFE] font-mono text-[10px] border border-[#007ACC]/40"
                  >
                    &#123;&#123;{v}&#125;&#125;
                  </span>
                ))}
              </div>
              <p className="text-[10px] text-[#858585]">
                When executing this snippet, a prompt dialog will automatically request values for these parameters.
              </p>
            </div>
          )}

          {/* Description (Optional) */}
          <div className="space-y-1">
            <label className="block text-[11px] font-medium text-[#CCCCCC]">
              Description / Documentation <span className="text-[10px] text-[#858585]">(Optional)</span>
            </label>
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Brief summary of what this command accomplishes"
              className="w-full px-2.5 py-1.5 bg-[#252526] border border-[#3C3C3C] rounded-md text-[#D4D4D4] placeholder-[#6D6D6D] focus:outline-none focus:border-[#007ACC]"
            />
          </div>

          {/* Footer Actions */}
          <div className="pt-2 flex items-center justify-between border-t border-[#333333]">
            {isEditing && onDeleteSnippet ? (
              showConfirmDelete ? (
                <div className="flex items-center gap-1.5">
                  <span className="text-[11px] text-[#FF5F56]">Confirm delete?</span>
                  <button
                    type="button"
                    onClick={() => {
                      if (initialSnippet) {
                        onDeleteSnippet(initialSnippet.id);
                        onClose();
                      }
                    }}
                    className="px-2 py-1 bg-[#FF5F56] hover:bg-[#FF5F56]/80 text-white rounded text-[11px] font-semibold transition-colors"
                  >
                    Yes, Delete
                  </button>
                  <button
                    type="button"
                    onClick={() => setShowConfirmDelete(false)}
                    className="px-2 py-1 bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#CCCCCC] rounded text-[11px] transition-colors"
                  >
                    Cancel
                  </button>
                </div>
              ) : (
                <button
                  type="button"
                  onClick={() => setShowConfirmDelete(true)}
                  className="flex items-center gap-1 text-[#858585] hover:text-[#FF5F56] transition-colors p-1"
                >
                  <Trash2 className="w-3.5 h-3.5" />
                  <span>Delete Snippet</span>
                </button>
              )
            ) : (
              <div />
            )}

            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={onClose}
                className="px-3 py-1.5 rounded-md bg-[#2D2D2D] hover:bg-[#3C3C3C] text-[#CCCCCC] transition-colors"
              >
                Cancel
              </button>
              <button
                type="submit"
                className="flex items-center gap-1 px-4 py-1.5 rounded-md bg-[#007ACC] hover:bg-[#007ACC]/80 text-white font-semibold transition-colors shadow-sm"
              >
                <Check className="w-3.5 h-3.5" />
                <span>{isEditing ? 'Save Changes' : 'Create Snippet'}</span>
              </button>
            </div>
          </div>
        </form>
      </div>
    </div>
  );
};
