import React, { useState, useRef, useEffect } from 'react';
import {
  Sparkles,
  X,
  Send,
  Play,
  Copy,
  Check,
  Bot,
  User,
  Zap,
  Terminal,
  ShieldCheck,
  Cpu,
  FileCode,
  RotateCw,
} from 'lucide-react';
import { ServerHost, AICopilotMessage } from '../types';

interface AICopilotDrawerProps {
  isOpen: boolean;
  onClose: () => void;
  currentHost?: ServerHost;
  onExecuteInTerminal: (command: string) => void;
  externalPrompt?: { text: string; taskType?: string };
}

export const AICopilotDrawer: React.FC<AICopilotDrawerProps> = ({
  isOpen,
  onClose,
  currentHost,
  onExecuteInTerminal,
  externalPrompt,
}) => {
  const [messages, setMessages] = useState<AICopilotMessage[]>([
    {
      id: 'welcome',
      role: 'assistant',
      content: `Hello! I am **WinSSH Pro AI Copilot** 🤖. I can help you diagnose server errors, craft complex Docker / Kubernetes manifests, optimize Linux performance, and analyze logs in real-time.`,
      timestamp: 'Just now',
    },
  ]);

  const [inputQuery, setInputQuery] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const messagesEndRef = useRef<HTMLDivElement | null>(null);

  // Handle incoming prompt from terminal context menu or quick action
  useEffect(() => {
    if (externalPrompt && externalPrompt.text) {
      handleSendMessage(externalPrompt.text, externalPrompt.taskType);
    }
  }, [externalPrompt]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, isLoading]);

  const handleSendMessage = async (customPrompt?: string, taskType?: string) => {
    const query = customPrompt || inputQuery;
    if (!query.trim() || isLoading) return;

    const userMsg: AICopilotMessage = {
      id: `usr-${Date.now()}`,
      role: 'user',
      content: query,
      timestamp: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }),
    };

    setMessages((prev) => [...prev, userMsg]);
    if (!customPrompt) setInputQuery('');
    setIsLoading(true);

    try {
      const response = await fetch('/api/ai/copilot', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          prompt: query,
          taskType: taskType || 'general',
          context: {
            hostName: currentHost?.name || 'Linux Server',
            osType: currentHost?.osType || 'Ubuntu Linux 24.04',
            ip: currentHost?.host || '127.0.0.1',
            user: currentHost?.username || 'root',
          },
        }),
      });

      const data = await response.json();
      const assistantMsg: AICopilotMessage = {
        id: `ai-${Date.now()}`,
        role: 'assistant',
        content: data.reply || 'Analysis completed.',
        codeSnippet: data.codeSnippet,
        commandSuggestions: data.commandSuggestions,
        timestamp: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }),
      };

      setMessages((prev) => [...prev, assistantMsg]);
    } catch (err) {
      const errorMsg: AICopilotMessage = {
        id: `err-${Date.now()}`,
        role: 'assistant',
        content: `Error connecting to AI backend. Please verify network connectivity.`,
        timestamp: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }),
      };
      setMessages((prev) => [...prev, errorMsg]);
    } finally {
      setIsLoading(false);
    }
  };

  const handleCopy = (text: string, id: string) => {
    navigator.clipboard.writeText(text);
    setCopiedId(id);
    setTimeout(() => setCopiedId(null), 2000);
  };

  if (!isOpen) return null;

  return (
    <div className="w-80 sm:w-96 h-full bg-zinc-900 border-l border-zinc-800 flex flex-col shadow-2xl z-30 font-sans select-none">
      {/* Drawer Header */}
      <div className="p-3 bg-zinc-950 border-b border-zinc-800 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <div className="w-6 h-6 rounded-md bg-indigo-500/20 border border-indigo-500/40 flex items-center justify-center text-indigo-400">
            <Sparkles className="w-3.5 h-3.5" />
          </div>
          <div>
            <h3 className="text-xs font-semibold text-zinc-100 flex items-center gap-1.5">
              <span>DevOps AI Copilot</span>
              <span className="text-[9px] px-1 py-0.2 rounded bg-indigo-500/20 text-indigo-300 border border-indigo-500/30">
                Gemini 2.5
              </span>
            </h3>
            <span className="text-[10px] text-zinc-500 font-mono">
              Target: {currentHost?.name || 'Local'} ({currentHost?.osType || 'Linux'})
            </span>
          </div>
        </div>

        <button
          onClick={onClose}
          className="p-1 hover:bg-zinc-800 rounded text-zinc-400 hover:text-zinc-200 transition-colors"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      {/* Quick Prompt Presets */}
      <div className="p-2 bg-zinc-900/60 border-b border-zinc-850 flex items-center gap-1 overflow-x-auto scrollbar-none text-[10px]">
        {[
          { label: '🔍 Explain Logs', prompt: 'Analyze recent server logs and explain potential root causes and fixes.' },
          { label: '🛡️ Audit Security', prompt: 'Give me bash commands to audit open ports, SSH root login security, and firewall UFW rules.' },
          { label: '🐳 Optimize Docker', prompt: 'Provide cleanup commands for unused Docker images, volumes, and build cache.' },
          { label: '⚡ Sysctl Tuning', prompt: 'Provide kernel sysctl.conf network performance tuning parameters for a high-traffic server.' },
        ].map((btn) => (
          <button
            key={btn.label}
            onClick={() => handleSendMessage(btn.prompt)}
            className="px-2 py-1 bg-zinc-850 hover:bg-zinc-800 text-zinc-300 rounded whitespace-nowrap transition-colors border border-zinc-800"
          >
            {btn.label}
          </button>
        ))}
      </div>

      {/* Messages List Area */}
      <div className="flex-1 overflow-y-auto p-3 space-y-3 font-sans text-xs select-text">
        {messages.map((msg) => (
          <div
            key={msg.id}
            className={`flex flex-col space-y-1.5 ${
              msg.role === 'user' ? 'items-end' : 'items-start'
            }`}
          >
            <div className="flex items-center gap-1 text-[10px] text-zinc-500 select-none">
              {msg.role === 'assistant' ? (
                <>
                  <Bot className="w-3 h-3 text-indigo-400" />
                  <span className="text-zinc-400 font-medium">Copilot</span>
                </>
              ) : (
                <>
                  <User className="w-3 h-3 text-cyan-400" />
                  <span className="text-zinc-400 font-medium">You</span>
                </>
              )}
              <span>• {msg.timestamp}</span>
            </div>

            {/* Message Bubble */}
            <div
              className={`p-3 rounded-xl max-w-[95%] leading-relaxed ${
                msg.role === 'user'
                  ? 'bg-cyan-600 text-white rounded-tr-none'
                  : 'bg-zinc-950 border border-zinc-800 text-zinc-200 rounded-tl-none shadow-sm'
              }`}
            >
              <div className="whitespace-pre-wrap">{msg.content}</div>

              {/* Code Snippet Box (if any) */}
              {msg.codeSnippet && (
                <div className="mt-2.5 bg-zinc-900 border border-zinc-800 rounded-lg overflow-hidden select-none">
                  <div className="p-1.5 bg-zinc-850 flex items-center justify-between text-[10px] text-zinc-400">
                    <span className="font-mono">Suggested Command</span>
                    <div className="flex items-center gap-1">
                      <button
                        onClick={() => handleCopy(msg.codeSnippet!, msg.id)}
                        className="flex items-center gap-0.5 px-1.5 py-0.5 hover:bg-zinc-750 text-zinc-300 rounded"
                      >
                        {copiedId === msg.id ? (
                          <Check className="w-3 h-3 text-emerald-400" />
                        ) : (
                          <Copy className="w-3 h-3" />
                        )}
                        <span>{copiedId === msg.id ? 'Copied' : 'Copy'}</span>
                      </button>

                      <button
                        onClick={() => onExecuteInTerminal(msg.codeSnippet!)}
                        className="flex items-center gap-0.5 px-2 py-0.5 bg-cyan-600/30 hover:bg-cyan-600/40 text-cyan-300 border border-cyan-500/40 rounded font-medium"
                      >
                        <Play className="w-3 h-3" />
                        <span>Run in SSH</span>
                      </button>
                    </div>
                  </div>

                  <pre className="p-2.5 font-mono text-xs text-cyan-300 overflow-x-auto whitespace-pre select-text">
                    {msg.codeSnippet}
                  </pre>
                </div>
              )}
            </div>
          </div>
        ))}

        {isLoading && (
          <div className="flex items-center gap-2 p-2.5 bg-zinc-950/70 border border-zinc-800 rounded-xl text-zinc-400 text-xs">
            <span className="w-3.5 h-3.5 border-2 border-indigo-400 border-t-transparent rounded-full animate-spin" />
            <span>AI Copilot is analyzing infrastructure context...</span>
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>

      {/* Input Prompt Box */}
      <div className="p-2.5 bg-zinc-950 border-t border-zinc-800 flex items-center gap-2">
        <input
          type="text"
          placeholder="Ask Copilot (e.g. how to diagnose high CPU?)..."
          value={inputQuery}
          onChange={(e) => setInputQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') handleSendMessage();
          }}
          className="flex-1 px-3 py-2 bg-zinc-900 border border-zinc-800 rounded-lg text-xs text-zinc-100 placeholder-zinc-500 focus:outline-none focus:border-indigo-500"
        />
        <button
          disabled={isLoading || !inputQuery.trim()}
          onClick={() => handleSendMessage()}
          className="p-2 bg-indigo-600 hover:bg-indigo-500 disabled:opacity-40 text-white rounded-lg transition-colors"
        >
          <Send className="w-4 h-4" />
        </button>
      </div>
    </div>
  );
};
