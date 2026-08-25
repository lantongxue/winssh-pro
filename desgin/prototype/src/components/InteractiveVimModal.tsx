import React, { useState, useEffect } from 'react';
import {
  FileCode,
  Save,
  X,
  Sparkles,
  Check,
  RotateCcw,
  Copy,
  Terminal,
} from 'lucide-react';

interface InteractiveVimModalProps {
  isOpen: boolean;
  onClose: () => void;
  file: {
    path: string;
    name: string;
    content: string;
  };
  onSave: (path: string, newContent: string) => void;
  onAskAI: (fileContent: string, taskType: string) => void;
}

export const InteractiveVimModal: React.FC<InteractiveVimModalProps> = ({
  isOpen,
  onClose,
  file,
  onSave,
  onAskAI,
}) => {
  const [content, setContent] = useState(file.content);
  const [savedSuccess, setSavedSuccess] = useState(false);

  useEffect(() => {
    setContent(file.content);
    setSavedSuccess(false);
  }, [file]);

  if (!isOpen) return null;

  const lines = content.split('\n');

  const handleSave = () => {
    onSave(file.path, content);
    setSavedSuccess(true);
    setTimeout(() => setSavedSuccess(false), 2000);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.ctrlKey || e.metaKey) && e.key === 's') {
      e.preventDefault();
      handleSave();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      const target = e.currentTarget;
      const start = target.selectionStart;
      const end = target.selectionEnd;
      const updated = content.substring(0, start) + '  ' + content.substring(end);
      setContent(updated);
      setTimeout(() => {
        target.selectionStart = target.selectionEnd = start + 2;
      }, 0);
    }
  };

  const getLanguage = (fileName: string) => {
    if (fileName.endsWith('.sh') || fileName.endsWith('.bash')) return 'Bash / Shell';
    if (fileName.endsWith('.yml') || fileName.endsWith('.yaml')) return 'YAML / K8s';
    if (fileName.endsWith('.json')) return 'JSON';
    if (fileName.endsWith('.conf') || fileName.includes('nginx')) return 'Nginx Conf';
    if (fileName.endsWith('.py')) return 'Python 3';
    return 'Plain Text';
  };

  return (
    <div className="fixed inset-0 bg-black/80 backdrop-blur-sm z-50 flex items-center justify-center p-4 select-none">
      <div className="bg-zinc-900 border border-zinc-750 rounded-2xl max-w-4xl w-full h-[85vh] shadow-2xl flex flex-col font-sans overflow-hidden">
        {/* Editor Top Bar */}
        <div className="h-11 bg-zinc-950 border-b border-zinc-800 px-4 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <FileCode className="w-4 h-4 text-cyan-400" />
            <span className="text-xs font-semibold text-zinc-100 font-mono">
              {file.name}
            </span>
            <span className="text-[11px] text-zinc-500 font-mono truncate max-w-md">
              ({file.path})
            </span>
          </div>

          <div className="flex items-center gap-2 text-xs">
            <span className="px-2 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono text-[10px]">
              {getLanguage(file.name)}
            </span>

            <button
              onClick={() => onAskAI(`Analyze and refactor this file:\n\`\`\`\n${content}\n\`\`\``, 'refactor_code')}
              className="flex items-center gap-1 px-2.5 py-1 bg-indigo-600/20 hover:bg-indigo-600/30 text-indigo-300 border border-indigo-500/40 rounded-lg transition-colors font-medium"
            >
              <Sparkles className="w-3.5 h-3.5" />
              <span>AI Review</span>
            </button>

            <button
              onClick={handleSave}
              className="flex items-center gap-1.5 px-3 py-1 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg transition-colors font-medium shadow-sm"
            >
              {savedSuccess ? <Check className="w-3.5 h-3.5" /> : <Save className="w-3.5 h-3.5" />}
              <span>{savedSuccess ? 'Saved!' : 'Save (Ctrl+S)'}</span>
            </button>

            <button
              onClick={onClose}
              className="p-1 hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 rounded-lg transition-colors"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Editor Code Area with Line Numbers */}
        <div className="flex-1 flex bg-zinc-950 overflow-hidden font-mono text-xs select-text">
          {/* Left Gutter: Line Numbers */}
          <div className="w-12 bg-zinc-900/60 border-r border-zinc-850 text-zinc-600 text-right pr-2.5 py-3 select-none overflow-hidden leading-5">
            {lines.map((_, i) => (
              <div key={i}>{i + 1}</div>
            ))}
          </div>

          {/* Code Textarea */}
          <div className="flex-1 relative overflow-hidden">
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              onKeyDown={handleKeyDown}
              spellCheck={false}
              autoFocus
              className="w-full h-full p-3 bg-transparent text-cyan-200 font-mono text-xs leading-5 resize-none focus:outline-none scrollbar-thin scrollbar-thumb-zinc-800"
            />
          </div>
        </div>

        {/* Bottom Status Bar */}
        <div className="h-6 bg-zinc-950 border-t border-zinc-800 px-3 flex items-center justify-between text-[11px] font-mono text-zinc-500 select-none">
          <div className="flex items-center gap-3">
            <span>Lines: {lines.length}</span>
            <span>Length: {content.length} bytes</span>
          </div>

          <div className="flex items-center gap-3">
            <span>Encoding: UTF-8</span>
            <span>LF (Unix)</span>
            <span>Tab Size: 2</span>
          </div>
        </div>
      </div>
    </div>
  );
};
