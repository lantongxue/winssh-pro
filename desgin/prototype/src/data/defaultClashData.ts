import {
  ClashSubscription,
  ClashCoreSettings,
  ClashActiveConnection,
  ClashNode,
  ClashRule,
  ClashProxyGroup,
} from '../types';

export const DEFAULT_CLASH_SETTINGS: ClashCoreSettings = {
  masterProxyEnabled: true,
  systemProxyEnabled: true,
  tunEnabled: false,
  mode: 'rule',
  mixedPort: 7890,
  socksPort: 7891,
  redirPort: 7892,
  tproxyPort: 7893,
  apiPort: 9090,
  allowLan: false,
  tunStack: 'gVisor',
  tunDevice: 'MetaTUN',
  fakeIp: true,
  autoRoute: true,
  strictRoute: false,
  dnsServers: ['223.5.5.5', '119.29.29.29', 'tls://1.1.1.1:853', 'https://dns.alidns.com/dns-query'],
};

const SAMPLE_YAML_PREMIUM = `mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
ipv6: false
external-controller: 127.0.0.1:9090

tun:
  enable: false
  stack: gVisor
  device: MetaTUN
  auto-route: true
  auto-detect-interface: true
  dns-hijack:
    - any:53
    - tcp://any:53

dns:
  enable: true
  listen: :1053
  enhanced-mode: fake-ip
  fake-ip-range: 198.18.0.1/16
  nameserver:
    - 223.5.5.5
    - 119.29.29.29
    - https://dns.alidns.com/dns-query

proxies:
  - name: "🇭🇰 香港 01 [Hysteria2 专线]"
    type: hysteria2
    server: hk01.meta-fast.net
    port: 443
    password: auth-token-hy2-8921
    sni: hk01.meta-fast.net
    alpn: [h3]
    up: "100 Mbps"
    down: "500 Mbps"

  - name: "🇭🇰 香港 02 [VLESS Reality]"
    type: vless
    server: hk02.meta-fast.net
    port: 443
    uuid: 8a4b2c1d-9e3f-4a5b-8c7d-1e2f3a4b5c6d
    network: tcp
    tls: true
    reality-opts:
      public-key: 7xK...realityPubKey
      short-id: a1b2c3d4
    client-fingerprint: chrome

proxy-groups:
  - name: "🚀 节点选择"
    type: select
    proxies:
      - "♻️ 自动选择"
      - "🇭🇰 香港 01 [Hysteria2 专线]"
      - "🇭🇰 香港 02 [VLESS Reality]"
      - "🇯🇵 东京 01 [Hysteria2 4K]"
      - "🇺🇸 洛杉矶 01 [Hysteria2 8K]"
      - "🇸🇬 新加坡 01 [Trojan BGP]"
      - DIRECT

rules:
  - DOMAIN-SUFFIX,google.com,🚀 节点选择
  - DOMAIN-SUFFIX,github.com,🚀 节点选择
  - DOMAIN-SUFFIX,openai.com,🇺🇸 美国节点
  - DOMAIN-SUFFIX,claude.ai,🇺🇸 美国节点
  - GEOIP,CN,DIRECT
  - MATCH,🐟 漏网之鱼
`;

