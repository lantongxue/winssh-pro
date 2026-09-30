export type OsType = 'ubuntu' | 'debian' | 'centos' | 'alpine' | 'macos' | 'windows' | 'freebsd' | 'arch';

export type AuthType = 'password' | 'privateKey' | 'key' | 'agent' | 'none';

export interface ServerHost {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  password?: string;
  keyId?: string;
  group: string;
  tags: string[];
  color: string;
  osType: OsType;
  status: 'online' | 'offline' | 'connecting' | 'error';
  lastConnected?: string;
  pingMs?: number;
  jumpHostId?: string;
  notes?: string;
  startupScript?: string;
}

export type LineType = 'command' | 'output' | 'error' | 'system' | 'prompt' | 'ascii' | 'success' | 'warning';

export interface TerminalLine {
  id: string;
  text: string;
  type: LineType;
  timestamp: string;
  promptInfo?: {
    user: string;
    host: string;
    cwd: string;
  };
}

export interface TerminalPane {
  id: string;
  hostId: string;
  title?: string;
  history: TerminalLine[];
  commandHistory: string[];
  historyIdx: number;
  currentInput?: string;
  cwd: string;
  isConnected?: boolean;
  activeProcess?: string | null;
}

export type TabType = 'terminal' | 'sftp' | 'monitor' | 'tunnels' | 'docker' | 'snippets';

export interface SessionTab {
  id: string;
  title: string;
  hostId: string;
  type: TabType;
  split: 'none' | 'single' | 'vertical' | 'horizontal';
  activePaneId: string;
  panes: TerminalPane[];
  connectedAt?: string;
  // Session bottom SFTP state
  sftpOpen?: boolean;
  sftpRemotePath?: string;
  sftpLocalPath?: string;
  sftpFollowTerminalCwd?: boolean;
  sftpHeight?: number;
  sftpViewMode?: 'dual' | 'remote-only';
  sftpFilter?: string;
  // Session right Top/Monitor panel state
  monitorOpen?: boolean;
  monitorWidth?: number;
}

export interface SFTPFile {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  permissions: string;
  modified: string;
  owner: string;
  group: string;
  content?: string;
}

export interface SFTPTransferSubItem {
  id: string;
  filename: string;
  relativePath?: string;
  size: number;
  transferred: number;
  speed?: string;
  status: 'queued' | 'transferring' | 'completed' | 'failed' | 'paused';
  progress: number;
  isDir?: boolean;
  errorMsg?: string;
}

export interface SFTPTransferItem {
  id: string;
  filename: string;
  direction: 'upload' | 'download';
  size: number;
  transferred: number;
  speed: string;
  status: 'queued' | 'transferring' | 'completed' | 'failed' | 'paused';
  progress: number;
  hostName: string;
  isDir?: boolean;
  totalFiles?: number;
  completedFiles?: number;
  children?: SFTPTransferSubItem[];
  path?: string;
  remotePath?: string;
  startTime?: string;
  errorMsg?: string;
}

export type TunnelType = 'local' | 'remote' | 'dynamic';

export interface PortTunnel {
  id: string;
  name: string;
  hostId: string;
  type: TunnelType;
  bindAddress: string;
  localPort: number;
  remoteHost: string;
  remotePort: number;
  status: 'active' | 'inactive';
  bytesTransferred: number;
  description: string;
}

export interface SnippetVariable {
  name: string;
  default: string;
  label: string;
}

export interface DevOpsSnippet {
  id: string;
  title: string;
  category: 'Docker & K8s' | 'System & Maintenance' | 'Nginx & Web' | 'Database & Storage' | 'Security & Firewall' | 'Git & DevOps' | 'Network & Diagnostics';
  command: string;
  description: string;
  tags: string[];
  variables?: SnippetVariable[];
}

export interface SSHKeyItem {
  id: string;
  name: string;
  type: 'Ed25519' | 'RSA 4096' | 'ECDSA P-384';
  fingerprint: string;
  publicKey: string;
  privateKey: string;
  createdDate: string;
  comment: string;
  bits?: number;
}

export type VaultCredentialType = 'password' | 'privateKey' | 'token';

export interface VaultCredential {
  id: string;
  name: string;
  type: VaultCredentialType;
  category?: string;
  username?: string;
  password?: string;
  // Private key fields
  keyType?: 'Ed25519' | 'RSA 4096' | 'ECDSA P-384';
  publicKey?: string;
  privateKey?: string;
  passphrase?: string;
  fingerprint?: string;
  // Token / secret fields
  token?: string;
  tags: string[];
  notes?: string;
  createdDate: string;
  updatedDate?: string;
  favorite?: boolean;
}

