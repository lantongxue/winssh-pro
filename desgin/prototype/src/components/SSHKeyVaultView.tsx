import React, { useState } from 'react';
import {
  KeyRound,
  Plus,
  Copy,
  Check,
  Download,
  Trash2,
  ShieldCheck,
  Lock,
  FileCode,
  Sparkles,
  Key,
} from 'lucide-react';
import { SSHKeyItem } from '../types';

interface SSHKeyVaultViewProps {
  keys: SSHKeyItem[];
  onUpdateKeys: (keys: SSHKeyItem[]) => void;
}

export const SSHKeyVaultView: React.FC<SSHKeyVaultViewProps> = ({ keys, onUpdateKeys }) => {
  const [copiedKeyId, setCopiedKeyId] = useState<string | null>(null);
  const [showGenerateModal, setShowGenerateModal] = useState(false);
  const [genType, setGenType] = useState<'Ed25519' | 'RSA 4096' | 'ECDSA P-384'>('Ed25519');
  const [genName, setGenName] = useState('');
  const [genComment, setGenComment] = useState('winssh-key@prod');
  const [isGenerating, setIsGenerating] = useState(false);

  const handleCopyPublicKey = (key: SSHKeyItem) => {
    navigator.clipboard.writeText(key.publicKey);
    setCopiedKeyId(key.id);
    setTimeout(() => setCopiedKeyId(null), 2000);
  };

  const handleDownloadPrivateKey = (key: SSHKeyItem) => {
    const blob = new Blob([key.privateKey], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${key.name}.pem`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleDeleteKey = (keyId: string) => {
    onUpdateKeys(keys.filter((k) => k.id !== keyId));
  };

  const handleGenerateKey = () => {
    if (!genName.trim()) return;
    setIsGenerating(true);

    setTimeout(() => {
      const randomHex = Array.from({ length: 44 }, () =>
        'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'.charAt(
          Math.floor(Math.random() * 64)
        )
      ).join('');

      const fingerprint = `SHA256:${Array.from({ length: 43 }, () =>
        '0123456789abcdef'.charAt(Math.floor(Math.random() * 16))
      ).join('')}`;

      const pubKey =
        genType === 'Ed25519'
          ? `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI${randomHex.substring(0, 32)} ${genComment}`
          : `ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQD${randomHex}== ${genComment}`;

      const privKey =
        genType === 'Ed25519'
          ? `-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAAC${randomHex}AAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\nAAAAHndpbnNzaC1wcm9AZGVlcG1pbmQtY2x1c3RlcgECAwQFBgc=\n-----END OPENSSH PRIVATE KEY-----`
          : `-----BEGIN RSA PRIVATE KEY-----\nMIIJKQIBAAKCAQEA${randomHex}...\n[WinSSH Generated RSA 4096 Private Key Material]\n-----END RSA PRIVATE KEY-----`;

      const newKeyItem: SSHKeyItem = {
        id: `key-${Date.now()}`,
        name: genName.trim(),
        type: genType,
        fingerprint,
        publicKey: pubKey,
        privateKey: privKey,
        createdDate: new Date().toISOString().replace('T', ' ').substring(0, 19),
        comment: genComment,
      };

      onUpdateKeys([...keys, newKeyItem]);
      setIsGenerating(false);
      setShowGenerateModal(false);
      setGenName('');
    }, 600);
  };

  return (
    <div className="flex-1 flex flex-col bg-zinc-950 overflow-y-auto p-4 space-y-4 font-sans select-none">
      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 bg-zinc-900 border border-zinc-800 p-3 rounded-xl">
        <div className="flex items-center gap-3">
          <div className="w-9 h-9 rounded-lg bg-emerald-500/10 border border-emerald-500/30 flex items-center justify-center text-emerald-400">
            <KeyRound className="w-5 h-5" />
          </div>
          <div>
            <h3 className="text-sm font-semibold text-zinc-100">
              SSH Key Management & Generator
            </h3>
            <p className="text-xs text-zinc-400">
              Store, generate, and deploy Ed25519 and RSA 4096-bit public/private keys securely
            </p>
          </div>
        </div>

        <button
          onClick={() => setShowGenerateModal(true)}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg text-xs font-medium transition-colors shadow-sm"
        >
          <Plus className="w-3.5 h-3.5" />
          <span>Generate Keypair</span>
        </button>
      </div>

      {/* Keys List Grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        {keys.map((key) => (
          <div
            key={key.id}
            className="p-4 bg-zinc-900/80 border border-zinc-800 rounded-xl space-y-3"
          >
            {/* Card Header */}
            <div className="flex items-start justify-between">
              <div className="flex items-center gap-2">
                <div className="p-2 rounded-lg bg-zinc-950 border border-zinc-800 text-emerald-400">
                  <ShieldCheck className="w-4 h-4" />
                </div>
                <div>
                  <h4 className="text-xs font-semibold text-zinc-100 font-mono">
                    {key.name}
                  </h4>
                  <span className="text-[10px] text-zinc-400 font-mono">
                    {key.comment || 'No comment'}
                  </span>
                </div>
              </div>

              <span
                className={`text-[10px] uppercase font-mono px-2 py-0.5 rounded ${
                  key.type === 'Ed25519'
                    ? 'bg-emerald-500/20 text-emerald-300 border border-emerald-500/30'
                    : 'bg-blue-500/20 text-blue-300 border border-blue-500/30'
                }`}
              >
                {key.type}
              </span>
            </div>

            {/* Fingerprint */}
            <div className="p-2 bg-zinc-950 rounded-lg border border-zinc-850 space-y-1 font-mono text-xs">
              <span className="text-[10px] text-zinc-500 block">Fingerprint:</span>
              <span className="text-zinc-300 text-[11px] truncate block">
                {key.fingerprint}
              </span>
            </div>

            {/* Public Key snippet */}
            <div className="p-2 bg-zinc-950/60 rounded-lg border border-zinc-850 text-[11px] font-mono text-zinc-400 truncate">
              {key.publicKey}
            </div>

            {/* Actions */}
            <div className="flex items-center justify-between pt-1 border-t border-zinc-850 text-xs">
              <span className="text-[10px] text-zinc-500 font-mono">
                Created: {key.createdDate}
              </span>

              <div className="flex items-center gap-1.5">
                <button
                  onClick={() => handleCopyPublicKey(key)}
                  className="flex items-center gap-1 px-2.5 py-1 bg-zinc-800 hover:bg-zinc-750 text-zinc-200 rounded-md transition-colors"
                >
                  {copiedKeyId === key.id ? (
                    <Check className="w-3 h-3 text-emerald-400" />
                  ) : (
                    <Copy className="w-3 h-3" />
                  )}
                  <span>{copiedKeyId === key.id ? 'Copied' : 'Copy PubKey'}</span>
                </button>

                <button
                  onClick={() => handleDownloadPrivateKey(key)}
                  title="Download .pem Private Key"
                  className="p-1.5 bg-zinc-800 hover:bg-zinc-750 text-zinc-300 rounded-md transition-colors"
                >
                  <Download className="w-3.5 h-3.5" />
                </button>

                <button
                  onClick={() => handleDeleteKey(key.id)}
                  title="Delete Key"
                  className="p-1.5 text-zinc-500 hover:text-red-400 rounded hover:bg-zinc-800 transition-colors"
                >
                  <Trash2 className="w-3.5 h-3.5" />
                </button>
              </div>
            </div>
          </div>
        ))}
      </div>

      {/* Generate Keypair Modal */}
      {showGenerateModal && (
        <div className="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
          <div className="bg-zinc-900 border border-zinc-750 rounded-xl max-w-md w-full p-4 space-y-4 shadow-2xl">
            <h4 className="text-sm font-semibold text-zinc-100 flex items-center gap-2">
              <Sparkles className="w-4 h-4 text-emerald-400" />
              <span>Generate New SSH Keypair</span>
            </h4>

            <div className="space-y-3 text-xs">
              <div>
                <label className="text-zinc-400 block mb-1">Key Name / Identifier</label>
                <input
                  type="text"
                  placeholder="e.g. id_ed25519_production"
                  value={genName}
                  onChange={(e) => setGenName(e.target.value)}
                  autoFocus
                  className="w-full px-3 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500"
                />
              </div>

              <div>
                <label className="text-zinc-400 block mb-1">Algorithm</label>
                <div className="grid grid-cols-2 gap-2">
                  <button
                    type="button"
                    onClick={() => setGenType('Ed25519')}
                    className={`p-2 rounded-lg border text-left transition-colors ${
                      genType === 'Ed25519'
                        ? 'bg-emerald-500/20 border-emerald-500 text-emerald-300'
                        : 'bg-zinc-950 border-zinc-800 text-zinc-400'
                    }`}
                  >
                    <span className="font-bold block">Ed25519 (Recommended)</span>
                    <span className="text-[10px] text-zinc-500">Fast, compact, elliptic-curve 256-bit</span>
                  </button>

                  <button
                    type="button"
                    onClick={() => setGenType('RSA 4096')}
                    className={`p-2 rounded-lg border text-left transition-colors ${
                      genType === 'RSA 4096'
                        ? 'bg-blue-500/20 border-blue-500 text-blue-300'
                        : 'bg-zinc-950 border-zinc-800 text-zinc-400'
                    }`}
                  >
                    <span className="font-bold block">RSA 4096</span>
                    <span className="text-[10px] text-zinc-500">Maximum legacy server compatibility</span>
                  </button>
                </div>
              </div>

              <div>
                <label className="text-zinc-400 block mb-1">Key Comment</label>
                <input
                  type="text"
                  value={genComment}
                  onChange={(e) => setGenComment(e.target.value)}
                  className="w-full px-3 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none"
                />
              </div>
            </div>

            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={() => setShowGenerateModal(false)}
                className="px-3 py-1.5 text-xs text-zinc-400 hover:text-zinc-200"
              >
                Cancel
              </button>
              <button
                disabled={isGenerating || !genName.trim()}
                onClick={handleGenerateKey}
                className="px-3.5 py-1.5 bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-white rounded-lg text-xs font-medium flex items-center gap-1.5"
              >
                {isGenerating && <span className="w-3 h-3 border-2 border-white border-t-transparent rounded-full animate-spin" />}
                <span>{isGenerating ? 'Generating...' : 'Generate Keypair'}</span>
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
