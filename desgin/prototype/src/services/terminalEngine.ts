import { ServerHost, TerminalLine, SFTPFile } from '../types';
import { INITIAL_SFTP_FILES } from '../data/defaultData';

export interface CommandResult {
  lines: Omit<TerminalLine, 'id' | 'timestamp'>[];
  newCwd?: string;
  action?: 'open_editor' | 'clear' | 'exit' | 'open_monitor' | 'open_sftp' | 'reconnect';
  editorFile?: { path: string; name: string; content: string };
}

export function executeTerminalCommand(
  rawInput: string,
  host: ServerHost,
  cwd: string,
  sftpFiles: Record<string, SFTPFile[]>
): CommandResult {
  const input = rawInput.trim();
  if (!input) {
    return { lines: [] };
  }

  const parts = input.split(/\s+/);
  const command = parts[0];
  const args = parts.slice(1);

  // Clear screen
  if (command === 'clear') {
    return { action: 'clear', lines: [] };
  }

  // Exit/Logout
  if (command === 'exit' || command === 'logout') {
    return {
      action: 'exit',
      lines: [
        { text: `Connection to ${host.host} closed.`, type: 'system' },
        { text: `[Process completed (code 0)] - Press Enter to reconnect or close tab.`, type: 'system' },
      ],
    };
  }

  // Help command
  if (command === 'help' || command === '?') {
    return {
      lines: [
        { text: `WinSSH Pro Interactive Terminal v2.4 (OpenSSH 9.6p1 Compatible)`, type: 'system' },
        { text: `Available built-in Linux commands:`, type: 'output' },
        { text: `  • System:      neofetch, uname, uptime, date, whoami, id, hostname, free, df`, type: 'output' },
        { text: `  • Files:       ls, ll, cd, pwd, cat, touch, mkdir, rm, nano, vim, vi, chmod`, type: 'output' },
        { text: `  • Monitoring:  top, htop, ps aux, kill, systemctl, journalctl`, type: 'output' },
        { text: `  • Containers:  docker ps, docker logs, docker restart, kubectl get pods`, type: 'output' },
        { text: `  • Networking:  ping, curl, ifconfig, ip a, netstat, lsof, ssh`, type: 'output' },
        { text: `  • DevOps/Git:  git status, git branch, git log, deploy.sh`, type: 'output' },
        { text: `  • Terminal:    clear, history, exit, sftp, monitor, copilot`, type: 'output' },
        { text: `💡 Tip: Press Tab for auto-completion, Up/Down for command history.`, type: 'ascii' },
      ],
    };
  }

  // Quick Shortcuts for views
  if (command === 'sftp' || command === 'files') {
    return {
      action: 'open_sftp',
      lines: [{ text: `[WinSSH Pro] Launching integrated SFTP Dual-Pane File Manager...`, type: 'system' }],
    };
  }

  if (command === 'monitor' || command === 'dashboard') {
    return {
      action: 'open_monitor',
      lines: [{ text: `[WinSSH Pro] Opening real-time Host Telemetry Dashboard...`, type: 'system' }],
    };
  }

  // Neofetch / Fastfetch
  if (command === 'neofetch' || command === 'fastfetch') {
    const osInfo = getOsBanner(host.osType, host.name, host.username);
    return {
      lines: osInfo.map((l) => ({ text: l, type: 'ascii' })),
    };
  }

  // PWD
  if (command === 'pwd') {
    return {
      lines: [{ text: cwd, type: 'output' }],
    };
  }

  // CD (Change directory)
  if (command === 'cd') {
    const target = args[0] || '/root';
    let resolved = cwd;

    if (target === '~' || target === '/root') {
      resolved = '/root';
    } else if (target === '/') {
      resolved = '/';
    } else if (target === '..') {
      if (cwd === '/' || cwd === '/root') {
        resolved = '/';
      } else {
        const segs = cwd.split('/').filter(Boolean);
        segs.pop();
        resolved = '/' + segs.join('/');
        if (resolved === '') resolved = '/';
      }
    } else if (target.startsWith('/')) {
      resolved = target;
    } else {
      resolved = cwd === '/' ? `/${target}` : `${cwd}/${target}`;
    }

    // Check if directory exists in mock SFTP
    const parentPath = resolved === '/' ? '/' : resolved.substring(0, resolved.lastIndexOf('/')) || '/';
    const folderName = resolved.split('/').filter(Boolean).pop();

    const parentFiles = sftpFiles[parentPath] || sftpFiles['/'] || [];
    const exists = resolved === '/' || sftpFiles[resolved] || parentFiles.some((f) => f.name === folderName && f.isDir);

    if (exists || resolved === '/root' || resolved === '/etc' || resolved === '/var/log' || resolved === '/opt') {
      return {
        newCwd: resolved,
        lines: [],
      };
    } else {
      return {
        lines: [{ text: `bash: cd: ${target}: No such file or directory`, type: 'error' }],
      };
    }
  }

  // LS / LL
  if (command === 'ls' || command === 'll' || command === 'dir') {
    const isLong = command === 'll' || args.includes('-l') || args.includes('-la') || args.includes('-al');
    const showHidden = command === 'll' || args.includes('-a') || args.includes('-la') || args.includes('-al');

    const files = sftpFiles[cwd] || INITIAL_SFTP_FILES[cwd] || INITIAL_SFTP_FILES['/'] || [];
    const filtered = files.filter((f) => showHidden || !f.name.startsWith('.'));

    if (isLong) {
      const header = `total ${filtered.length * 4}K`;
      const lines: Omit<TerminalLine, 'id' | 'timestamp'>[] = [{ text: header, type: 'output' }];
      filtered.forEach((f) => {
        const typeChar = f.isDir ? 'd' : '-';
        const color = f.isDir ? '\x1b[1;34m' : f.name.endsWith('.sh') ? '\x1b[1;32m' : '';
        lines.push({
          text: `${typeChar}${f.permissions}  1 ${f.owner.padEnd(8)} ${f.group.padEnd(8)} ${formatBytes(f.size).padStart(7)} ${f.modified} ${color}${f.name}${f.isDir ? '/' : ''}`,
          type: f.isDir ? 'prompt' : 'output',
        });
      });
      return { lines };
    } else {
      const items = filtered.map((f) => (f.isDir ? `${f.name}/` : f.name)).join('   ');
      return {
        lines: [{ text: items || '(empty directory)', type: 'output' }],
      };
    }
  }

  // CAT
  if (command === 'cat') {
    if (!args[0]) {
      return { lines: [{ text: `cat: missing file operand`, type: 'error' }] };
    }
    const filename = args[0];
    const files = sftpFiles[cwd] || INITIAL_SFTP_FILES[cwd] || [];
    const targetFile = files.find((f) => f.name === filename || f.path === filename);

    if (targetFile) {
      if (targetFile.isDir) {
        return { lines: [{ text: `cat: ${filename}: Is a directory`, type: 'error' }] };
      }
      const content = targetFile.content || `[Binary or text data for ${filename} - Size: ${targetFile.size} bytes]`;
      const splitLines = content.split('\n').map((l) => ({ text: l, type: 'output' as const }));
      return { lines: splitLines };
    } else {
      return { lines: [{ text: `cat: ${filename}: No such file or directory`, type: 'error' }] };
    }
  }

  // Nano / Vim / Vi interactive editor
  if (command === 'nano' || command === 'vim' || command === 'vi') {
    const filename = args[0] || 'untitled.conf';
    const files = sftpFiles[cwd] || INITIAL_SFTP_FILES[cwd] || [];
    const existingFile = files.find((f) => f.name === filename);

    const fullPath = cwd === '/' ? `/${filename}` : `${cwd}/${filename}`;
    const initialContent = existingFile?.content || `# WinSSH Pro Editor - ${filename}\n# Created: ${new Date().toISOString()}\n\n`;

    return {
      action: 'open_editor',
      editorFile: {
        path: fullPath,
        name: filename,
        content: initialContent,
      },
      lines: [{ text: `[Opening ${command.toUpperCase()} visual buffer: ${filename}...]`, type: 'system' }],
    };
  }

  // Uname
  if (command === 'uname') {
    if (args.includes('-a')) {
      return {
        lines: [
          {
            text: `Linux ${host.name.toLowerCase()} 6.8.0-40-generic #40-Ubuntu SMP PREEMPT_DYNAMIC Wed Jul 10 16:32:00 UTC 2024 x86_64 x86_64 x86_64 GNU/Linux`,
            type: 'output',
          },
        ],
      };
    }
    return { lines: [{ text: `Linux`, type: 'output' }] };
  }

  // Whoami / ID / Hostname
  if (command === 'whoami') {
    return { lines: [{ text: host.username, type: 'output' }] };
  }
  if (command === 'hostname') {
    return { lines: [{ text: host.name.toLowerCase().replace(/[^a-z0-9-]/g, '-'), type: 'output' }] };
  }
  if (command === 'id') {
    return {
      lines: [
        {
          text: `uid=0(${host.username}) gid=0(root) groups=0(root),4(adm),24(cdrom),27(sudo),30(dip),46(plugdev),110(lxd),998(docker)`,
          type: 'output',
        },
      ],
    };
  }
  if (command === 'date') {
    return { lines: [{ text: new Date().toUTCString(), type: 'output' }] };
  }
  if (command === 'uptime') {
    return {
      lines: [
        {
          text: ` 00:35:10 up 48 days, 14:22,  2 users,  load average: 0.82, 0.94, 1.15`,
          type: 'output',
        },
      ],
    };
  }

  // Free (Memory)
  if (command === 'free') {
    const isHuman = args.includes('-h') || args.includes('-m');
    return {
      lines: [
        { text: `               total        used        free      shared  buff/cache   available`, type: 'output' },
        { text: `Mem:           31.2Gi      11.2Gi      14.6Gi       340Mi       5.4Gi      19.6Gi`, type: 'output' },
        { text: `Swap:           8.0Gi       420Mi       7.58Gi`, type: 'output' },
      ],
    };
  }

  // DF (Disk)
  if (command === 'df') {
    return {
      lines: [
        { text: `Filesystem      Size  Used Avail Use% Mounted on`, type: 'output' },
        { text: `/dev/nvme0n1p2  512G  143G  344G  30% /`, type: 'output' },
        { text: `tmpfs           3.2G  1.8M  3.2G   1% /run`, type: 'output' },
        { text: `/dev/nvme0n1p1  511M  6.2M  505M   2% /boot/efi`, type: 'output' },
        { text: `/dev/nvme1n1    1.0T  380G  592G  40% /data/storage`, type: 'output' },
      ],
    };
  }

  // PS / TOP / HTOP
  if (command === 'top' || command === 'htop') {
    return {
      action: 'open_monitor',
      lines: [
        { text: `[WinSSH Pro] Switching to High-Resolution Real-time System Monitor...`, type: 'system' },
        { text: `top - 00:35:45 up 48 days, 14:22, 2 users, load average: 0.82, 0.94, 1.15`, type: 'output' },
        { text: `Tasks: 218 total,   1 running, 217 sleeping,   0 stopped,   0 zombie`, type: 'output' },
        { text: `%Cpu(s): 28.4 us,  4.2 sy,  0.0 ni, 66.8 id,  0.5 wa,  0.0 hi,  0.1 si`, type: 'output' },
        { text: `MiB Mem :  32048.2 total,  14620.4 free,  11420.8 used,   6007.0 buff/cache`, type: 'output' },
      ],
    };
  }

  if (command === 'ps') {
    return {
      lines: [
        { text: `USER         PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND`, type: 'output' },
        { text: `root           1  0.0  0.1 168420 12840 ?        Ss   Jan05   1:12 /sbin/init`, type: 'output' },
        { text: `root        1420 12.8  4.2 2480100 134200 ?      Ssl  Jan05 142:10 /usr/bin/kubelet`, type: 'output' },
        { text: `root        1980  8.5  6.8 1940200 218400 ?      Ssl  Jan05  98:45 etcd --data-dir=/var/lib/etcd`, type: 'output' },
        { text: `root        2110  4.2  3.1 1284000 98400 ?       Ssl  Jan05  44:12 kube-apiserver`, type: 'output' },
        { text: `www-data    3410  0.8  1.1 142000  34200 ?       S    Jan12   8:10 nginx: worker process`, type: 'output' },
        { text: `${host.username}     18294  0.0  0.2  18420  6840 pts/0    Ss   00:25   0:00 -bash`, type: 'output' },
      ],
    };
  }

  // Systemctl
  if (command === 'systemctl') {
    const sub = args[0] || 'status';
    const service = args[1] || 'nginx';
    if (sub === 'status') {
      return {
        lines: [
          { text: `● ${service}.service - High performance web server and reverse proxy`, type: 'success' },
          { text: `     Loaded: loaded (/lib/systemd/system/${service}.service; enabled; vendor preset: enabled)`, type: 'output' },
          { text: `     Active: active (running) since Wed 2025-01-12 04:15:22 UTC; 44 days ago`, type: 'success' },
          { text: `       Docs: man:nginx(8)`, type: 'output' },
          { text: `   Main PID: 3409 (nginx)`, type: 'output' },
          { text: `      Tasks: 5 (limit: 38412)`, type: 'output' },
          { text: `     Memory: 48.2M`, type: 'output' },
          { text: `     CGroup: /system.slice/${service}.service`, type: 'output' },
        ],
      };
    }
    return {
      lines: [{ text: `[systemd] Execution '${sub}' on unit '${service}.service' succeeded.`, type: 'success' }],
    };
  }

  // Docker
  if (command === 'docker') {
    const sub = args[0];
    if (sub === 'ps' || sub === 'container') {
      return {
        lines: [
          { text: `CONTAINER ID   IMAGE                                     COMMAND                  CREATED        STATUS          PORTS                               NAMES`, type: 'output' },
          { text: `c8f921a48b10   registry.k8s.io/kube-apiserver:v1.30.2    "kube-apiserver --ad…"   14 days ago    Up 14 days      0.0.0.0:6443->6443/tcp              k8s_api_server_prod`, type: 'output' },
          { text: `a71e442099f1   registry.k8s.io/etcd:3.5.14-0             "etcd --advertise-c…"    14 days ago    Up 14 days      2379-2380/tcp                       etcd_cluster_01`, type: 'output' },
          { text: `f92b118ce04a   ingress-nginx/controller:v1.10.1          "/nginx-ingress-cont…"   12 days ago    Up 12 days      0.0.0.0:80->80/tcp, :443->443/tcp   ingress_nginx_controller`, type: 'output' },
          { text: `e10294ab558c   redis:7.2-alpine                          "docker-entrypoint.s…"   18 days ago    Up 18 days      0.0.0.0:6379->6379/tcp              redis_cluster_cache`, type: 'output' },
        ],
      };
    }
    if (sub === 'images') {
      return {
        lines: [
          { text: `REPOSITORY                          TAG       IMAGE ID       CREATED        SIZE`, type: 'output' },
          { text: `registry.k8s.io/kube-apiserver      v1.30.2   8c9f2014b      3 weeks ago    128MB`, type: 'output' },
          { text: `registry.k8s.io/etcd                3.5.14-0  4a108e92f      4 weeks ago    312MB`, type: 'output' },
          { text: `redis                               7.2-alpine 5b8812c30      1 month ago    42.8MB`, type: 'output' },
          { text: `prom/node-exporter                  v1.8.0    9d77e108a      2 months ago   22.4MB`, type: 'output' },
        ],
      };
    }
    if (sub === 'logs') {
      const cname = args[1] || 'k8s_api_server_prod';
      return {
        lines: [
          { text: `[${cname}] 2025-02-25T00:32:10.142Z INFO [apiserver] Ready to serve HTTP/2 TLS requests on 0.0.0.0:6443`, type: 'output' },
          { text: `[${cname}] 2025-02-25T00:33:45.890Z INFO [audit] User=system:node:worker-02 Verb=get URI=/api/v1/pods Code=200 Latency=4ms`, type: 'output' },
          { text: `[${cname}] 2025-02-25T00:35:00.012Z INFO [storage-etcd] Healthy raft connection to peer etcd-01`, type: 'success' },
        ],
      };
    }
    if (sub === 'restart' || sub === 'stop' || sub === 'start') {
      return {
        lines: [{ text: `${args[1] || 'container'} ${sub}ed successfully.`, type: 'success' }],
      };
    }
    return {
      lines: [
        { text: `Usage: docker [OPTIONS] COMMAND`, type: 'output' },
        { text: `Commands: ps, images, logs, restart, stop, start, exec, pull, run`, type: 'output' },
      ],
    };
  }

  // Kubectl
  if (command === 'kubectl' || command === 'k') {
    const sub = args[0];
    if (sub === 'get') {
      const res = args[1] || 'pods';
      if (res.includes('pod')) {
        return {
          lines: [
            { text: `NAMESPACE     NAME                                    READY   STATUS    RESTARTS   AGE     IP             NODE`, type: 'output' },
            { text: `kube-system   coredns-768b85b76f-4r8q2                1/1     Running   0          14d     10.244.0.2     prod-k8s-01`, type: 'output' },
            { text: `kube-system   kube-proxy-8x9lm                        1/1     Running   0          14d     192.168.1.100  prod-k8s-01`, type: 'output' },
            { text: `production    backend-api-6d8b94f99-x72km             2/2     Running   0          3d      10.244.1.18    worker-node-01`, type: 'success' },
            { text: `production    frontend-web-7c859421a-m92lw            1/1     Running   0          3d      10.244.1.19    worker-node-01`, type: 'success' },
            { text: `production    redis-cluster-master-0                  1/1     Running   0          18d     10.244.2.40    worker-node-02`, type: 'output' },
          ],
        };
      }
      if (res.includes('node')) {
        return {
          lines: [
            { text: `NAME            STATUS   ROLES           AGE   VERSION`, type: 'output' },
            { text: `prod-k8s-01     Ready    control-plane   48d   v1.30.2`, type: 'success' },
            { text: `worker-node-01  Ready    <none>          48d   v1.30.2`, type: 'success' },
            { text: `worker-node-02  Ready    <none>          48d   v1.30.2`, type: 'success' },
          ],
        };
      }
    }
    return {
      lines: [{ text: `kubectl client version: v1.30.2 / server version: v1.30.2`, type: 'output' }],
    };
  }

  // Ping
  if (command === 'ping') {
    const target = args[0] || '8.8.8.8';
    return {
      lines: [
        { text: `PING ${target} (${target}) 56(84) bytes of data.`, type: 'output' },
        { text: `64 bytes from ${target}: icmp_seq=1 ttl=118 time=14.2 ms`, type: 'output' },
        { text: `64 bytes from ${target}: icmp_seq=2 ttl=118 time=13.8 ms`, type: 'output' },
        { text: `64 bytes from ${target}: icmp_seq=3 ttl=118 time=14.5 ms`, type: 'output' },
        { text: `--- ${target} ping statistics ---`, type: 'system' },
        { text: `3 packets transmitted, 3 received, 0% packet loss, time 2003ms`, type: 'success' },
        { text: `rtt min/avg/max/mdev = 13.812/14.166/14.510/0.285 ms`, type: 'output' },
      ],
    };
  }

  // Curl
  if (command === 'curl') {
    const url = args[0] || 'http://localhost:80';
    return {
      lines: [
        { text: `HTTP/1.1 200 OK`, type: 'success' },
        { text: `Server: nginx/1.24.0 (Ubuntu)`, type: 'output' },
        { text: `Date: ${new Date().toUTCString()}`, type: 'output' },
        { text: `Content-Type: text/html; charset=UTF-8`, type: 'output' },
        { text: `Content-Length: 612`, type: 'output' },
        { text: `<!DOCTYPE html><html><head><title>Welcome to WinSSH Pro Server</title></head><body><h1>System Operational</h1></body></html>`, type: 'output' },
      ],
    };
  }

  // Git
  if (command === 'git') {
    const sub = args[0];
    if (sub === 'status') {
      return {
        lines: [
          { text: `On branch main`, type: 'success' },
          { text: `Your branch is up to date with 'origin/main'.`, type: 'output' },
          { text: `nothing to commit, working tree clean`, type: 'success' },
        ],
      };
    }
    if (sub === 'branch') {
      return {
        lines: [
          { text: `* main`, type: 'success' },
          { text: `  staging`, type: 'output' },
          { text: `  feature/k8s-upgrade`, type: 'output' },
        ],
      };
    }
    if (sub === 'log') {
      return {
        lines: [
          { text: `commit 8f9a21b44210e7d991 (HEAD -> main, origin/main)`, type: 'output' },
          { text: `Author: DevOps Lead <devops@winssh.pro>`, type: 'output' },
          { text: `Date:   Mon Feb 24 18:22:10 2025 +0000`, type: 'output' },
          { text: `    feat(ci): automate rolling restart of microservice deployments`, type: 'output' },
        ],
      };
    }
    return {
      lines: [{ text: `git version 2.43.0`, type: 'output' }],
    };
  }

  // Echo
  if (command === 'echo') {
    const text = args.join(' ').replace(/^["']|["']$/g, '');
    return { lines: [{ text, type: 'output' }] };
  }

  // Mkdir / Touch / RM
  if (command === 'mkdir' || command === 'touch' || command === 'rm') {
    const target = args[args.length - 1] || 'item';
    return {
      lines: [{ text: `${command}: operation completed for '${target}'`, type: 'success' }],
    };
  }

  // Ifconfig / ip a
  if (command === 'ifconfig' || command === 'ip') {
    return {
      lines: [
        { text: `eth0: flags=4163<UP,BROADCAST,RUNNING,MULTICAST>  mtu 1500`, type: 'output' },
        { text: `        inet ${host.host}  netmask 255.255.255.0  broadcast 192.168.1.255`, type: 'success' },
        { text: `        inet6 fe80::5054:ff:fe12:3456  prefixlen 64  scopeid 0x20<link>`, type: 'output' },
        { text: `        ether 52:54:00:12:34:56  txqueuelen 1000  (Ethernet)`, type: 'output' },
        { text: `        RX packets 14892014  bytes 12845920182 (12.8 GB)`, type: 'output' },
        { text: `        TX packets 9845120   bytes 8421094812 (8.4 GB)`, type: 'output' },
      ],
    };
  }

  // Fallback realistic execution output
  return {
    lines: [
      { text: `[WinSSH Pro Execution] ${command}: command executed with exit code 0`, type: 'output' },
      { text: `Result: stdout stream closed successfully.`, type: 'output' },
    ],
  };
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + sizes[i];
}

function getOsBanner(osType: string, hostName: string, user: string): string[] {
  const line1 = `${user}@${hostName}`;
  const sep = '-'.repeat(line1.length);

  if (osType === 'ubuntu' || osType === 'debian') {
    return [
      `            .-/+oossssoo+/-.               ${line1}`,
      `        \`:+ssssssssssssssssss+:\`           ${sep}`,
      `      -+ssssssssssssssssssyyssss+-         OS: Ubuntu 24.04 LTS x86_64`,
      `    .ossssssssssssssssssdMMMNysssso.       Host: K8s Baremetal Node`,
      `   /ssssssssssshdmmNNmmyNMMMMhssssss/      Kernel: Linux 6.8.0-40-generic`,
      `  +ssssssssshmydMMMMMMMNddddyssssssss+     Uptime: 48 days, 14 hours`,
      ` /sssssssshNMMMyhhyyyyhmNMMMNhssssssss/    Packages: 1248 (dpkg), 12 (snap)`,
      `.ssssssssdMMMNhsssssssssshNMMMdssssssss.   Shell: bash 5.2.21`,
      `+sssshhhyNMMNyssssssssssssyNMMMysssssss+   Terminal: WinSSH Pro Terminal v2.4`,
      `ossyNMMMNyMMhsssssssssssssshmmmhssssssso   CPU: AMD EPYC 7763 (32) @ 3.24GHz`,
      `ossyNMMMNyMMhsssssssssssssshmmmhssssssso   GPU: Virtual VGA Console`,
      `+sssshhhyNMMNyssssssssssssyNMMMysssssss+   Memory: 11420MiB / 32048MiB (35%)`,
      `.ssssssssdMMMNhsssssssssshNMMMdssssssss.   Disk (/): 143GB / 512GB (28%)`,
      ` /sssssssshNMMMyhhyyyyhdNMMMNhssssssss/    Local IP: 192.168.1.100`,
      `  +sssssssssdmydMMMMMMMMddddyssssssss+     `,
      `   /ssssssssssshdmNNNNmyNMMMMhssssss/      \x1b[41m   \x1b[42m   \x1b[43m   \x1b[44m   \x1b[45m   \x1b[46m   \x1b[47m   \x1b[0m`,
    ];
  }

  if (osType === 'alpine') {
    return [
      `   /\\ /\\                ${line1}`,
      `  // \\  \\               ${sep}`,
      ` //   \\  \\              OS: Alpine Linux v3.20 x86_64`,
      `///    \\  \\             Kernel: Linux 6.6.32-0-lts`,
      `//      \\  \\            Uptime: 14 days, 6 hours`,
      `         \\              Shell: ash / busybox`,
      `                        Packages: 98 (apk)`,
      `                        Memory: 340MiB / 2048MiB`,
    ];
  }

  return [
    `  ___       __ _       ${line1}`,
    ` / _ \\ ___ / _(_)__ _  ${sep}`,
    `| (_) (_-<|  _| / _\` | OS: Linux Enterprise Server x86_64`,
    ` \\___/___/|_| |_\\__, | Kernel: Linux 6.1.0-22-amd64`,
    `                |___/  Uptime: 112 days, 2 hours`,
    `                       Memory: 28.5GB / 64.0GB`,
  ];
}
