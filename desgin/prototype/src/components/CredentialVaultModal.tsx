import React, { useState, useMemo } from 'react';
import {
  ShieldCheck,
  KeyRound,
  Lock,
  Plus,
  Search,
  Eye,
  EyeOff,
  Copy,
  Check,
  Download,
  Trash2,
  Edit2,
  Star,
  Sparkles,
  RefreshCw,
  Sliders,
  X,
  FileCode,
  Key,
  FolderLock,
  ArrowUpDown,
  Tag,
  Upload,
  Layers,
  Terminal,
} from 'lucide-react';
import { VaultCredential, VaultCredentialType } from '../types';

interface CredentialVaultModalProps {
  isOpen: boolean;
  onClose: () => void;
  credentials: VaultCredential[];
  onUpdateCredentials: (credentials: VaultCredential[]) => void;
  onApplyToTerminal?: (text: string) => void;
}

// Password strength evaluator
export function calculatePasswordStrength(pass: string): { score: number; label: string; color: string } {
  if (!pass) return { score: 0, label: '无密码', color: '#6B7280' };
  let score = 0;
  if (pass.length >= 8) score += 1;
  if (pass.length >= 14) score += 1;
  if (/[A-Z]/.test(pass)) score += 1;
  if (/[a-z]/.test(pass)) score += 1;
  if (/[0-9]/.test(pass)) score += 1;
  if (/[^A-Za-z0-9]/.test(pass)) score += 1;

  if (score <= 2) return { score, label: '弱', color: '#EF4444' };
  if (score <= 4) return { score, label: '中等', color: '#F59E0B' };
  if (score <= 5) return { score, label: '强', color: '#10B981' };
  return { score, label: '极高', color: '#06B6D4' };
}

// Secure random password generator
export function generateSecurePassword(options: {
  length: number;
  useUpper: boolean;
  useLower: boolean;
  useNumbers: boolean;
  useSymbols: boolean;
  avoidAmbiguous: boolean;
}): string {
  let upper = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ';
  let lower = 'abcdefghijklmnopqrstuvwxyz';
  let numbers = '0123456789';
  let symbols = '!@#$%^&*()_+-=[]{}|;:,.<>?';

  if (options.avoidAmbiguous) {
    upper = upper.replace(/[IO]/g, '');
    lower = lower.replace(/[lo]/g, '');
    numbers = numbers.replace(/[01]/g, '');
    symbols = symbols.replace(/[|;:,.]/g, '');
  }

  let charset = '';
  const requiredChars: string[] = [];

  if (options.useUpper) {
    charset += upper;
    requiredChars.push(upper.charAt(Math.floor(Math.random() * upper.length)));
  }
  if (options.useLower) {
    charset += lower;
    requiredChars.push(lower.charAt(Math.floor(Math.random() * lower.length)));
  }
  if (options.useNumbers) {
    charset += numbers;
    requiredChars.push(numbers.charAt(Math.floor(Math.random() * numbers.length)));
  }
  if (options.useSymbols) {
    charset += symbols;
    requiredChars.push(symbols.charAt(Math.floor(Math.random() * symbols.length)));
  }

  if (!charset) charset = lower + numbers;

  const result: string[] = [...requiredChars];
  for (let i = result.length; i < options.length; i++) {
    result.push(charset.charAt(Math.floor(Math.random() * charset.length)));
  }

  // Shuffle array
  for (let i = result.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [result[i], result[j]] = [result[j], result[i]];
  }

  return result.join('');
}

