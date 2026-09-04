import React, { useState } from 'react';
import {
  Server,
  X,
  KeyRound,
  Shield,
  ShieldCheck,
  Tag,
  Palette,
  Layers,
  ArrowRight,
  HardDrive,
  Globe,
  Terminal,
} from 'lucide-react';
import { ServerHost, SSHKeyItem, VaultCredential } from '../types';
import { OsIcon, OsBadge } from './OsIcon';

interface HostManagerModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSaveHost: (host: ServerHost) => void;
  editingHost?: ServerHost | null;
  allHosts: ServerHost[];
  keys: SSHKeyItem[];
  credentials?: VaultCredential[];
  onOpenVaultModal?: () => void;
}

export const HostManagerModal: React.FC<HostManagerModalProps> = ({
  isOpen,
  onClose,
  onSaveHost,
  editingHost,
  allHosts,
  keys,
  credentials = [],
  onOpenVaultModal,
}) => {
  const [formData, setFormData] = useState<ServerHost>(
    editingHost || {
      id: `host-${Date.now()}`,
      name: '',
      host: '',
      port: 22,
      username: 'root',
      authType: 'password',
      password: '',
      group: 'Production',
      tags: ['Linux', 'SSH'],
      color: '#38bdf8',
      osType: 'ubuntu',
      status: 'online',
      notes: '',
    }
  );

  const [tagInput, setTagInput] = useState('');

  if (!isOpen) return null;

  const colorPalette = [
    '#38bdf8', // Cyan
    '#10b981', // Emerald
    '#8b5cf6', // Purple
    '#f59e0b', // Amber
    '#ef4444', // Red
    '#ec4899', // Pink
    '#64748b', // Slate
  ];

  const handleAddTag = () => {
    if (tagInput.trim() && !formData.tags.includes(tagInput.trim())) {
      setFormData({ ...formData, tags: [...formData.tags, tagInput.trim()] });
      setTagInput('');
    }
  };

  const handleRemoveTag = (tagToRemove: string) => {
    setFormData({
      ...formData,
      tags: formData.tags.filter((t) => t !== tagToRemove),
    });
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!formData.name || !formData.host || !formData.username) return;
    onSaveHost(formData);
    onClose();
  };

  return (
    <div className="fixed inset-0 bg-black/75 backdrop-blur-sm z-50 flex items-center justify-center p-4 select-none">
      <div className="bg-zinc-900 border border-zinc-750 rounded-2xl max-w-xl w-full p-5 shadow-2xl space-y-4 max-h-[90vh] flex flex-col font-sans">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-zinc-800 pb-3">
          <div className="flex items-center gap-2.5">
            <div className="w-8 h-8 rounded-lg bg-cyan-500/10 border border-cyan-500/30 flex items-center justify-center text-cyan-400">
              <Server className="w-4 h-4" />
            </div>
            <div>
              <h3 className="text-sm font-semibold text-zinc-100">
                {editingHost ? 'Edit SSH Server Node' : 'Register New SSH Server Node'}
              </h3>
              <p className="text-[11px] text-zinc-400">
                Configure host connectivity, credentials, proxy bastions, and tags
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

        {/* Form Body */}
        <form onSubmit={handleSubmit} className="flex-1 overflow-y-auto space-y-4 pr-1 text-xs">
          {/* Node Display Name & Color */}
          <div className="grid grid-cols-3 gap-3">
            <div className="col-span-2 space-y-1">
              <label className="text-zinc-300 font-medium">Node Display Name *</label>
              <input
                type="text"
                required
                placeholder="e.g. US-East K8s Master Node"
                value={formData.name}
                onChange={(e) => setFormData({ ...formData, name: e.target.value })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500 font-medium"
              />
            </div>

            <div className="space-y-1">
              <label className="text-zinc-300 font-medium">Group / Folder</label>
              <input
                type="text"
                placeholder="Production"
                value={formData.group}
                onChange={(e) => setFormData({ ...formData, group: e.target.value })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500 font-mono"
              />
            </div>
          </div>

          {/* Host IP & Port */}
          <div className="grid grid-cols-3 gap-3 font-mono">
            <div className="col-span-2 space-y-1">
              <label className="text-zinc-300 font-medium font-sans">Host IP / FQDN *</label>
              <input
                type="text"
                required
                placeholder="192.168.1.100 or ec2.aws.com"
                value={formData.host}
                onChange={(e) => setFormData({ ...formData, host: e.target.value })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500"
              />
            </div>

            <div className="space-y-1">
              <label className="text-zinc-300 font-medium font-sans">Port</label>
              <input
                type="number"
                value={formData.port}
                onChange={(e) => setFormData({ ...formData, port: Number(e.target.value) })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500"
              />
            </div>
          </div>

          {/* Username & Authentication */}
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-zinc-300 font-medium">Login Username *</label>
              <input
                type="text"
                required
                placeholder="root or ubuntu"
                value={formData.username}
                onChange={(e) => setFormData({ ...formData, username: e.target.value })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500 font-mono"
              />
            </div>

            <div className="space-y-1">
              <label className="text-zinc-300 font-medium">Authentication Type</label>
              <select
                value={formData.authType}
                onChange={(e) => setFormData({ ...formData, authType: e.target.value as any })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500"
              >
                <option value="password">Password Authentication</option>
                <option value="key">SSH Keypair (Private Key)</option>
                <option value="agent">SSH Agent Forwarding</option>
              </select>
            </div>
          </div>

          {/* Auth credentials fields */}
          {formData.authType === 'password' ? (
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <label className="text-zinc-300 font-medium text-xs">SSH Password</label>
                {credentials.filter((c) => c.type === 'password').length > 0 && (
                  <div className="flex items-center gap-1">
                    <ShieldCheck className="w-3 h-3 text-cyan-400" />
                    <select
                      aria-label="从保险箱选择"
                      defaultValue=""
                      onChange={(e) => {
                        const sel = credentials.find((c) => c.id === e.target.value);
                        if (sel && sel.password) {
                          setFormData({
                            ...formData,
                            password: sel.password,
                            username: sel.username || formData.username,
                          });
                        }
                      }}
                      className="bg-zinc-900 border border-zinc-700 text-cyan-400 text-[11px] rounded px-1.5 py-0.5 focus:outline-none"
                    >
                      <option value="">从密码保险箱选取...</option>
                      {credentials
                        .filter((c) => c.type === 'password')
                        .map((c) => (
                          <option key={c.id} value={c.id}>
                            {c.name} ({c.username || '无用户名'})
                          </option>
                        ))}
                    </select>
                  </div>
                )}
              </div>
              <input
                type="password"
                placeholder="••••••••••••"
                value={formData.password || ''}
                onChange={(e) => setFormData({ ...formData, password: e.target.value })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500 font-mono"
              />
            </div>
          ) : formData.authType === 'key' ? (
            <div className="space-y-1">
              <div className="flex items-center justify-between">
                <label className="text-zinc-300 font-medium text-xs">Select Saved SSH Key</label>
                {onOpenVaultModal && (
                  <button
                    type="button"
                    onClick={onOpenVaultModal}
                    className="text-cyan-400 hover:underline text-[11px] flex items-center gap-1"
                  >
                    <ShieldCheck className="w-3 h-3" />
                    <span>管理私钥保险箱</span>
                  </button>
                )}
              </div>
              <select
                value={formData.keyId || ''}
                onChange={(e) => setFormData({ ...formData, keyId: e.target.value })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500 font-mono"
              >
                <option value="">-- Choose Key from Vault --</option>
                {/* Regular keys */}
                {keys.map((k) => (
                  <option key={k.id} value={k.id}>
                    {k.name} ({k.type})
                  </option>
                ))}
                {/* Vault private keys */}
                {credentials
                  .filter((c) => c.type === 'privateKey' && !keys.some((k) => k.id === c.id))
                  .map((k) => (
                    <option key={k.id} value={k.id}>
                      [保险箱] {k.name} ({k.keyType || 'SSH-KEY'})
                    </option>
                  ))}
              </select>
            </div>
          ) : null}

          {/* OS Type & Jump Host (ProxyJump Bastion) */}
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <div className="flex items-center justify-between">
                <label className="text-zinc-300 font-medium text-xs">Target OS Platform</label>
                <OsBadge osType={formData.osType} size="sm" />
              </div>
              <select
                value={formData.osType}
                onChange={(e) => setFormData({ ...formData, osType: e.target.value as any })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500"
              >
                <option value="ubuntu">Ubuntu / Debian Linux</option>
                <option value="centos">CentOS / RedHat / Rocky</option>
                <option value="alpine">Alpine Linux</option>
                <option value="arch">Arch Linux</option>
                <option value="macos">macOS Darwin</option>
                <option value="windows">Windows OpenSSH</option>
                <option value="freebsd">FreeBSD / BSD</option>
              </select>
            </div>

            <div className="space-y-1">
              <label className="text-zinc-300 font-medium">ProxyJump Bastion Node</label>
              <select
                value={formData.jumpHostId || ''}
                onChange={(e) => setFormData({ ...formData, jumpHostId: e.target.value || undefined })}
                className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none focus:border-cyan-500"
              >
                <option value="">Direct Connection (No Jump)</option>
                {allHosts
                  .filter((h) => h.id !== formData.id)
                  .map((h) => (
                    <option key={h.id} value={h.id}>
                      Bastion: {h.name} ({h.host})
                    </option>
                  ))}
              </select>
            </div>
          </div>

          {/* Color Accent Picker */}
          <div className="space-y-1.5">
            <label className="text-zinc-300 font-medium">Badge Color Accent</label>
            <div className="flex items-center gap-2">
              {colorPalette.map((color) => (
                <button
                  key={color}
                  type="button"
                  onClick={() => setFormData({ ...formData, color })}
                  className={`w-6 h-6 rounded-full border-2 transition-transform ${
                    formData.color === color ? 'scale-125 border-white' : 'border-transparent'
                  }`}
                  style={{ backgroundColor: color }}
                />
              ))}
            </div>
          </div>

          {/* Tags */}
          <div className="space-y-1.5">
            <label className="text-zinc-300 font-medium">Tags</label>
            <div className="flex items-center gap-2">
              <input
                type="text"
                placeholder="Add tag (e.g. AWS, Production)..."
                value={tagInput}
                onChange={(e) => setTagInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    handleAddTag();
                  }
                }}
                className="flex-1 px-3 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-zinc-100 focus:outline-none"
              />
              <button
                type="button"
                onClick={handleAddTag}
                className="px-3 py-1.5 bg-zinc-800 hover:bg-zinc-700 text-zinc-200 rounded-lg font-medium"
              >
                Add
              </button>
            </div>

            <div className="flex items-center gap-1.5 flex-wrap pt-1">
              {formData.tags.map((t) => (
                <span
                  key={t}
                  className="px-2 py-0.5 rounded-full bg-zinc-800 text-zinc-300 text-[11px] font-mono flex items-center gap-1"
                >
                  <span>#{t}</span>
                  <button
                    type="button"
                    onClick={() => handleRemoveTag(t)}
                    className="hover:text-red-400"
                  >
                    ×
                  </button>
                </span>
              ))}
            </div>
          </div>

          {/* Footer actions */}
          <div className="flex justify-end gap-2 pt-3 border-t border-zinc-800">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 text-zinc-400 hover:text-zinc-200"
            >
              Cancel
            </button>
            <button
              type="submit"
              className="px-5 py-2 bg-cyan-600 hover:bg-cyan-500 text-white rounded-lg font-medium transition-colors shadow-md shadow-cyan-950"
            >
              {editingHost ? 'Save Changes' : 'Register Host Node'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