export const DEFAULT_CLASH_SUBSCRIPTIONS: ClashSubscription[] = [
  {
    id: 'sub-bgp-fast',
    name: '🚀 极速专线 BGP IEPL (Clash.Meta)',
    url: 'https://sub.metasub-speed.io/api/v1/client/subscribe?token=8f9a2b3c4d5e6f7a',
    rawYaml: SAMPLE_YAML_PREMIUM,
    updatedAt: '2026-09-29 22:45',
    autoUpdateInterval: 24,
    uploadBytes: 18.4 * 1024 * 1024 * 1024,
    downloadBytes: 64.2 * 1024 * 1024 * 1024,
    totalBytes: 250 * 1024 * 1024 * 1024,
    expireDate: '2027-10-15',
    status: 'active',
    tag: 'IEPL高速',
    color: '#06b6d4',
    userAgent: 'clash.meta / Mihomo v1.18.9',
    notes: '沪港、深港高速专线，支持 UDP 转发、Hysteria2 与 VLESS-Reality 全协议。',
    nodes: [
      {
        id: 'node-hk-01',
        name: '🇭🇰 香港 01 [Hysteria2 专线]',
        type: 'hysteria2',
        server: 'hk01.meta-fast.net',
        port: 443,
        latency: 32,
        udp: true,
        tls: true,
        network: 'udp/h3',
        country: 'HK',
        flag: '🇭🇰',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-hk-02',
        name: '🇭🇰 香港 02 [VLESS Reality]',
        type: 'vless',
        server: 'hk02.meta-fast.net',
        port: 443,
        latency: 36,
        udp: true,
        tls: true,
        network: 'tcp',
        country: 'HK',
        flag: '🇭🇰',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-hk-03',
        name: '🇭🇰 香港 03 [Trojan 03]',
        type: 'trojan',
        server: 'hk03.meta-fast.net',
        port: 8443,
        latency: 42,
        udp: true,
        tls: true,
        network: 'tcp/tls',
        country: 'HK',
        flag: '🇭🇰',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-jp-01',
        name: '🇯🇵 东京 01 [Hysteria2 4K]',
        type: 'hysteria2',
        server: 'jp01.meta-fast.net',
        port: 443,
        latency: 58,
        udp: true,
        tls: true,
        network: 'udp/h3',
        country: 'JP',
        flag: '🇯🇵',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-jp-02',
        name: '🇯🇵 东京 02 [VLESS Reality]',
        type: 'vless',
        server: 'jp02.meta-fast.net',
        port: 443,
        latency: 63,
        udp: true,
        tls: true,
        network: 'tcp',
        country: 'JP',
        flag: '🇯🇵',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-us-01',
        name: '🇺🇸 洛杉矶 01 [Hysteria2 8K]',
        type: 'hysteria2',
        server: 'us01.meta-fast.net',
        port: 443,
        latency: 138,
        udp: true,
        tls: true,
        network: 'udp/h3',
        country: 'US',
        flag: '🇺🇸',
        status: 'online',
        rate: 1.2,
      },
      {
        id: 'node-us-02',
        name: '🇺🇸 硅谷 02 [TUIC v5 直连]',
        type: 'tuic',
        server: 'us02.meta-fast.net',
        port: 8443,
        latency: 145,
        udp: true,
        tls: true,
        network: 'quic',
        country: 'US',
        flag: '🇺🇸',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-sg-01',
        name: '🇸🇬 新加坡 01 [Trojan BGP]',
        type: 'trojan',
        server: 'sg01.meta-fast.net',
        port: 443,
        latency: 68,
        udp: true,
        tls: true,
        network: 'tcp/tls',
        country: 'SG',
        flag: '🇸🇬',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-sg-02',
        name: '🇸🇬 新加坡 02 [VMess 极速]',
        type: 'vmess',
        server: 'sg02.meta-fast.net',
        port: 2053,
        latency: 74,
        udp: true,
        tls: true,
        network: 'ws',
        country: 'SG',
        flag: '🇸🇬',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-tw-01',
        name: '🇹🇼 台北 01 [Hysteria2 专线]',
        type: 'hysteria2',
        server: 'tw01.meta-fast.net',
        port: 443,
        latency: 48,
        udp: true,
        tls: true,
        network: 'udp/h3',
        country: 'TW',
        flag: '🇹🇼',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-de-01',
        name: '🇩🇪 法兰克福 01 [WireGuard]',
        type: 'wireguard',
        server: 'de01.meta-fast.net',
        port: 51820,
        latency: 178,
        udp: true,
        tls: false,
        network: 'udp',
        country: 'DE',
        flag: '🇩🇪',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-uk-01',
        name: '🇬🇧 伦敦 01 [Shadowsocks 2022]',
        type: 'ss',
        server: 'uk01.meta-fast.net',
        port: 8388,
        latency: 192,
        udp: true,
        tls: false,
        network: 'tcp/udp',
        country: 'UK',
        flag: '🇬🇧',
        status: 'online',
        rate: 1.0,
      },
    ],
    proxyGroups: [
      {
        id: 'group-proxy',
        name: '🚀 节点选择',
        type: 'select',
        selectedNode: '🇭🇰 香港 01 [Hysteria2 专线]',
        proxies: [
          '♻️ 自动选择',
          '🇭🇰 香港 01 [Hysteria2 专线]',
          '🇭🇰 香港 02 [VLESS Reality]',
          '🇯🇵 东京 01 [Hysteria2 4K]',
          '🇺🇸 洛杉矶 01 [Hysteria2 8K]',
          '🇸🇬 新加坡 01 [Trojan BGP]',
          'DIRECT',
        ],
      },
      {
        id: 'group-auto',
        name: '♻️ 自动选择',
        type: 'url-test',
        selectedNode: '🇭🇰 香港 01 [Hysteria2 专线]',
        testUrl: 'http://www.gstatic.com/generate_204',
        interval: 300,
        proxies: [
          '🇭🇰 香港 01 [Hysteria2 专线]',
          '🇭🇰 香港 02 [VLESS Reality]',
          '🇯🇵 东京 01 [Hysteria2 4K]',
          '🇸🇬 新加坡 01 [Trojan BGP]',
        ],
      },
      {
        id: 'group-hk',
        name: '🇭🇰 香港节点',
        type: 'select',
        selectedNode: '🇭🇰 香港 01 [Hysteria2 专线]',
        proxies: [
          '🇭🇰 香港 01 [Hysteria2 专线]',
          '🇭🇰 香港 02 [VLESS Reality]',
          '🇭🇰 香港 03 [Trojan 03]',
        ],
      },
      {
        id: 'group-jp',
        name: '🇯🇵 日本节点',
        type: 'select',
        selectedNode: '🇯🇵 东京 01 [Hysteria2 4K]',
        proxies: ['🇯🇵 东京 01 [Hysteria2 4K]', '🇯🇵 东京 02 [VLESS Reality]'],
      },
      {
        id: 'group-us',
        name: '🇺🇸 美国节点',
        type: 'select',
        selectedNode: '🇺🇸 洛杉矶 01 [Hysteria2 8K]',
        proxies: ['🇺🇸 洛杉矶 01 [Hysteria2 8K]', '🇺🇸 硅谷 02 [TUIC v5 直连]'],
      },
      {
        id: 'group-sg',
        name: '🇸🇬 新加坡节点',
        type: 'select',
        selectedNode: '🇸🇬 新加坡 01 [Trojan BGP]',
        proxies: ['🇸🇬 新加坡 01 [Trojan BGP]', '🇸🇬 新加坡 02 [VMess 极速]'],
      },
      {
        id: 'group-final',
        name: '🐟 漏网之鱼',
        type: 'select',
        selectedNode: '🚀 节点选择',
        proxies: ['🚀 节点选择', 'DIRECT'],
      },
    ],
    rules: [
      { id: 'rule-1', type: 'DOMAIN-SUFFIX', payload: 'google.com', target: '🚀 节点选择', hits: 1420 },
      { id: 'rule-2', type: 'DOMAIN-SUFFIX', payload: 'github.com', target: '🚀 节点选择', hits: 980 },
      { id: 'rule-3', type: 'DOMAIN-SUFFIX', payload: 'openai.com', target: '🇺🇸 美国节点', hits: 760 },
      { id: 'rule-4', type: 'DOMAIN-SUFFIX', payload: 'claude.ai', target: '🇺🇸 美国节点', hits: 540 },
      { id: 'rule-5', type: 'DOMAIN-SUFFIX', payload: 'anthropic.com', target: '🇺🇸 美国节点', hits: 310 },
      { id: 'rule-6', type: 'DOMAIN-SUFFIX', payload: 'youtube.com', target: '🚀 节点选择', hits: 2310 },
      { id: 'rule-7', type: 'DOMAIN-SUFFIX', payload: 'docker.com', target: '🚀 节点选择', hits: 450 },
      { id: 'rule-8', type: 'DOMAIN-SUFFIX', payload: 'registry-1.docker.io', target: '🚀 节点选择', hits: 620 },
      { id: 'rule-9', type: 'DOMAIN-SUFFIX', payload: 'telegram.org', target: '🚀 节点选择', hits: 890 },
      { id: 'rule-10', type: 'DOMAIN-KEYWORD', payload: 'torrent', target: 'REJECT', hits: 12 },
      { id: 'rule-11', type: 'DOMAIN-KEYWORD', payload: 'tracker', target: 'REJECT', hits: 8 },
      { id: 'rule-12', type: 'IP-CIDR', payload: '192.168.0.0/16', target: 'DIRECT', hits: 4890 },
      { id: 'rule-13', type: 'IP-CIDR', payload: '10.0.0.0/8', target: 'DIRECT', hits: 6200 },
      { id: 'rule-14', type: 'IP-CIDR', payload: '127.0.0.0/8', target: 'DIRECT', hits: 15400 },
      { id: 'rule-15', type: 'GEOIP', payload: 'CN', target: 'DIRECT', hits: 28400 },
      { id: 'rule-16', type: 'MATCH', payload: '', target: '🐟 漏网之鱼', hits: 1120 },
    ],
  },
  {
    id: 'sub-devops-cloud',
    name: '⚡ DevOps 海外自建专网 (Cloud-Mihomo)',
    url: 'https://sub.mydevops-proxy.internal/clash-meta.yaml',
    rawYaml: '# DevOps internal proxy nodes config',
    updatedAt: '2026-09-29 18:20',
    autoUpdateInterval: 12,
    uploadBytes: 8.5 * 1024 * 1024 * 1024,
    downloadBytes: 31.4 * 1024 * 1024 * 1024,
    totalBytes: 500 * 1024 * 1024 * 1024,
    expireDate: '2028-06-30',
    status: 'ready',
    tag: '自建节点',
    color: '#8b5cf6',
    userAgent: 'Mihomo/1.18.9',
    notes: '自建 AWS us-west-2 / GCP tokyo / Azure singapore 专线集群，支持超大并发与 DevOps 加速。',
    nodes: [
      {
        id: 'node-dev-us',
        name: '🇺🇸 AWS-Oregon [Hysteria2 10G]',
        type: 'hysteria2',
        server: 'aws-us.devops.net',
        port: 443,
        latency: 132,
        udp: true,
        tls: true,
        network: 'udp/h3',
        country: 'US',
        flag: '🇺🇸',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-dev-jp',
        name: '🇯🇵 GCP-Tokyo [VLESS-Reality]',
        type: 'vless',
        server: 'gcp-jp.devops.net',
        port: 443,
        latency: 55,
        udp: true,
        tls: true,
        network: 'tcp',
        country: 'JP',
        flag: '🇯🇵',
        status: 'online',
        rate: 1.0,
      },
      {
        id: 'node-dev-sg',
        name: '🇸🇬 Azure-SG [TUIC v5]',
        type: 'tuic',
        server: 'azure-sg.devops.net',
        port: 8443,
        latency: 66,
        udp: true,
        tls: true,
        network: 'quic',
        country: 'SG',
        flag: '🇸🇬',
        status: 'online',
        rate: 1.0,
      },
    ],
    proxyGroups: [
      {
        id: 'grp-dev-1',
        name: '🚀 节点选择',
        type: 'select',
        selectedNode: '🇯🇵 GCP-Tokyo [VLESS-Reality]',
        proxies: ['🇯🇵 GCP-Tokyo [VLESS-Reality]', '🇺🇸 AWS-Oregon [Hysteria2 10G]', '🇸🇬 Azure-SG [TUIC v5]'],
      },
    ],
    rules: [
      { id: 'r-dev-1', type: 'DOMAIN-SUFFIX', payload: 'github.com', target: '🚀 节点选择', hits: 320 },
      { id: 'r-dev-2', type: 'MATCH', payload: '', target: 'DIRECT', hits: 840 },
    ],
  },
  {
    id: 'sub-backup-fallback',
    name: '🌐 全球备用跨境加速通道',
    url: 'https://sub.backup-hub.cc/meta-feed?key=sec_98129',
    updatedAt: '2026-09-28 10:00',
    autoUpdateInterval: 48,
    uploadBytes: 2.1 * 1024 * 1024 * 1024,
    downloadBytes: 9.8 * 1024 * 1024 * 1024,
    totalBytes: 100 * 1024 * 1024 * 1024,
    expireDate: '2026-12-31',
    status: 'ready',
    tag: '备用通道',
    color: '#10b981',
    userAgent: 'clash.meta',
    notes: '应急备用节点库，覆盖欧美亚太主要机房。',
    nodes: [
      {
        id: 'node-bak-01',
        name: '🇭🇰 香港备用 01 [Trojan]',
        type: 'trojan',
        server: 'hk-bak.hub.cc',
        port: 443,
        latency: 49,
        udp: true,
        tls: true,
        country: 'HK',
        flag: '🇭🇰',
        status: 'online',
      },
      {
        id: 'node-bak-02',
        name: '🇯🇵 日本备用 02 [SS-2022]',
        type: 'ss',
        server: 'jp-bak.hub.cc',
        port: 8388,
        latency: 71,
        udp: true,
        tls: false,
        country: 'JP',
        flag: '🇯🇵',
        status: 'online',
      },
    ],
    proxyGroups: [
      {
        id: 'grp-bak-1',
        name: '🚀 节点选择',
        type: 'select',
        selectedNode: '🇭🇰 香港备用 01 [Trojan]',
        proxies: ['🇭🇰 香港备用 01 [Trojan]', '🇯🇵 日本备用 02 [SS-2022]'],
      },
    ],
    rules: [{ id: 'r-bak-1', type: 'MATCH', payload: '', target: 'DIRECT', hits: 140 }],
  },
];