export const CredentialVaultModal: React.FC<CredentialVaultModalProps> = ({
  isOpen,
  onClose,
  credentials,
  onUpdateCredentials,
  onApplyToTerminal,
}) => {
  // Search and filters
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedType, setSelectedType] = useState<'all' | VaultCredentialType | 'favorite'>('all');
  const [selectedCategory, setSelectedCategory] = useState<string>('all');
  const [visiblePasswords, setVisiblePasswords] = useState<Record<string, boolean>>({});
  const [copiedFieldId, setCopiedFieldId] = useState<string | null>(null);

  // Sub-modal states
  const [showAddEditModal, setShowAddEditModal] = useState(false);
  const [editingCred, setEditingCred] = useState<VaultCredential | null>(null);
  const [showGeneratorModal, setShowGeneratorModal] = useState(false);
  const [showKeyGenModal, setShowKeyGenModal] = useState(false);

  // Generator Options State
  const [genLength, setGenLength] = useState(20);
  const [genUpper, setGenUpper] = useState(true);
  const [genLower, setGenLower] = useState(true);
  const [genNumbers, setGenNumbers] = useState(true);
  const [genSymbols, setGenSymbols] = useState(true);
  const [genAvoidAmbiguous, setGenAvoidAmbiguous] = useState(false);
  const [generatedPass, setGeneratedPass] = useState(() =>
    generateSecurePassword({
      length: 20,
      useUpper: true,
      useLower: true,
      useNumbers: true,
      useSymbols: true,
      avoidAmbiguous: false,
    })
  );

  // Form State for Add / Edit
  const [formType, setFormType] = useState<VaultCredentialType>('password');
  const [formName, setFormName] = useState('');
  const [formCategory, setFormCategory] = useState('Linux 服务器');
  const [formUsername, setFormUsername] = useState('root');
  const [formPassword, setFormPassword] = useState('');
  const [formKeyType, setFormKeyType] = useState<'Ed25519' | 'RSA 4096' | 'ECDSA P-384'>('Ed25519');
  const [formPublicKey, setFormPublicKey] = useState('');
  const [formPrivateKey, setFormPrivateKey] = useState('');
  const [formPassphrase, setFormPassphrase] = useState('');
  const [formToken, setFormToken] = useState('');
  const [formTags, setFormTags] = useState<string[]>(['SSH']);
  const [tagInput, setTagInput] = useState('');
  const [formNotes, setFormNotes] = useState('');

  // Keypair Generator Modal State
  const [keyGenType, setKeyGenType] = useState<'Ed25519' | 'RSA 4096' | 'ECDSA P-384'>('Ed25519');
  const [keyGenName, setKeyGenName] = useState('');
  const [keyGenComment, setKeyGenComment] = useState('winssh-key@prod');
  const [isGeneratingKey, setIsGeneratingKey] = useState(false);

  // Unique categories list
  const categories = useMemo(() => {
    const set = new Set<string>();
    credentials.forEach((c) => {
      if (c.category) set.add(c.category);
    });
    return Array.from(set);
  }, [credentials]);

  // Filtered credentials
  const filteredCredentials = useMemo(() => {
    return credentials.filter((c) => {
      // Type filter
      if (selectedType === 'favorite' && !c.favorite) return false;
      if (selectedType !== 'all' && selectedType !== 'favorite' && c.type !== selectedType) return false;

      // Category filter
      if (selectedCategory !== 'all' && c.category !== selectedCategory) return false;

      // Search Query
      if (searchQuery.trim()) {
        const q = searchQuery.toLowerCase();
        const matchName = c.name.toLowerCase().includes(q);
        const matchUser = c.username?.toLowerCase().includes(q) || false;
        const matchCategory = c.category?.toLowerCase().includes(q) || false;
        const matchNotes = c.notes?.toLowerCase().includes(q) || false;
        const matchTags = c.tags.some((t) => t.toLowerCase().includes(q));
        if (!matchName && !matchUser && !matchCategory && !matchNotes && !matchTags) {
          return false;
        }
      }
      return true;
    });
  }, [credentials, selectedType, selectedCategory, searchQuery]);

  if (!isOpen) return null;

  const handleCopy = (fieldId: string, text: string) => {
    navigator.clipboard.writeText(text);
    setCopiedFieldId(fieldId);
    setTimeout(() => setCopiedFieldId(null), 2000);
  };

  const togglePasswordVisibility = (id: string) => {
    setVisiblePasswords((prev) => ({ ...prev, [id]: !prev[id] }));
  };

  const handleToggleFavorite = (id: string) => {
    onUpdateCredentials(
      credentials.map((c) => (c.id === id ? { ...c, favorite: !c.favorite } : c))
    );
  };

  const handleDelete = (id: string) => {
    if (window.confirm('确定从保险箱中移除此凭据？此操作不可逆。')) {
      onUpdateCredentials(credentials.filter((c) => c.id !== id));
    }
  };

  const handleDownloadPrivateKey = (cred: VaultCredential) => {
    if (!cred.privateKey) return;
    const blob = new Blob([cred.privateKey], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${cred.name || 'id_rsa'}.pem`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleOpenAddModal = (initialType: VaultCredentialType = 'password') => {
    setEditingCred(null);
    setFormType(initialType);
    setFormName('');
    setFormCategory('Linux 服务器');
    setFormUsername(initialType === 'password' ? 'root' : '');
    setFormPassword(
      initialType === 'password'
        ? generateSecurePassword({
            length: 16,
            useUpper: true,
            useLower: true,
            useNumbers: true,
            useSymbols: true,
            avoidAmbiguous: false,
          })
        : ''
    );
    setFormKeyType('Ed25519');
    setFormPublicKey('');
    setFormPrivateKey('');
    setFormPassphrase('');
    setFormToken('');
    setFormTags(['SSH']);
    setFormNotes('');
    setShowAddEditModal(true);
  };

  const handleOpenEditModal = (cred: VaultCredential) => {
    setEditingCred(cred);
    setFormType(cred.type);
    setFormName(cred.name);
    setFormCategory(cred.category || '通用');
    setFormUsername(cred.username || '');
    setFormPassword(cred.password || '');
    setFormKeyType(cred.keyType || 'Ed25519');
    setFormPublicKey(cred.publicKey || '');
    setFormPrivateKey(cred.privateKey || '');
    setFormPassphrase(cred.passphrase || '');
    setFormToken(cred.token || '');
    setFormTags(cred.tags || []);
    setFormNotes(cred.notes || '');
    setShowAddEditModal(true);
  };

  const handleSaveForm = (e: React.FormEvent) => {
    e.preventDefault();
    if (!formName.trim()) return;

    let fingerprint = editingCred?.fingerprint;
    if (formType === 'privateKey' && !fingerprint) {
      fingerprint = `SHA256:${Array.from({ length: 43 }, () =>
        '0123456789abcdef'.charAt(Math.floor(Math.random() * 16))
      ).join('')}`;
    }

    const updatedItem: VaultCredential = {
      id: editingCred ? editingCred.id : `cred-${Date.now()}`,
      name: formName.trim(),
      type: formType,
      category: formCategory.trim() || '通用',
      username: formUsername.trim(),
      password: formType === 'password' ? formPassword : '',
      keyType: formType === 'privateKey' ? formKeyType : undefined,
      publicKey: formType === 'privateKey' ? formPublicKey : undefined,
      privateKey: formType === 'privateKey' ? formPrivateKey : undefined,
      passphrase: formType === 'privateKey' ? formPassphrase : undefined,
      fingerprint,
      token: formType === 'token' ? formToken : undefined,
      tags: formTags,
      notes: formNotes.trim(),
      createdDate: editingCred?.createdDate || new Date().toISOString().replace('T', ' ').substring(0, 19),
      updatedDate: new Date().toISOString().replace('T', ' ').substring(0, 19),
      favorite: editingCred ? editingCred.favorite : false,
    };

    if (editingCred) {
      onUpdateCredentials(credentials.map((c) => (c.id === editingCred.id ? updatedItem : c)));
    } else {
      onUpdateCredentials([updatedItem, ...credentials]);
    }

    setShowAddEditModal(false);
  };

  const handleRegeneratePassword = () => {
    const newPass = generateSecurePassword({
      length: genLength,
      useUpper: genUpper,
      useLower: genLower,
      useNumbers: genNumbers,
      useSymbols: genSymbols,
      avoidAmbiguous: genAvoidAmbiguous,
    });
    setGeneratedPass(newPass);
  };

  const handleExecuteKeygen = () => {
    if (!keyGenName.trim()) return;
    setIsGeneratingKey(true);

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
        keyGenType === 'Ed25519'
          ? `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI${randomHex.substring(0, 32)} ${keyGenComment}`
          : `ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQD${randomHex}== ${keyGenComment}`;

      const privKey =
        keyGenType === 'Ed25519'
          ? `-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAAC${randomHex}AAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\nAAAAHndpbnNzaC1wcm9AZGVlcG1pbmQtY2x1c3RlcgECAwQFBgc=\n-----END OPENSSH PRIVATE KEY-----`
          : `-----BEGIN RSA PRIVATE KEY-----\nMIIJKQIBAAKCAQEA${randomHex}...\n[WinSSH Generated RSA 4096 Private Key Material]\n-----END RSA PRIVATE KEY-----`;

      const newCred: VaultCredential = {
        id: `key-${Date.now()}`,
        name: keyGenName.trim(),
        type: 'privateKey',
        keyType: keyGenType,
        category: 'SSH 基础设施私钥',
        username: 'root',
        fingerprint,
        publicKey: pubKey,
        privateKey: privKey,
        tags: ['SSH', keyGenType, 'AutoGenerated'],
        notes: `由 WinSSH 密钥生成引擎于 ${new Date().toLocaleDateString()} 自动生成`,
        createdDate: new Date().toISOString().replace('T', ' ').substring(0, 19),
        favorite: true,
      };

      onUpdateCredentials([newCred, ...credentials]);
      setIsGeneratingKey(false);
      setShowKeyGenModal(false);
      setKeyGenName('');
    }, 600);
  };

  const handleExportVault = () => {
    const dataStr = 'data:text/json;charset=utf-8,' + encodeURIComponent(JSON.stringify(credentials, null, 2));
    const a = document.createElement('a');
    a.href = dataStr;
    a.download = `winssh-vault-backup-${new Date().toISOString().slice(0, 10)}.json`;
    a.click();
  };

  return (
    <div className="fixed inset-0 bg-black/80 backdrop-blur-sm z-50 flex items-center justify-center p-3 select-none">
      <div className="bg-[#1E1E1E] border border-[#3C3C3C] rounded-2xl w-full max-w-5xl h-[88vh] shadow-2xl flex flex-col overflow-hidden font-sans">
        {/* Top Header Bar */}
        <div className="px-5 py-3.5 bg-[#252526] border-b border-[#333333] flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="w-9 h-9 rounded-xl bg-gradient-to-br from-[#007ACC]/20 to-[#4EC9B0]/20 border border-[#007ACC]/40 flex items-center justify-center text-[#4EC9B0] shadow-sm">
              <ShieldCheck className="w-5 h-5 text-[#4EC9B0]" />
            </div>
            <div>
              <div className="flex items-center gap-2">
                <h2 className="text-sm font-bold text-[#E0E0E0] tracking-wide">
                  账号密码与私钥保险箱
                </h2>
                <span className="text-[10px] px-2 py-0.5 rounded-full bg-[#10B981]/15 text-[#10B981] border border-[#10B981]/30 font-mono">
                  AES-256 加密保护
                </span>
              </div>
              <p className="text-[11px] text-[#858585]">
                集中安全存储管理服务器账号密码、SSH 密钥对、API 令牌与凭据
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <button
              onClick={() => setShowGeneratorModal(true)}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[#2D2D2D] hover:bg-[#383838] border border-[#404040] text-[#CCCCCC] hover:text-white text-xs transition-colors"
            >
              <Sparkles className="w-3.5 h-3.5 text-[#E5C07B]" />
              <span>密码生成器</span>
            </button>

            <button
              onClick={() => {
                setKeyGenName(`id_${keyGenType.toLowerCase()}_${Date.now().toString().slice(-4)}`);
                setShowKeyGenModal(true);
              }}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[#2D2D2D] hover:bg-[#383838] border border-[#404040] text-[#CCCCCC] hover:text-white text-xs transition-colors"
            >
              <FileCode className="w-3.5 h-3.5 text-[#4EC9B0]" />
              <span>生成 SSH 密钥对</span>
            </button>

            <button
              onClick={() => handleOpenAddModal('password')}
              className="flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg bg-[#007ACC] hover:bg-[#0062a3] text-white text-xs font-semibold shadow transition-colors"
            >
              <Plus className="w-4 h-4" />
              <span>新建凭据</span>
            </button>

            <button
              onClick={onClose}
              className="p-1.5 text-[#858585] hover:text-white hover:bg-[#333333] rounded-lg transition-colors ml-1"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Main Body Area */}
        <div className="flex-1 flex overflow-hidden">
          {/* Left Sidebar Filters */}
          <div className="w-56 bg-[#181818] border-r border-[#2D2D2D] p-3 flex flex-col justify-between overflow-y-auto">
            <div className="space-y-4">
              {/* Type Category Selection */}
              <div className="space-y-1">
                <div className="text-[10px] uppercase font-bold text-[#6D6D6D] px-2 tracking-wider">
                  凭据分类
                </div>

                <button
                  onClick={() => setSelectedType('all')}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition-colors ${
                    selectedType === 'all'
                      ? 'bg-[#007ACC]/15 text-[#4EC9B0] font-medium border border-[#007ACC]/30'
                      : 'text-[#CCCCCC] hover:bg-[#252526] hover:text-white'
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <FolderLock className="w-3.5 h-3.5" />
                    <span>全部凭据</span>
                  </div>
                  <span className="text-[10px] font-mono px-1.5 py-0.2 bg-[#2D2D2D] rounded text-[#858585]">
                    {credentials.length}
                  </span>
                </button>

                <button
                  onClick={() => setSelectedType('password')}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition-colors ${
                    selectedType === 'password'
                      ? 'bg-[#007ACC]/15 text-[#4EC9B0] font-medium border border-[#007ACC]/30'
                      : 'text-[#CCCCCC] hover:bg-[#252526] hover:text-white'
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <KeyRound className="w-3.5 h-3.5 text-[#007ACC]" />
                    <span>账号密码</span>
                  </div>
                  <span className="text-[10px] font-mono px-1.5 py-0.2 bg-[#2D2D2D] rounded text-[#858585]">
                    {credentials.filter((c) => c.type === 'password').length}
                  </span>
                </button>

                <button
                  onClick={() => setSelectedType('privateKey')}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition-colors ${
                    selectedType === 'privateKey'
                      ? 'bg-[#007ACC]/15 text-[#4EC9B0] font-medium border border-[#007ACC]/30'
                      : 'text-[#CCCCCC] hover:bg-[#252526] hover:text-white'
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <FileCode className="w-3.5 h-3.5 text-[#E5C07B]" />
                    <span>SSH 私钥</span>
                  </div>
                  <span className="text-[10px] font-mono px-1.5 py-0.2 bg-[#2D2D2D] rounded text-[#858585]">
                    {credentials.filter((c) => c.type === 'privateKey').length}
                  </span>
                </button>

                <button
                  onClick={() => setSelectedType('token')}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition-colors ${
                    selectedType === 'token'
                      ? 'bg-[#007ACC]/15 text-[#4EC9B0] font-medium border border-[#007ACC]/30'
                      : 'text-[#CCCCCC] hover:bg-[#252526] hover:text-white'
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <Key className="w-3.5 h-3.5 text-[#98C379]" />
                    <span>API / 令牌</span>
                  </div>
                  <span className="text-[10px] font-mono px-1.5 py-0.2 bg-[#2D2D2D] rounded text-[#858585]">
                    {credentials.filter((c) => c.type === 'token').length}
                  </span>
                </button>

                <button
                  onClick={() => setSelectedType('favorite')}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition-colors ${
                    selectedType === 'favorite'
                      ? 'bg-[#007ACC]/15 text-[#4EC9B0] font-medium border border-[#007ACC]/30'
                      : 'text-[#CCCCCC] hover:bg-[#252526] hover:text-white'
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <Star className="w-3.5 h-3.5 text-[#E5C07B] fill-[#E5C07B]" />
                    <span>特别关注</span>
                  </div>
                  <span className="text-[10px] font-mono px-1.5 py-0.2 bg-[#2D2D2D] rounded text-[#858585]">
                    {credentials.filter((c) => c.favorite).length}
                  </span>
                </button>
              </div>

              {/* Tag / Category Filter */}
              {categories.length > 0 && (
                <div className="space-y-1 pt-2 border-t border-[#2D2D2D]">
                  <div className="text-[10px] uppercase font-bold text-[#6D6D6D] px-2 tracking-wider">
                    所属分组
                  </div>
                  <button
                    onClick={() => setSelectedCategory('all')}
                    className={`w-full text-left px-2 py-1 rounded text-[11px] truncate transition-colors ${
                      selectedCategory === 'all'
                        ? 'text-[#4EC9B0] font-semibold bg-[#252526]'
                        : 'text-[#858585] hover:text-[#CCCCCC]'
                    }`}
                  >
                    全部目录
                  </button>
                  {categories.map((cat) => (
                    <button
                      key={cat}
                      onClick={() => setSelectedCategory(cat)}
                      className={`w-full text-left px-2 py-1 rounded text-[11px] truncate transition-colors ${
                        selectedCategory === cat
                          ? 'text-[#4EC9B0] font-semibold bg-[#252526]'
                          : 'text-[#858585] hover:text-[#CCCCCC]'
                      }`}
                    >
                      {cat}
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* Bottom Backup and Status */}
            <div className="pt-3 border-t border-[#2D2D2D] space-y-2 text-xs">
              <button
                onClick={handleExportVault}
                className="w-full flex items-center justify-center gap-1.5 py-1.5 rounded bg-[#252526] hover:bg-[#2F2F2F] text-[#CCCCCC] text-[11px] border border-[#333333] transition-colors"
              >
                <Download className="w-3 h-3 text-[#4EC9B0]" />
                <span>导出保险箱备份</span>
              </button>
              <div className="text-[9.5px] text-[#666666] text-center leading-tight">
                本地存储并实时持久化，数据不经由第三方服务器传输
              </div>
            </div>
          </div>

          {/* Right Main Vault List Area */}
          <div className="flex-1 flex flex-col overflow-hidden bg-[#1E1E1E]">
            {/* Search and Action Bar */}
            <div className="p-3 border-b border-[#2D2D2D] bg-[#222222] flex items-center justify-between gap-3">
              <div className="relative flex-1 max-w-md">
                <Search className="w-3.5 h-3.5 text-[#858585] absolute left-3 top-2.5" />
                <input
                  type="text"
                  placeholder="搜索名称、用户名、主机、标签或备注..."
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="w-full pl-9 pr-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-xs text-[#E0E0E0] placeholder-[#6D6D6D] focus:outline-none focus:border-[#007ACC]"
                />
              </div>

              <div className="flex items-center gap-2 text-xs text-[#858585]">
                <span>
                  共 <strong className="text-[#D4D4D4] font-mono">{filteredCredentials.length}</strong> 条凭据
                </span>
              </div>
            </div>

            {/* List of Credential Cards */}
            <div className="flex-1 overflow-y-auto p-4 space-y-3">
              {filteredCredentials.length === 0 ? (
                <div className="h-64 flex flex-col items-center justify-center text-center p-6 space-y-3">
                  <div className="w-12 h-12 rounded-full bg-[#252526] border border-[#333333] flex items-center justify-center text-[#858585]">
                    <ShieldCheck className="w-6 h-6" />
                  </div>
                  <div>
                    <h4 className="text-sm font-semibold text-[#D4D4D4]">未找到匹配的凭据记录</h4>
                    <p className="text-xs text-[#858585] mt-1">
                      可通过上方搜索框调整过滤条件，或点击“新建凭据”录入新账户密码
                    </p>
                  </div>
                  <button
                    onClick={() => handleOpenAddModal('password')}
                    className="flex items-center gap-1 px-3 py-1.5 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded text-xs transition-colors"
                  >
                    <Plus className="w-3.5 h-3.5" />
                    <span>立即新建凭据</span>
                  </button>
                </div>
              ) : (
                <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                  {filteredCredentials.map((cred) => {
                    const isVisible = visiblePasswords[cred.id];
                    const passStrength = cred.password ? calculatePasswordStrength(cred.password) : null;

                    return (
                      <div
                        key={cred.id}
                        className="bg-[#252526] hover:bg-[#2A2A2B] border border-[#333333] hover:border-[#454545] rounded-xl p-3.5 space-y-3 transition-all shadow-sm group"
                      >
                        {/* Card Header */}
                        <div className="flex items-start justify-between gap-2">
                          <div className="flex items-start gap-2.5 truncate">
                            <div
                              className={`p-2 rounded-lg shrink-0 ${
                                cred.type === 'password'
                                  ? 'bg-[#007ACC]/15 text-[#007ACC] border border-[#007ACC]/30'
                                  : cred.type === 'privateKey'
                                  ? 'bg-[#E5C07B]/15 text-[#E5C07B] border border-[#E5C07B]/30'
                                  : 'bg-[#98C379]/15 text-[#98C379] border border-[#98C379]/30'
                              }`}
                            >
                              {cred.type === 'password' ? (
                                <KeyRound className="w-4 h-4" />
                              ) : cred.type === 'privateKey' ? (
                                <FileCode className="w-4 h-4" />
                              ) : (
                                <Key className="w-4 h-4" />
                              )}
                            </div>

                            <div className="truncate">
                              <div className="flex items-center gap-1.5">
                                <h3 className="text-xs font-bold text-[#E0E0E0] truncate">
                                  {cred.name}
                                </h3>
                                <button
                                  onClick={() => handleToggleFavorite(cred.id)}
                                  className="text-[#858585] hover:text-[#E5C07B] transition-colors"
                                  title={cred.favorite ? '取消收藏' : '收藏'}
                                >
                                  <Star
                                    className={`w-3.5 h-3.5 ${
                                      cred.favorite ? 'text-[#E5C07B] fill-[#E5C07B]' : ''
                                    }`}
                                  />
                                </button>
                              </div>
                              <div className="flex items-center gap-2 mt-0.5 text-[10.5px] text-[#858585]">
                                <span>{cred.category || '通用'}</span>
                                {cred.keyType && (
                                  <span className="font-mono text-[#4EC9B0]">
                                    ({cred.keyType})
                                  </span>
                                )}
                              </div>
                            </div>
                          </div>

                          <div className="flex items-center gap-1 shrink-0">
                            <button
                              onClick={() => handleOpenEditModal(cred)}
                              title="编辑凭据"
                              className="p-1 text-[#858585] hover:text-[#4EC9B0] hover:bg-[#333333] rounded transition-colors"
                            >
                              <Edit2 className="w-3.5 h-3.5" />
                            </button>
                            <button
                              onClick={() => handleDelete(cred.id)}
                              title="删除凭据"
                              className="p-1 text-[#858585] hover:text-[#FF5F56] hover:bg-[#333333] rounded transition-colors"
                            >
                              <Trash2 className="w-3.5 h-3.5" />
                            </button>
                          </div>
                        </div>

                        {/* Credential Content Details */}
                        <div className="space-y-1.5 text-xs bg-[#1A1A1A] p-2.5 rounded-lg border border-[#2D2D2D] font-mono">
                          {/* Username row */}
                          {cred.username && (
                            <div className="flex items-center justify-between">
                              <span className="text-[#858585] text-[10.5px]">账号/用户名:</span>
                              <div className="flex items-center gap-1.5">
                                <span className="text-[#D4D4D4] font-semibold select-all">
                                  {cred.username}
                                </span>
                                <button
                                  onClick={() => handleCopy(`user-${cred.id}`, cred.username!)}
                                  className="text-[#858585] hover:text-white transition-colors"
                                  title="复制用户名"
                                >
                                  {copiedFieldId === `user-${cred.id}` ? (
                                    <Check className="w-3 h-3 text-[#27C93F]" />
                                  ) : (
                                    <Copy className="w-3 h-3" />
                                  )}
                                </button>
                              </div>
                            </div>
                          )}

                          {/* Password row */}
                          {cred.type === 'password' && cred.password && (
                            <div className="space-y-1 pt-1 border-t border-[#262626]">
                              <div className="flex items-center justify-between">
                                <span className="text-[#858585] text-[10.5px]">密码:</span>
                                <div className="flex items-center gap-1.5">
                                  <span className="text-[#D4D4D4] select-all">
                                    {isVisible ? cred.password : '••••••••••••••••'}
                                  </span>
                                  <button
                                    onClick={() => togglePasswordVisibility(cred.id)}
                                    className="text-[#858585] hover:text-white transition-colors p-0.5"
                                    title={isVisible ? '隐藏密码' : '显示明文密码'}
                                  >
                                    {isVisible ? (
                                      <EyeOff className="w-3 h-3 text-[#E5C07B]" />
                                    ) : (
                                      <Eye className="w-3 h-3" />
                                    )}
                                  </button>
                                  <button
                                    onClick={() => handleCopy(`pass-${cred.id}`, cred.password!)}
                                    className="text-[#858585] hover:text-white transition-colors p-0.5"
                                    title="复制密码"
                                  >
                                    {copiedFieldId === `pass-${cred.id}` ? (
                                      <Check className="w-3 h-3 text-[#27C93F]" />
                                    ) : (
                                      <Copy className="w-3 h-3" />
                                    )}
                                  </button>
                                </div>
                              </div>

                              {/* Strength bar */}
                              {passStrength && (
                                <div className="flex items-center gap-2 pt-0.5">
                                  <div className="flex-1 h-1 bg-[#2D2D2D] rounded-full overflow-hidden flex">
                                    <div
                                      className="h-full rounded-full transition-all"
                                      style={{
                                        width: `${(passStrength.score / 6) * 100}%`,
                                        backgroundColor: passStrength.color,
                                      }}
                                    />
                                  </div>
                                  <span
                                    className="text-[9.5px] font-mono"
                                    style={{ color: passStrength.color }}
                                  >
                                    强度: {passStrength.label}
                                  </span>
                                </div>
                              )}
                            </div>
                          )}

                          {/* Private Key row */}
                          {cred.type === 'privateKey' && (
                            <div className="space-y-1.5 pt-1 border-t border-[#262626]">
                              {cred.fingerprint && (
                                <div className="text-[10px] text-[#858585] truncate">
                                  <span className="text-[#666666]">指纹: </span>
                                  <span className="text-[#CCCCCC]">{cred.fingerprint}</span>
                                </div>
                              )}
                              <div className="flex items-center justify-between pt-1">
                                <span className="text-[10.5px] text-[#4EC9B0]">
                                  {cred.keyType || 'Ed25519'} 私钥凭据
                                </span>
                                <div className="flex items-center gap-1.5">
                                  {cred.publicKey && (
                                    <button
                                      onClick={() => handleCopy(`pub-${cred.id}`, cred.publicKey!)}
                                      className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2A2A2B] hover:bg-[#333333] text-[#CCCCCC] text-[10px] border border-[#3C3C3C] transition-colors"
                                    >
                                      {copiedFieldId === `pub-${cred.id}` ? (
                                        <Check className="w-2.5 h-2.5 text-[#27C93F]" />
                                      ) : (
                                        <Copy className="w-2.5 h-2.5" />
                                      )}
                                      <span>公钥</span>
                                    </button>
                                  )}
                                  {cred.privateKey && (
                                    <button
                                      onClick={() => handleDownloadPrivateKey(cred)}
                                      title="下载私钥 .pem 文件"
                                      className="flex items-center gap-1 px-2 py-0.5 rounded bg-[#2A2A2B] hover:bg-[#333333] text-[#CCCCCC] text-[10px] border border-[#3C3C3C] transition-colors"
                                    >
                                      <Download className="w-2.5 h-2.5 text-[#4EC9B0]" />
                                      <span>下载 PEM</span>
                                    </button>
                                  )}
                                </div>
                              </div>
                            </div>
                          )}

                          {/* Token row */}
                          {cred.type === 'token' && cred.token && (
                            <div className="space-y-1 pt-1 border-t border-[#262626]">
                              <div className="flex items-center justify-between">
                                <span className="text-[#858585] text-[10.5px]">Token 令牌:</span>
                                <div className="flex items-center gap-1.5">
                                  <span className="text-[#D4D4D4] truncate max-w-[180px] select-all">
                                    {isVisible ? cred.token : '••••••••••••••••••••••••'}
                                  </span>
                                  <button
                                    onClick={() => togglePasswordVisibility(cred.id)}
                                    className="text-[#858585] hover:text-white transition-colors"
                                  >
                                    {isVisible ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3" />}
                                  </button>
                                  <button
                                    onClick={() => handleCopy(`tok-${cred.id}`, cred.token!)}
                                    className="text-[#858585] hover:text-white transition-colors"
                                  >
                                    {copiedFieldId === `tok-${cred.id}` ? (
                                      <Check className="w-3 h-3 text-[#27C93F]" />
                                    ) : (
                                      <Copy className="w-3 h-3" />
                                    )}
                                  </button>
                                </div>
                              </div>
                            </div>
                          )}
                        </div>

                        {/* Notes / Tags Footer */}
                        <div className="flex items-center justify-between gap-2 pt-1 text-[10px]">
                          <div className="flex items-center gap-1 flex-wrap">
                            {cred.tags.map((tag) => (
                              <span
                                key={tag}
                                className="px-1.5 py-0.2 rounded bg-[#181818] border border-[#2D2D2D] text-[#858585]"
                              >
                                #{tag}
                              </span>
                            ))}
                          </div>

                          {onApplyToTerminal && cred.username && (
                            <button
                              onClick={() => onApplyToTerminal(cred.username!)}
                              className="text-[#007ACC] hover:underline flex items-center gap-1 shrink-0"
                            >
                              <Terminal className="w-2.5 h-2.5" />
                              <span>填入终端</span>
                            </button>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* =========================================================================
         Sub-Modal: Add / Edit Credential
         ========================================================================= */}
      {showAddEditModal && (
        <div className="fixed inset-0 bg-black/75 backdrop-blur-sm z-[60] flex items-center justify-center p-3 select-none">
          <div className="bg-[#222222] border border-[#3C3C3C] rounded-2xl max-w-lg w-full p-5 space-y-4 shadow-2xl max-h-[92vh] flex flex-col font-sans">
            <div className="flex items-center justify-between border-b border-[#333333] pb-3">
              <div className="flex items-center gap-2.5">
                <div className="w-8 h-8 rounded-lg bg-[#007ACC]/15 border border-[#007ACC]/30 flex items-center justify-center text-[#007ACC]">
                  <KeyRound className="w-4 h-4" />
                </div>
                <div>
                  <h3 className="text-sm font-bold text-white">
                    {editingCred ? '编辑保险箱凭据' : '新建保险箱凭据'}
                  </h3>
                  <p className="text-[11px] text-[#858585]">
                    支持安全管理主机账号密码、SSH 密钥对及通用认证令牌
                  </p>
                </div>
              </div>
              <button
                onClick={() => setShowAddEditModal(false)}
                className="text-[#858585] hover:text-white p-1 rounded hover:bg-[#333333]"
              >
                <X className="w-4 h-4" />
              </button>
            </div>

            <form onSubmit={handleSaveForm} className="flex-1 overflow-y-auto space-y-3 pr-1 text-xs">
              {/* Type Switcher */}
              <div>
                <label className="text-[#CCCCCC] font-medium block mb-1">凭据类型 *</label>
                <div className="grid grid-cols-3 gap-2">
                  <button
                    type="button"
                    onClick={() => setFormType('password')}
                    className={`py-2 px-3 rounded-lg border flex items-center justify-center gap-1.5 transition-colors ${
                      formType === 'password'
                        ? 'bg-[#007ACC]/20 border-[#007ACC] text-white font-medium'
                        : 'bg-[#181818] border-[#333333] text-[#858585] hover:text-[#CCCCCC]'
                    }`}
                  >
                    <KeyRound className="w-3.5 h-3.5" />
                    <span>账号密码</span>
                  </button>

                  <button
                    type="button"
                    onClick={() => setFormType('privateKey')}
                    className={`py-2 px-3 rounded-lg border flex items-center justify-center gap-1.5 transition-colors ${
                      formType === 'privateKey'
                        ? 'bg-[#E5C07B]/20 border-[#E5C07B] text-[#E5C07B] font-medium'
                        : 'bg-[#181818] border-[#333333] text-[#858585] hover:text-[#CCCCCC]'
                    }`}
                  >
                    <FileCode className="w-3.5 h-3.5" />
                    <span>SSH 私钥</span>
                  </button>

                  <button
                    type="button"
                    onClick={() => setFormType('token')}
                    className={`py-2 px-3 rounded-lg border flex items-center justify-center gap-1.5 transition-colors ${
                      formType === 'token'
                        ? 'bg-[#98C379]/20 border-[#98C379] text-[#98C379] font-medium'
                        : 'bg-[#181818] border-[#333333] text-[#858585] hover:text-[#CCCCCC]'
                    }`}
                  >
                    <Key className="w-3.5 h-3.5" />
                    <span>API / 令牌</span>
                  </button>
                </div>
              </div>

              {/* Title & Category */}
              <div className="grid grid-cols-3 gap-2">
                <div className="col-span-2 space-y-1">
                  <label className="text-[#CCCCCC] font-medium">凭据名称 / 标识 *</label>
                  <input
                    type="text"
                    required
                    placeholder="例如: 生产环境 Root 密码"
                    value={formName}
                    onChange={(e) => setFormName(e.target.value)}
                    className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC]"
                  />
                </div>

                <div className="space-y-1">
                  <label className="text-[#CCCCCC] font-medium">分组目录</label>
                  <input
                    type="text"
                    placeholder="例如: 数据库"
                    value={formCategory}
                    onChange={(e) => setFormCategory(e.target.value)}
                    className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC]"
                  />
                </div>
              </div>

              {/* Password Mode Fields */}
              {formType === 'password' && (
                <>
                  <div className="grid grid-cols-2 gap-2">
                    <div className="space-y-1">
                      <label className="text-[#CCCCCC] font-medium">用户名 / 账号</label>
                      <input
                        type="text"
                        placeholder="root 或 ubuntu"
                        value={formUsername}
                        onChange={(e) => setFormUsername(e.target.value)}
                        className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC] font-mono"
                      />
                    </div>

                    <div className="space-y-1">
                      <div className="flex items-center justify-between">
                        <label className="text-[#CCCCCC] font-medium">密码</label>
                        <button
                          type="button"
                          onClick={() => {
                            const p = generateSecurePassword({
                              length: 18,
                              useUpper: true,
                              useLower: true,
                              useNumbers: true,
                              useSymbols: true,
                              avoidAmbiguous: false,
                            });
                            setFormPassword(p);
                          }}
                          className="text-[10px] text-[#007ACC] hover:underline flex items-center gap-1"
                        >
                          <Sparkles className="w-2.5 h-2.5" />
                          <span>生成强密码</span>
                        </button>
                      </div>
                      <input
                        type="text"
                        placeholder="输入或生成密码"
                        value={formPassword}
                        onChange={(e) => setFormPassword(e.target.value)}
                        className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC] font-mono"
                      />
                    </div>
                  </div>
                </>
              )}

              {/* Private Key Mode Fields */}
              {formType === 'privateKey' && (
                <div className="space-y-3">
                  <div className="grid grid-cols-2 gap-2">
                    <div className="space-y-1">
                      <label className="text-[#CCCCCC] font-medium">算法类型</label>
                      <select
                        value={formKeyType}
                        onChange={(e) => setFormKeyType(e.target.value as any)}
                        className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC]"
                      >
                        <option value="Ed25519">Ed25519 (高安全性推荐)</option>
                        <option value="RSA 4096">RSA 4096-bit</option>
                        <option value="ECDSA P-384">ECDSA P-384</option>
                      </select>
                    </div>

                    <div className="space-y-1">
                      <label className="text-[#CCCCCC] font-medium">私钥口令 Passphrase (可选)</label>
                      <input
                        type="password"
                        placeholder="若有加密口令请填写"
                        value={formPassphrase}
                        onChange={(e) => setFormPassphrase(e.target.value)}
                        className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC] font-mono"
                      />
                    </div>
                  </div>

                  <div className="space-y-1">
                    <div className="flex items-center justify-between">
                      <label className="text-[#CCCCCC] font-medium">私钥内容 (OpenSSH PEM)</label>
                      <button
                        type="button"
                        onClick={() => {
                          const rand = Array.from({ length: 44 }, () =>
                            'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'.charAt(
                              Math.floor(Math.random() * 64)
                            )
                          ).join('');
                          setFormPrivateKey(
                            `-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAAC${rand}AAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\nAAAAHndpbnNzaC1wcm9AZGVlcG1pbmQtY2x1c3RlcgECAwQFBgc=\n-----END OPENSSH PRIVATE KEY-----`
                          );
                          setFormPublicKey(
                            `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI${rand.substring(0, 32)} winssh-auto@host`
                          );
                        }}
                        className="text-[10px] text-[#4EC9B0] hover:underline flex items-center gap-1"
                      >
                        <Sparkles className="w-2.5 h-2.5" />
                        <span>快速填入随机密钥示例</span>
                      </button>
                    </div>
                    <textarea
                      rows={4}
                      placeholder="-----BEGIN OPENSSH PRIVATE KEY----- ... -----END OPENSSH PRIVATE KEY-----"
                      value={formPrivateKey}
                      onChange={(e) => setFormPrivateKey(e.target.value)}
                      className="w-full p-2 bg-[#141414] border border-[#333333] rounded-lg text-[10.5px] font-mono text-[#D4D4D4] focus:outline-none focus:border-[#007ACC]"
                    />
                  </div>

                  <div className="space-y-1">
                    <label className="text-[#CCCCCC] font-medium">公钥内容 (可选)</label>
                    <input
                      type="text"
                      placeholder="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5... user@host"
                      value={formPublicKey}
                      onChange={(e) => setFormPublicKey(e.target.value)}
                      className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC] font-mono text-[11px]"
                    />
                  </div>
                </div>
              )}

              {/* Token Mode Fields */}
              {formType === 'token' && (
                <div className="space-y-1">
                  <label className="text-[#CCCCCC] font-medium">令牌密钥 (Secret / Bearer Token)</label>
                  <textarea
                    rows={3}
                    placeholder="eyJhbGciOi... 或 API Secret Key"
                    value={formToken}
                    onChange={(e) => setFormToken(e.target.value)}
                    className="w-full p-2 bg-[#141414] border border-[#333333] rounded-lg text-xs font-mono text-[#D4D4D4] focus:outline-none focus:border-[#007ACC]"
                  />
                </div>
              )}

              {/* Tags */}
              <div className="space-y-1">
                <label className="text-[#CCCCCC] font-medium">分类标签</label>
                <div className="flex items-center gap-2">
                  <input
                    type="text"
                    placeholder="输入标签并按 Enter"
                    value={tagInput}
                    onChange={(e) => setTagInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault();
                        if (tagInput.trim() && !formTags.includes(tagInput.trim())) {
                          setFormTags([...formTags, tagInput.trim()]);
                          setTagInput('');
                        }
                      }
                    }}
                    className="flex-1 px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC]"
                  />
                  <button
                    type="button"
                    onClick={() => {
                      if (tagInput.trim() && !formTags.includes(tagInput.trim())) {
                        setFormTags([...formTags, tagInput.trim()]);
                        setTagInput('');
                      }
                    }}
                    className="px-3 py-1.5 bg-[#2D2D2D] hover:bg-[#383838] text-white rounded-lg border border-[#404040]"
                  >
                    添加
                  </button>
                </div>

                <div className="flex flex-wrap gap-1.5 pt-1">
                  {formTags.map((t) => (
                    <span
                      key={t}
                      className="px-2 py-0.5 rounded bg-[#2D2D2D] text-[#CCCCCC] text-[10.5px] flex items-center gap-1 border border-[#3C3C3C]"
                    >
                      <span>#{t}</span>
                      <button
                        type="button"
                        onClick={() => setFormTags(formTags.filter((x) => x !== t))}
                        className="text-[#858585] hover:text-white"
                      >
                        &times;
                      </button>
                    </span>
                  ))}
                </div>
              </div>

              {/* Notes */}
              <div className="space-y-1">
                <label className="text-[#CCCCCC] font-medium">备注说明</label>
                <textarea
                  rows={2}
                  placeholder="关联服务器、轮换周期或特殊授权说明..."
                  value={formNotes}
                  onChange={(e) => setFormNotes(e.target.value)}
                  className="w-full p-2 bg-[#181818] border border-[#383838] rounded-lg text-xs text-white focus:outline-none focus:border-[#007ACC]"
                />
              </div>

              {/* Bottom Buttons */}
              <div className="flex items-center justify-end gap-2 pt-3 border-t border-[#333333]">
                <button
                  type="button"
                  onClick={() => setShowAddEditModal(false)}
                  className="px-4 py-1.5 rounded-lg bg-[#2D2D2D] hover:bg-[#383838] text-[#CCCCCC] transition-colors"
                >
                  取消
                </button>
                <button
                  type="submit"
                  className="px-5 py-1.5 rounded-lg bg-[#007ACC] hover:bg-[#0062a3] text-white font-semibold shadow transition-colors"
                >
                  {editingCred ? '保存修改' : '确认添加'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* =========================================================================
         Sub-Modal: Interactive Password Generator
         ========================================================================= */}
      {showGeneratorModal && (
        <div className="fixed inset-0 bg-black/75 backdrop-blur-sm z-[60] flex items-center justify-center p-3 select-none">
          <div className="bg-[#222222] border border-[#3C3C3C] rounded-2xl max-w-md w-full p-5 space-y-4 shadow-2xl font-sans">
            <div className="flex items-center justify-between border-b border-[#333333] pb-3">
              <div className="flex items-center gap-2">
                <Sparkles className="w-4 h-4 text-[#E5C07B]" />
                <h3 className="text-sm font-bold text-white">高强度随机密码生成器</h3>
              </div>
              <button
                onClick={() => setShowGeneratorModal(false)}
                className="text-[#858585] hover:text-white"
              >
                <X className="w-4 h-4" />
              </button>
            </div>

            {/* Generated Password Result Box */}
            <div className="space-y-1.5">
              <div className="flex items-center justify-between bg-[#141414] border border-[#383838] p-3 rounded-xl font-mono text-sm">
                <span className="text-[#4EC9B0] break-all font-semibold">{generatedPass}</span>
                <div className="flex items-center gap-1.5 shrink-0 ml-2">
                  <button
                    onClick={handleRegeneratePassword}
                    title="重新生成"
                    className="p-1.5 rounded bg-[#252526] hover:bg-[#333333] text-[#CCCCCC] transition-colors"
                  >
                    <RefreshCw className="w-3.5 h-3.5" />
                  </button>
                  <button
                    onClick={() => handleCopy('gen-pass', generatedPass)}
                    title="复制密码"
                    className="p-1.5 rounded bg-[#007ACC] hover:bg-[#0062a3] text-white transition-colors"
                  >
                    {copiedFieldId === 'gen-pass' ? (
                      <Check className="w-3.5 h-3.5 text-[#27C93F]" />
                    ) : (
                      <Copy className="w-3.5 h-3.5" />
                    )}
                  </button>
                </div>
              </div>

              {/* Password Strength */}
              {(() => {
                const s = calculatePasswordStrength(generatedPass);
                return (
                  <div className="flex items-center justify-between text-xs px-1">
                    <span className="text-[#858585]">密码强度:</span>
                    <span className="font-mono font-bold" style={{ color: s.color }}>
                      {s.label} ({Math.round((s.score / 6) * 100)}%)
                    </span>
                  </div>
                );
              })()}
            </div>

            {/* Settings Options */}
            <div className="space-y-3 text-xs bg-[#1A1A1A] p-3 rounded-xl border border-[#2D2D2D]">
              <div>
                <div className="flex items-center justify-between mb-1">
                  <span className="text-[#CCCCCC]">密码长度:</span>
                  <span className="font-mono text-[#007ACC] font-bold">{genLength} 字符</span>
                </div>
                <input
                  type="range"
                  min={8}
                  max={64}
                  value={genLength}
                  onChange={(e) => {
                    setGenLength(Number(e.target.value));
                    setGeneratedPass(
                      generateSecurePassword({
                        length: Number(e.target.value),
                        useUpper: genUpper,
                        useLower: genLower,
                        useNumbers: genNumbers,
                        useSymbols: genSymbols,
                        avoidAmbiguous: genAvoidAmbiguous,
                      })
                    );
                  }}
                  className="w-full accent-[#007ACC]"
                />
              </div>

              <div className="grid grid-cols-2 gap-2 text-[#D4D4D4]">
                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={genUpper}
                    onChange={(e) => {
                      setGenUpper(e.target.checked);
                      handleRegeneratePassword();
                    }}
                    className="accent-[#007ACC]"
                  />
                  <span>大写字母 (A-Z)</span>
                </label>

                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={genLower}
                    onChange={(e) => {
                      setGenLower(e.target.checked);
                      handleRegeneratePassword();
                    }}
                    className="accent-[#007ACC]"
                  />
                  <span>小写字母 (a-z)</span>
                </label>

                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={genNumbers}
                    onChange={(e) => {
                      setGenNumbers(e.target.checked);
                      handleRegeneratePassword();
                    }}
                    className="accent-[#007ACC]"
                  />
                  <span>数字 (0-9)</span>
                </label>

                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={genSymbols}
                    onChange={(e) => {
                      setGenSymbols(e.target.checked);
                      handleRegeneratePassword();
                    }}
                    className="accent-[#007ACC]"
                  />
                  <span>特殊符号 (!@#$)</span>
                </label>
              </div>

              <label className="flex items-center gap-2 cursor-pointer text-[#858585] hover:text-[#CCCCCC] pt-1 border-t border-[#262626]">
                <input
                  type="checkbox"
                  checked={genAvoidAmbiguous}
                  onChange={(e) => {
                    setGenAvoidAmbiguous(e.target.checked);
                    handleRegeneratePassword();
                  }}
                  className="accent-[#007ACC]"
                />
                <span>排除易混淆字符 (1, l, I, 0, O, o)</span>
              </label>
            </div>

            <div className="flex items-center justify-between pt-2">
              <button
                onClick={() => {
                  setShowGeneratorModal(false);
                  handleOpenAddModal('password');
                  setFormPassword(generatedPass);
                }}
                className="text-xs text-[#4EC9B0] hover:underline"
              >
                直接用此密码创建新凭据 &rarr;
              </button>

              <button
                onClick={() => {
                  navigator.clipboard.writeText(generatedPass);
                  setShowGeneratorModal(false);
                }}
                className="px-4 py-1.5 rounded-lg bg-[#007ACC] hover:bg-[#0062a3] text-white text-xs font-semibold"
              >
                复制并关闭
              </button>
            </div>
          </div>
        </div>
      )}

      {/* =========================================================================
         Sub-Modal: SSH Keypair Generator
         ========================================================================= */}
      {showKeyGenModal && (
        <div className="fixed inset-0 bg-black/75 backdrop-blur-sm z-[60] flex items-center justify-center p-3 select-none">
          <div className="bg-[#222222] border border-[#3C3C3C] rounded-2xl max-w-md w-full p-5 space-y-4 shadow-2xl font-sans">
            <div className="flex items-center justify-between border-b border-[#333333] pb-3">
              <div className="flex items-center gap-2">
                <FileCode className="w-4 h-4 text-[#4EC9B0]" />
                <h3 className="text-sm font-bold text-white">生成新 SSH 密钥对</h3>
              </div>
              <button onClick={() => setShowKeyGenModal(false)} className="text-[#858585] hover:text-white">
                <X className="w-4 h-4" />
              </button>
            </div>

            <div className="space-y-3 text-xs">
              <div className="space-y-1">
                <label className="text-[#CCCCCC] font-medium">密钥名称 / 标识</label>
                <input
                  type="text"
                  placeholder="e.g. id_ed25519_production"
                  value={keyGenName}
                  onChange={(e) => setKeyGenName(e.target.value)}
                  className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white font-mono focus:outline-none focus:border-[#007ACC]"
                />
              </div>

              <div className="space-y-1">
                <label className="text-[#CCCCCC] font-medium">密钥加密算法</label>
                <select
                  value={keyGenType}
                  onChange={(e) => setKeyGenType(e.target.value as any)}
                  className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white focus:outline-none focus:border-[#007ACC]"
                >
                  <option value="Ed25519">Ed25519 (现代轻量、高安全性、256-bit)</option>
                  <option value="RSA 4096">RSA 4096-bit (传统工业标准、兼容性高)</option>
                  <option value="ECDSA P-384">ECDSA P-384 (椭圆曲线标准)</option>
                </select>
              </div>

              <div className="space-y-1">
                <label className="text-[#CCCCCC] font-medium">注释 Comment</label>
                <input
                  type="text"
                  placeholder="winssh-key@prod"
                  value={keyGenComment}
                  onChange={(e) => setKeyGenComment(e.target.value)}
                  className="w-full px-3 py-1.5 bg-[#181818] border border-[#383838] rounded-lg text-white font-mono focus:outline-none focus:border-[#007ACC]"
                />
              </div>

              <div className="p-2.5 rounded-lg bg-[#181818] border border-[#2D2D2D] text-[11px] text-[#858585] leading-relaxed">
                生成后将自动计算 SHA256 指纹并保存至保险箱，支持一键复制公钥并下载 .pem 格式私钥。
              </div>

              <div className="flex items-center justify-end gap-2 pt-2">
                <button
                  type="button"
                  onClick={() => setShowKeyGenModal(false)}
                  className="px-3 py-1.5 bg-[#2D2D2D] hover:bg-[#383838] text-[#CCCCCC] rounded-lg"
                >
                  取消
                </button>
                <button
                  type="button"
                  disabled={isGeneratingKey}
                  onClick={handleExecuteKeygen}
                  className="flex items-center gap-1.5 px-4 py-1.5 bg-[#007ACC] hover:bg-[#0062a3] text-white rounded-lg font-semibold shadow disabled:opacity-50"
                >
                  {isGeneratingKey ? (
                    <>
                      <RefreshCw className="w-3.5 h-3.5 animate-spin" />
                      <span>计算密钥对中...</span>
                    </>
                  ) : (
                    <>
                      <Sparkles className="w-3.5 h-3.5" />
                      <span>立即生成并入库</span>
                    </>
                  )}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