export interface ProcessItem {
  pid: number;
  user: string;
  cpu: number;
  mem: number;
  command: string;
  state: 'R' | 'S' | 'D' | 'Z';
}

export interface DockerContainerItem {
  id: string;
  name: string;
  image: string;
  status: 'running' | 'exited' | 'restarting';
  ports: string;
  cpu: number;
  mem: string;
  created: string;
}

export interface ServerTelemetry {
  hostId: string;
  cpu: number;
  cpuHistory: number[];
  memoryUsedGB: number;
  memoryTotalGB: number;
  memHistory: number[];
  diskUsedGB: number;
  diskTotalGB: number;
  netInKBps: number;
  netOutKBps: number;
  uptime: string;
  loadAvg: [number, number, number];
  processes: ProcessItem[];
  containers: DockerContainerItem[];
}

export interface AICopilotMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  codeSnippet?: string;
  commandSuggestions?: string[];
  timestamp: string;
}

// Clash.Meta (Mihomo) Types
export type ClashNodeType =
  | 'hysteria2'
  | 'vless'
  | 'vmess'
  | 'trojan'
  | 'ss'
  | 'ssr'
  | 'wireguard'
  | 'tuic'
  | 'direct'
  | 'reject';

export interface ClashNode {
  id: string;
  name: string;
  type: ClashNodeType;
  server: string;
  port: number;
  latency?: number | null; // in ms (-1 = timeout, null = untested)
  isTesting?: boolean;
  udp?: boolean;
  tls?: boolean;
  network?: string;
  country?: string;
  flag?: string;
  status: 'online' | 'slow' | 'timeout' | 'untested';
  rate?: number;
}

export interface ClashProxyGroup {
  id: string;
  name: string;
  type: 'select' | 'url-test' | 'fallback' | 'load-balance';
  selectedNode: string;
  proxies: string[];
  testUrl?: string;
  interval?: number;
}

export type ClashRuleType =
  | 'DOMAIN-SUFFIX'
  | 'DOMAIN'
  | 'DOMAIN-KEYWORD'
  | 'IP-CIDR'
  | 'GEOIP'
  | 'GEOSITE'
  | 'MATCH'
  | 'RULE-SET'
  | 'PROCESS-NAME';

export interface ClashRule {
  id: string;
  type: ClashRuleType;
  payload: string;
  target: string;
  noResolve?: boolean;
  hits?: number;
}

export interface ClashSubscription {
  id: string;
  name: string;
  url: string;
  rawYaml?: string;
  updatedAt: string;
  autoUpdateInterval: number; // hours (0 = off)
  uploadBytes: number;
  downloadBytes: number;
  totalBytes: number;
  expireDate: string;
  nodes: ClashNode[];
  proxyGroups: ClashProxyGroup[];
  rules: ClashRule[];
  status: 'active' | 'ready' | 'updating' | 'error';
  tag?: string;
  color?: string;
  notes?: string;
  userAgent?: string;
}

export type ClashProxyMode = 'rule' | 'global' | 'direct' | 'script';

export interface ClashCoreSettings {
  masterProxyEnabled: boolean;
  systemProxyEnabled: boolean;
  tunEnabled: boolean;
  mode: ClashProxyMode;
  mixedPort: number;
  socksPort: number;
  redirPort: number;
  tproxyPort: number;
  apiPort: number;
  allowLan: boolean;
  tunStack: 'gVisor' | 'mixed' | 'system';
  tunDevice: string;
  fakeIp: boolean;
  autoRoute: boolean;
  strictRoute: boolean;
  dnsServers: string[];
}

export interface ClashActiveConnection {
  id: string;
  host: string;
  destinationIP: string;
  destinationPort: number;
  network: 'tcp' | 'udp';
  process: string;
  rule: string;
  proxyChain: string[];
  upload: number;
  download: number;
  speed: string;
  time: string;
}

export type ThemeType = 'winssh-dark' | 'dracula' | 'nord' | 'tokyo-night' | 'one-dark' | 'matrix-green' | 'light-clean' | 'dark' | 'matrix' | 'solarized';

export interface AppSettings {
  theme: ThemeType;
  fontSize: number;
  fontFamily: string;
  cursorStyle: 'block' | 'underline' | 'bar';
  cursorBlink: boolean;
  osWindowFrame?: 'windows' | 'macos' | 'linux';
  soundEffects?: boolean;
  broadcastMode?: boolean;
  autoReconnect?: boolean;
  terminalOpacity?: number;
  sftpShowHidden?: boolean;
  scrollbackLines?: number;
  bellSound?: boolean;
  keepaliveInterval?: number;
  activeOS?: 'windows' | 'macos' | 'linux';
}