export const INITIAL_ACTIVE_CONNECTIONS: ClashActiveConnection[] = [
  {
    id: 'conn-1',
    host: 'api.github.com:443',
    destinationIP: '140.82.112.5',
    destinationPort: 443,
    network: 'tcp',
    process: 'git / ssh',
    rule: 'DOMAIN-SUFFIX github.com',
    proxyChain: ['🚀 节点选择', '🇭🇰 香港 01 [Hysteria2 专线]'],
    upload: 48200,
    download: 2450000,
    speed: '420 KB/s',
    time: '24s ago',
  },
  {
    id: 'conn-2',
    host: 'registry-1.docker.io:443',
    destinationIP: '52.73.238.161',
    destinationPort: 443,
    network: 'tcp',
    process: 'dockerd',
    rule: 'DOMAIN-SUFFIX docker.io',
    proxyChain: ['🚀 节点选择', '🇭🇰 香港 01 [Hysteria2 专线]'],
    upload: 12000,
    download: 14500000,
    speed: '1.45 MB/s',
    time: '1m ago',
  },
  {
    id: 'conn-3',
    host: 'chat.openai.com:443',
    destinationIP: '104.18.37.228',
    destinationPort: 443,
    network: 'tcp',
    process: 'chrome',
    rule: 'DOMAIN-SUFFIX openai.com',
    proxyChain: ['🇺🇸 美国节点', '🇺🇸 洛杉矶 01 [Hysteria2 8K]'],
    upload: 34000,
    download: 890000,
    speed: '85 KB/s',
    time: '3m ago',
  },
  {
    id: 'conn-4',
    host: 'archive.ubuntu.com:80',
    destinationIP: '91.189.91.39',
    destinationPort: 80,
    network: 'tcp',
    process: 'apt-get',
    rule: 'MATCH',
    proxyChain: ['DIRECT'],
    upload: 4500,
    download: 6420000,
    speed: '820 KB/s',
    time: '5m ago',
  },
];
